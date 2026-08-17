// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Everything the webview is allowed to ask for.
//!
//! All server traffic happens here rather than in JavaScript, for two reasons
//! that both only appear once the app is packaged:
//!
//!  1. **No cross-origin problem.** In development the page is served by Vite,
//!     which proxies `/v1` to the server. A bundled app loads from
//!     `tauri://localhost`, so the same call would be cross-origin and every
//!     self-hosted server would need CORS configured before the product worked
//!     at all. Making the request from Rust removes the question.
//!
//!  2. **The token never enters the webview.** See `session.rs`.
//!
//! The commands deliberately mirror the server's own error shape — status plus
//! its JSON body — so the messages an operator reads are built in one place
//! (`api.ts`) whether the call went through Rust or, in browser development
//! mode, through fetch.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use fury_shared::api::{ProfileSummary, ProjectSummary};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::session::Session;
use crate::crypto;
use crate::settings::{self, Settings};

pub struct AppState {
    pub http: reqwest::Client,
    pub settings: Mutex<Settings>,
    pub config_dir: PathBuf,
    pub session: Session,

    /// Lock tokens held by this process, by profile id.
    ///
    /// Kept here for the same reason as the session token. A lock token
    /// authorises a bundle upload that overwrites whatever a force-released
    /// holder had — handing it to the webview would undo the one property this
    /// whole layer exists to establish. The agent will take these over when it
    /// lands; until then they are what lets `unlock` prove it is the holder.
    pub locks: Mutex<HashMap<String, String>>,

    /// The organisation key, unwrapped at sign-in and held only in memory.
    ///
    /// Never written to disk and never handed to the webview — it opens every
    /// proxy credential and every profile bundle in the organisation, so it
    /// lives exactly as long as the process and no longer. Signing out drops
    /// it; so does quitting. That is the intended cost of the design.
    pub org_key: Mutex<Option<[u8; 32]>>,

    /// The highest organisation-key generation this machine has ever accepted.
    ///
    /// Persisted, because the attack it stops spans restarts: the wrapping of
    /// an organisation key carries no signature, so a server that kept a
    /// member's pre-rotation blob could serve it back and return them to a key
    /// a removed colleague still holds. Everything they sealed afterwards would
    /// be readable by that person and by nobody else in the team. A watermark
    /// that only moves forward makes that a refusal instead.
    pub ork_generation: Mutex<i32>,
}

/// A failed call, in the same shape the browser transport produces.
///
/// `status` is 0 for failures that never reached the server, which the UI
/// distinguishes because "your session expired" and "this machine cannot reach
/// the server" send an operator to completely different places.
#[derive(Debug, Serialize)]
pub struct ApiErr {
    pub status: u16,
    pub body: serde_json::Value,
    pub message: String,
    /// A stable name for the failures this process produces itself, so the
    /// interface can say them in the operator's language. The message stays as
    /// the fallback: an untranslated sentence beats a bare code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
}

impl From<crate::agent::AgentError> for ApiErr {
    fn from(e: crate::agent::AgentError) -> Self {
        // Status 0 is "never reached a server", which is exactly what an agent
        // failure is from the interface's point of view.
        match e {
            crate::agent::AgentError::NotRunning => ApiErr::coded(
                "err.agentDown",
                "The local agent is not running. Nothing can be launched without it.",
            ),
            other => ApiErr::local(other.to_string()),
        }
    }
}

impl ApiErr {
    pub(crate) fn local(message: impl Into<String>) -> Self {
        Self {
            status: 0,
            body: serde_json::Value::Null,
            message: message.into(),
            code: None,
        }
    }

    fn coded(code: &'static str, message: impl Into<String>) -> Self {
        Self { code: Some(code), ..Self::local(message) }
    }
}

type R<T> = Result<T, ApiErr>;

// ---------------------------------------------------------------------------
// transport
// ---------------------------------------------------------------------------

enum Body {
    None,
    Json(serde_json::Value),
}

impl AppState {
    fn server_url(&self) -> R<String> {
        self.settings
            .lock()
            .unwrap()
            .server_url
            .clone()
            .ok_or_else(|| ApiErr::local("No server configured yet."))
    }

    /// Whether the server is holding no organisation key for this account.
    ///
    /// Its own request rather than `call`, and the reason is the timeout. This
    /// is asked from `shell_state`, which is the first thing the window calls
    /// and the thing it waits on before drawing anything — so a server that has
    /// gone away would hold the splash screen for the client's full twenty
    /// seconds before the person could reach the button that says "work without
    /// an account". Five, and an unanswered question is treated as "no", which
    /// leaves the screen saying the thing that is true either way.
    async fn key_withheld(&self) -> bool {
        let (Ok(base), Some(token)) = (self.server_url(), self.session.token()) else {
            return false;
        };
        let res = self
            .http
            .get(format!("{base}/v1/me"))
            .bearer_auth(token)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
        let Ok(res) = res else { return false };
        // The same treatment `call` gives a dead session: drop it here rather
        // than let every later request rediscover it.
        if res.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.session.clear();
            return false;
        }
        let Ok(body) = res.json::<serde_json::Value>().await else {
            return false;
        };
        // Absent — an older server — is not "withheld". Being wrong in that
        // direction offers a password that works; the other direction tells
        // somebody to wait for a colleague who has nothing to do.
        body.get("has_org_key").and_then(|v| v.as_bool()) == Some(false)
    }

    /// The organisation key, from memory or from the keychain.
    ///
    /// Lazy: the first command that needs it pays for the keychain read, and
    /// nothing on the startup path does.
    fn org_key(&self) -> Option<[u8; 32]> {
        if let Some(key) = *self.org_key.lock().unwrap() {
            return Some(key);
        }
        if !self.settings.lock().unwrap().remember_org_key {
            return None;
        }
        let found = self.session.load_org_key()?;
        *self.org_key.lock().unwrap() = Some(found);
        Some(found)
    }

    fn machine_id(&self) -> String {
        self.settings.lock().unwrap().machine_id.clone()
    }

    /// One request. Locks are taken and released before any `.await`, because a
    /// guard held across a suspension point would make the future non-Send and,
    /// worse, could deadlock two commands issued from the same click.
    async fn call<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Body,
        authenticated: bool,
    ) -> R<T> {
        let base = self.server_url()?;
        let token = if authenticated {
            match self.session.token() {
                Some(t) => Some(t),
                // Fail here rather than sending an anonymous request: the server
                // would answer 401 and the UI would report an expired session,
                // when in fact we never had one.
                None => return Err(unauthenticated()),
            }
        } else {
            None
        };

        let mut req = self.http.request(method, format!("{base}{path}"));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        if let Body::Json(v) = body {
            req = req.json(&v);
        }

        let res = req.send().await.map_err(|e| {
            ApiErr::local(if e.is_connect() || e.is_timeout() {
                format!("Cannot reach {base}. Check the address and your network.")
            } else {
                format!("Request failed: {e}")
            })
        })?;

        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);

        if !status.is_success() {
            // A dead session is dropped immediately. Keeping it would produce a
            // wall of identical failures on every subsequent poll.
            if status == reqwest::StatusCode::UNAUTHORIZED {
                self.session.clear();
            }
            return Err(ApiErr {
                status: status.as_u16(),
                body: parsed,
                message: format!("Request failed ({status})."),
                code: None,
            });
        }

        // 204 and other empty successes still have to satisfy T, which is `()`
        // for those endpoints.
        if text.trim().is_empty() {
            return serde_json::from_str("null").map_err(|e| ApiErr::local(e.to_string()));
        }
        serde_json::from_value(parsed).map_err(|e| {
            ApiErr::local(format!(
                "The server answered in a shape this version does not understand: {e}"
            ))
        })
    }
}

fn unauthenticated() -> ApiErr {
    ApiErr {
        status: 401,
        body: serde_json::json!({ "error": "unauthenticated" }),
        message: "Not signed in.".into(),
        code: Some("err.notSignedIn"),
    }
}

// ---------------------------------------------------------------------------
// settings & session
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct Shell {
    /// The agent's `core_download` block, passed through verbatim. None until
    /// the agent has answered once; a shell that has never asked and a download
    /// that has never started look the same to the user and should.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_download: Option<serde_json::Value>,
    pub server_url: Option<String>,
    pub machine_name: String,
    pub signed_in: bool,
    /// Lets the UI say "desktop" where the browser build has to stay vague.
    pub native: bool,
    /// `"local"` or `"team"`.
    ///
    /// Local is the default and the whole product for one person: no account,
    /// no server, no database. A sign-in screen appears only once there is a
    /// server to sign in to — asking for a password with nothing to check it
    /// against was the shape of the first version and it was wrong.
    pub mode: &'static str,
    /// Whether the local daemon answered. Nothing can be launched without it.
    pub agent_ready: bool,
    /// Whether a browser is installed. Separate from `agent_ready` because the
    /// application is one download and the browser is another, and somebody who
    /// took the first and not the second has an app that works up to the moment
    /// they press Launch.
    pub core_ready: bool,
    /// The agent's own sentence about why not, when it has one.
    pub core_problem: Option<String>,
    /// This build, for the About panel and for any bug report that follows it.
    pub version: &'static str,

    /// Whether this process holds the organisation key.
    ///
    /// A session survives a restart — the token is in the keychain — and the
    /// key deliberately does not: it lives in memory for exactly as long as the
    /// process. So "signed in" and "able to decrypt" are different states, and
    /// the second one has to be visible, or the application comes back looking
    /// signed in and quietly unable to open anything.
    pub org_key_ready: bool,
    /// Who signed in last, so unlocking asks for one field instead of two.
    pub last_email: Option<String>,
    /// Signed in, holding no key, and there is no key on the server to hold.
    ///
    /// The other half of `org_key_ready`, and the reason it is not enough on its
    /// own: "this process has no key" covers two people. One turned off
    /// remembering it and needs to type a password. The other enrolled with an
    /// invitation and has not been handed the key yet — for them a password is
    /// not a slower way in, it is no way in at all, and the screen offered them
    /// one anyway. They typed it, it was right, and they arrived back at the
    /// same box. Reported from the machine somebody was trying to join on.
    pub awaiting_key: bool,
}

/// Does this action go to the local store, or to the server?
///
/// The question used to be "what mode is the shell in", which was the same
/// question while the list showed one world at a time. It is not the same
/// question any more: connected to a server, the list carries both, and a row
/// from this machine must be launched, edited and deleted here no matter what
/// the shell is connected to.
///
/// `origin` comes back from the interface, on the row the person clicked. When
/// it is absent -- an older caller, or an action that is not about one
/// particular profile -- the mode decides, which is exactly the previous
/// behaviour.
fn team_origin() -> &'static str {
    "team"
}

fn local_for(state: &AppState, origin: Option<&str>) -> bool {
    match origin {
        Some("local") => true,
        Some("team") => false,
        _ => mode_of(state) == "local",
    }
}

fn mode_of(state: &AppState) -> &'static str {
    if state.settings.lock().unwrap().server_url.is_some() {
        "team"
    } else {
        "local"
    }
}

/// Ask the agent to fetch the browser and install it.
///
/// Returns as soon as the download has started; progress arrives in
/// `shell_state`, which the window already polls. A command that waited would
/// hold the UI for the length of a 134 MB download.
#[tauri::command]
pub async fn download_core(_state: State<'_, AppState>) -> Result<(), ApiErr> {
    crate::agent::ensure_running().await.map_err(ApiErr::from)?;
    crate::agent::call::<serde_json::Value>("core.download", serde_json::json!({}))
        .await
        .map(|_| ())
        .map_err(ApiErr::from)
}

#[tauri::command]
pub async fn shell_state(state: State<'_, AppState>) -> Result<Shell, ApiErr> {
    let mode = mode_of(&state);
    // Started on demand rather than announced as missing: telling someone to
    // open a terminal would make the desktop app useless to the people it is
    // for.
    let agent_ready = crate::agent::ensure_running().await.is_ok();

    // Asked once, here, rather than discovered on the first launch attempt.
    // A missing core is a five-minute fix and a confusing failure, and the
    // order those happen in is the whole difference.
    let mut core_download = None;
    let (core_ready, core_problem) = if agent_ready {
        match crate::agent::call::<serde_json::Value>("status", serde_json::json!({})).await {
            Ok(v) => (
                {
                    // Carried through untouched. The shell decides what to draw
                    // from it, and a struct here would be a third place that has
                    // to learn about every new field.
                    core_download = v.get("core_download").cloned();
                    v.get("core").map(|c| !c.is_null()).unwrap_or(false)
                },
                v.get("core_problem")
                    .and_then(|p| p.as_str())
                    .map(str::to_string),
            ),
            // The agent answered the socket and not this call. Not worth a
            // second warning bar — agent_ready is already false-ish territory.
            Err(_) => (true, None),
        }
    } else {
        (true, None)
    };

    // Both settings fields in one lock, released before the struct is built.
    //
    // Two `settings.lock()` calls inside a single struct literal deadlocked:
    // the temporaries a struct literal creates live until the end of the whole
    // expression, so the first guard was still held when the second asked for
    // the same non-reentrant mutex. The application started, drew its window,
    // and never left the splash — the first call it makes never returned.
    let (server_url, last_email) = {
        let s = state.settings.lock().unwrap();
        (s.server_url.clone(), s.last_email.clone())
    };
    let org_key_ready = state.org_key().is_some();

    // Asked of the server only in the one state where the answer changes what
    // the screen says: signed in, and holding nothing. Everywhere else it is
    // decided locally and this poll — which runs every five seconds — makes no
    // request at all.
    let awaiting_key =
        !org_key_ready && state.session.is_signed_in() && state.key_withheld().await;

    Ok(Shell {
        server_url,
        machine_name: settings::machine_name(),
        // Read after the call above, which drops the session if the server says
        // it is dead.
        signed_in: state.session.is_signed_in(),
        native: true,
        mode,
        agent_ready,
        core_download,
        core_ready,
        core_problem,
        version: env!("CARGO_PKG_VERSION"),
        org_key_ready,
        last_email,
        awaiting_key,
    })
}

/// Forgets the server and returns to working locally.
///
/// The session is revoked on the way out — leaving a live one behind on a
/// server the operator has walked away from is the same defect as a sign-out
/// that does not sign out.
#[tauri::command]
pub async fn disconnect_server(state: State<'_, AppState>) -> Result<Shell, ApiErr> {
    if state.session.is_signed_in() {
        let _ = logout(state.clone()).await;
    }
    {
        let mut s = state.settings.lock().unwrap();
        s.server_url = None;
        s.save(&state.config_dir)
            .map_err(|e| ApiErr::local(format!("Could not save settings: {e}")))?;
    }
    shell_state(state).await
}

/// Points this installation at a server, after checking something is there.
///
/// The check is what makes the setup screen worth having: a typo'd host
/// otherwise surfaces as a failed sign-in, which reads as "wrong password".
#[tauri::command]
pub async fn set_server(state: State<'_, AppState>, url: String) -> R<Shell> {
    let normalised = settings::normalise_server_url(&url).map_err(ApiErr::local)?;

    // /v1/me with no token. A Fury server answers exactly 401 with
    // {"error":"unauthenticated"} — see server/src/error.rs. Checking for that
    // pair rather than "some JSON with an error key" matters: half the APIs on
    // the internet return an error object, and accepting one of those here would
    // save an address that fails much later, during sign-in, where it reads as a
    // wrong password.
    let probe = state
        .http
        .get(format!("{normalised}/v1/me"))
        .send()
        .await
        .map_err(|e| {
            ApiErr::local(if e.is_connect() || e.is_timeout() {
                format!("Nothing answered at {normalised}.")
            } else {
                format!("Could not reach {normalised}: {e}")
            })
        })?;

    let status = probe.status();
    let body: Option<serde_json::Value> = probe
        .text()
        .await
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok());

    let looks_like_fury = status == reqwest::StatusCode::UNAUTHORIZED
        && body
            .as_ref()
            .and_then(|v| v.get("error"))
            .and_then(serde_json::Value::as_str)
            == Some("unauthenticated");

    if !looks_like_fury {
        return Err(ApiErr::local(format!(
            "{normalised} answered ({status}) but does not look like a Fury server."
        )));
    }

    {
        let mut s = state.settings.lock().unwrap();
        s.server_url = Some(normalised.clone());
        s.save(&state.config_dir)
            .map_err(|e| ApiErr::local(format!("Could not save settings: {e}")))?;
    }
    // Rebinding also swaps in whatever token was stored for this server.
    state.session.bind(&normalised);

    shell_state(state).await
}

#[derive(Serialize, Deserialize)]
pub struct Me {
    pub user_id: String,
    pub email: String,
    pub org_id: String,
    pub role: String,
}

#[tauri::command]
pub async fn login(state: State<'_, AppState>, email: String, password: String) -> R<Me> {
    #[derive(Deserialize)]
    struct LoginOk {
        token: String,
        wrapped_private_key: String,
        kdf_salt: String,
        wrapped_ork: Option<String>,
        #[serde(default)]
        ork_generation: Option<i32>,
    }

    let body = serde_json::json!({
        "email": email,
        "password": password,
        "machine_name": settings::machine_name(),
    });

    let ok: LoginOk = state
        .call(reqwest::Method::POST, "/v1/auth/login", Body::Json(body), false)
        .await?;
    state.session.store(&ok.token);

    // Unwrap the organisation key while the password is still in hand — it is
    // the only moment it exists in this process. A failure here is not fatal to
    // signing in: someone whose key has not been handed over yet still has an
    // account, and should be able to see the team rather than a login error.
    {
        let mut settings = state.settings.lock().unwrap();
        settings.last_email = Some(email.clone());
        let _ = settings.save(&state.config_dir);
    }

    // Never backwards. A generation below what this machine has already seen is
    // a rollback, whatever produced it.
    {
        let seen = *state.ork_generation.lock().unwrap();
        if let Some(g) = ok.ork_generation {
            if g < seen {
                return Err(ApiErr::coded(
                    "err.staleOrgKey",
                    format!(
                        "This server offered organisation key generation {g}, but this machine \
                         has already used {seen}. Refusing — a key that goes backwards is one \
                         somebody else may still hold."
                    ),
                ));
            }
        }
    }

    match crypto::unlock_org_key(&password, &ok.kdf_salt, &ok.wrapped_private_key, ok.wrapped_ork.as_deref()) {
        Ok(key) => {
            *state.org_key.lock().unwrap() = key;
            if let Some(k) = key.as_ref() {
                if state.settings.lock().unwrap().remember_org_key {
                    state.session.store_org_key(k);
                }
            }
            if let Some(g) = ok.ork_generation {
                let mut seen = state.ork_generation.lock().unwrap();
                *seen = (*seen).max(g);
                let mut s = state.settings.lock().unwrap();
                s.ork_generation = *seen;
                let _ = s.save(&state.config_dir);
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "signed in without the organisation key");
            *state.org_key.lock().unwrap() = None;
        }
    }

    // Fetch identity immediately: every later screen needs it, and a login that
    // succeeds but whose session cannot be used should fail here, visibly.
    state
        .call(reqwest::Method::GET, "/v1/me", Body::None, true)
        .await
}

/// What an invitation code is for, before anyone types a password.
#[tauri::command]
pub async fn invitation(state: State<'_, AppState>, url: String, code: String) -> R<serde_json::Value> {
    // The server is not configured yet at this point — this is how someone
    // reaches a server for the first time — so the address comes in with the
    // call rather than from settings.
    let base = settings::normalise_server_url(&url).map_err(ApiErr::local)?;
    let sanitised: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let resp = state
        .http
        .get(format!("{base}/v1/auth/enroll/{sanitised}"))
        .send()
        .await
        .map_err(|e| ApiErr::local(format!("Could not reach {base}: {e}")))?;

    if resp.status() == 404 {
        return Err(ApiErr::local(
            "This invitation is not valid. It may have been used already, or expired.",
        ));
    }
    resp.json()
        .await
        .map_err(|e| ApiErr::local(format!("The server answered something unexpected: {e}")))
}

/// Whether a server takes open sign-ups, asked before anyone has an account.
///
/// Unauthenticated on purpose: the launcher has to know whether to offer the
/// button, and an old server that has never heard of this endpoint answers 404 —
/// which reads as "no", correctly.
#[tauri::command]
pub async fn server_allows_signup(state: State<'_, AppState>, url: String) -> R<bool> {
    let base = settings::normalise_server_url(&url).map_err(ApiErr::local)?;
    let resp = state
        .http
        .get(format!("{base}/v1/server"))
        .send()
        .await
        .map_err(|e| ApiErr::local(format!("Could not reach {base}: {e}")))?;
    if !resp.status().is_success() {
        return Ok(false);
    }
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    Ok(body.get("open_signup").and_then(|v| v.as_bool()).unwrap_or(false))
}

/// Make an account on a server that takes them, with no invitation.
///
/// The same key generation as enrolment, and deliberately the same function:
/// the difference between the two is who vouched for the address, not what the
/// client makes. A sign-up always creates an organisation, so it always creates
/// the organisation key — `creates_org` is true and cannot be otherwise.
#[tauri::command]
pub async fn signup(
    state: State<'_, AppState>,
    url: String,
    email: String,
    password: String,
    org_name: String,
) -> R<Me> {
    let base = settings::normalise_server_url(&url).map_err(ApiErr::local)?;

    let e = crypto::enrol(&password, true)
        .map_err(|e| ApiErr::local(format!("Could not generate keys: {e}")))?;
    let wrapped_ork = e.wrapped_ork.clone().ok_or_else(|| {
        ApiErr::local("Could not generate the organisation key.".to_string())
    })?;

    let resp = state
        .http
        .post(format!("{base}/v1/auth/signup"))
        .json(&serde_json::json!({
            "email": email,
            "password": password,
            "org_name": org_name,
            "public_key": e.public_key,
            "wrapped_private_key": e.wrapped_private_key,
            "kdf_salt": e.kdf_salt,
            "wrapped_ork": wrapped_ork,
            "machine_name": settings::machine_name(),
        }))
        .send()
        .await
        .map_err(|err| ApiErr::local(format!("Could not reach {base}: {err}")))?;

    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    if status >= 400 {
        return Err(ApiErr {
            status,
            message: body
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("The server refused the sign-up.")
                .to_string(),
            body,
            code: None,
        });
    }

    let token = body
        .get("token")
        .and_then(|t| t.as_str())
        .ok_or_else(|| ApiErr::local("The server answered without a session.".to_string()))?
        .to_string();

    // Remembered only once it worked — see the same note on enrol.
    {
        let mut s = state.settings.lock().unwrap();
        s.server_url = Some(base.clone());
        s.last_email = Some(email);
        s.save(&state.config_dir)
            .map_err(|err| ApiErr::local(format!("Could not save settings: {err}")))?;
    }
    state.session.bind(&base);
    state.session.store(&token);
    *state.org_key.lock().unwrap() = e.org_key;
    if let Some(k) = e.org_key.as_ref() {
        if state.settings.lock().unwrap().remember_org_key {
            state.session.store_org_key(k);
        }
    }

    state
        .call(reqwest::Method::GET, "/v1/me", Body::None, true)
        .await
}

/// Complete an invitation: generate the keys, post the wrapped results, sign in.
#[tauri::command]
pub async fn enrol(
    state: State<'_, AppState>,
    url: String,
    code: String,
    password: String,
    creates_org: bool,
) -> R<Me> {
    let base = settings::normalise_server_url(&url).map_err(ApiErr::local)?;

    // Everything secret is made here and stays here. What crosses the wire is
    // wrapped under a key the server never receives.
    let e = crypto::enrol(&password, creates_org)
        .map_err(|e| ApiErr::local(format!("Could not generate keys: {e}")))?;

    #[derive(Deserialize)]
    struct EnrolOk {
        token: String,
    }

    let resp = state
        .http
        .post(format!("{base}/v1/auth/enroll"))
        .json(&serde_json::json!({
            "code": code,
            "password": password,
            "public_key": e.public_key,
            "wrapped_private_key": e.wrapped_private_key,
            "kdf_salt": e.kdf_salt,
            "wrapped_ork": e.wrapped_ork,
            "machine_name": settings::machine_name(),
        }))
        .send()
        .await
        .map_err(|err| ApiErr::local(format!("Could not reach {base}: {err}")))?;

    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    if status >= 400 {
        return Err(ApiErr {
            status,
            message: body
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("The server refused the invitation.")
                .to_string(),
            body,
            code: None,
        });
    }
    let ok: EnrolOk = serde_json::from_value(body)
        .map_err(|err| ApiErr::local(format!("The server answered something unexpected: {err}")))?;

    // The server is only remembered once enrolment succeeded. A half-configured
    // address left behind by a failed attempt would send the next launch to a
    // login screen for a server the person has no account on.
    {
        let mut s = state.settings.lock().unwrap();
        s.server_url = Some(base.clone());
        s.save(&state.config_dir)
            .map_err(|err| ApiErr::local(format!("Could not save settings: {err}")))?;
    }
    state.session.bind(&base);
    state.session.store(&ok.token);
    *state.org_key.lock().unwrap() = e.org_key;
    if let Some(k) = e.org_key.as_ref() {
        if state.settings.lock().unwrap().remember_org_key {
            state.session.store_org_key(k);
        }
    }

    let me: Me = state
        .call(reqwest::Method::GET, "/v1/me", Body::None, true)
        .await?;

    // Remembered here as well as at sign-in. A member enrolling with an
    // invitation holds no organisation key yet, so the very next screen they
    // see is the one that asks for it — and without this it did not know who
    // they were and asked for the address again, on the machine that had just
    // typed it in.
    {
        let mut s = state.settings.lock().unwrap();
        s.last_email = Some(me.email.clone());
        let _ = s.save(&state.config_dir);
    }

    Ok(me)
}

#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> R<()> {
    // Revoke first, forget second — the same ordering the browser build got
    // wrong. If the server cannot be reached we still drop the local copy,
    // because a user who asked to sign out must end up signed out.
    let revoked: R<serde_json::Value> = state
        .call(reqwest::Method::POST, "/v1/auth/logout", Body::None, true)
        .await;
    state.session.clear();
    // The organisation key goes with the session. Leaving it in memory would
    // mean a signed-out application that can still open every credential it
    // saw — which is exactly the state "sign out" is asked for.
    *state.org_key.lock().unwrap() = None;
    match revoked {
        Ok(_) => Ok(()),
        Err(e) if e.status == 401 => Ok(()),
        Err(e) => Err(e),
    }
}

#[tauri::command]
pub async fn me(state: State<'_, AppState>) -> R<Me> {
    state
        .call(reqwest::Method::GET, "/v1/me", Body::None, true)
        .await
}

// ---------------------------------------------------------------------------
// projects, profiles, locks
// ---------------------------------------------------------------------------

// Deserialised into the shared types rather than passed through as opaque JSON:
// if the server changes a field, this stops compiling instead of producing a
// blank column that nobody notices.

/// One shape for the table, whichever half of the product produced it.
///
/// Local and team mode answer different questions — one has permissions and a
/// distributed lock, the other has a running process — and folding them here
/// rather than in the interface keeps a single table component instead of two
/// that drift apart.
#[derive(Serialize)]
pub struct UiProject {
    /// "local" for a folder on this machine, "team" for one on the server.
    ///
    /// Added for the same reason UiProfile has one: with both worlds in one
    /// sidebar, the shell's mode no longer says which side a row belongs to,
    /// and every action on it has to follow the row.
    #[serde(default = "team_origin")]
    pub origin: &'static str,
    pub id: String,
    pub name: String,
    pub profile_count: i64,
}

#[derive(Serialize)]
pub struct UiProxy {
    pub id: String,
    pub name: String,
    pub kind: String,
    /// Host as the caller is allowed to see it. Masked by the server for anyone
    /// without `reveal_secrets`; never masked locally, where there is nobody to
    /// hide it from and the operator has to be able to check what they typed.
    pub display: String,
    pub country: Option<String>,
}

#[derive(Serialize)]
pub struct UiProfile {
    pub id: String,
    pub project_id: Option<String>,
    /// Where it is filed, for the flat list to show without a request per row.
    pub project_name: Option<String>,
    pub name: String,
    pub tags: Vec<String>,
    pub persona_id: String,
    /// Zero in team mode: the server never exposes a seed, and nothing in the
    /// interface may invent one.
    pub fp_seed: i64,
    pub proxy: Option<UiProxy>,
    /// `None` means "follow the proxy's exit", resolved by the agent at launch.
    /// Carried here because the edit dialog round-trips it — without these two
    /// fields, opening a profile and pressing save silently reset both.
    pub timezone: Option<String>,
    pub languages: Option<Vec<String>>,
    pub permissions: Vec<String>,
    pub lock: Option<serde_json::Value>,
    /// Only ever true in local mode today: the agent knows what it launched.
    /// In team mode a colleague's browser is visible through `lock`, not here.
    pub running: bool,
    pub last_opened_at: Option<String>,
    /// How many people hold this profile through a share. Zero for a local one.
    pub shared_with: i64,
    /// Which world this row came from: "local" for this machine's own store,
    /// "team" for the server.
    ///
    /// Added because the list stopped being one world. Connecting to a server
    /// used to hide every local profile, and the first person to try it read
    /// that as his profiles having been deleted -- reasonably, since nothing
    /// said otherwise and they were nowhere on screen. They were in the store
    /// the whole time.
    ///
    /// Every action that used to ask `mode_of(state)` now asks this instead,
    /// because with both worlds in one list the mode no longer says which one a
    /// row belongs to. See `local_for`.
    pub origin: &'static str,
}

/// Everything a local profile allows. There are no roles without a server —
/// whoever can open the agent socket owns the machine and the data.
fn all_permissions() -> Vec<String> {
    [
        "view", "launch", "edit_profile", "edit_fingerprint", "edit_proxy",
        "reveal_secrets", "export_cookies", "create_profile", "delete_profile",
        "manage_access",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[tauri::command]
/// Folders, from both worlds.
///
/// Connected to a server this used to list the SERVER's projects and only
/// those, so a local profile had nowhere to be filed and a folder made for one
/// could not be made at all -- "why is a local folder not being created" is
/// exactly right, it was not, the + made a project on the server.
pub async fn projects(state: State<'_, AppState>) -> R<Vec<UiProject>> {
    let local: Vec<crate::agent::LocalProject> =
        crate::agent::call("projects.list", serde_json::json!({}))
            .await
            .unwrap_or_default();
    let mine: Vec<UiProject> = local
        .into_iter()
        .map(|p| UiProject {
            id: p.id,
            name: p.name,
            profile_count: p.profile_count,
            origin: "local",
        })
        .collect();

    if mode_of(&state) == "local" {
        return Ok(mine);
    }

    let remote: Vec<ProjectSummary> = state
        .call(reqwest::Method::GET, "/v1/projects", Body::None, true)
        .await?;
    let mut rows: Vec<UiProject> = remote
        .into_iter()
        .map(|p| UiProject {
            id: p.id.to_string(),
            name: p.name,
            profile_count: p.profile_count,
            origin: "team",
        })
        .collect();
    // The server's first, then this machine's: a team's folders are what
    // somebody connected to a team is usually looking for.
    rows.extend(mine);
    Ok(rows)
}

#[tauri::command]
/// `project_id` absent means every profile on this machine — the Profiles view.
/// This machine's own profiles, whatever the shell is connected to.
///
/// Split out of `profiles` so that team mode can show them beside the server's.
/// A project id is only ever a SERVER project, so it is not passed down here --
/// filtering local rows by a project they cannot be in would return nothing and
/// look like the local profiles had vanished again.
async fn local_profiles() -> R<Vec<UiProfile>> {
    let local: Vec<crate::agent::LocalProfile> =
        crate::agent::call("profiles.list", serde_json::json!({})).await?;
    Ok(local
        .into_iter()
        .map(|p| UiProfile {
            proxy: p.proxy.map(|x| UiProxy {
                display: format!("{}:{}", x.host, x.port),
                country: x.last_country,
                id: x.id,
                name: x.name,
                kind: x.kind,
            }),
            permissions: all_permissions(),
            lock: None,
            running: p.running,
            last_opened_at: p.last_opened_at,
            id: p.id,
            project_id: p.project_id,
            project_name: p.project_name,
            name: p.name,
            tags: p.tags,
            persona_id: p.persona_id,
            fp_seed: p.fp_seed,
            timezone: p.timezone,
            languages: p.languages,
            shared_with: 0,
            origin: "local",
        })
        .collect())
}

#[tauri::command]
pub async fn profiles(
    state: State<'_, AppState>,
    project_id: Option<String>,
    // Which world the chosen project belongs to. Absent for the flat list.
    origin: Option<String>,
) -> R<Vec<UiProfile>> {
    // A LOCAL folder chosen while connected to a server: its contents come from
    // the agent, and asking the server for them returns "not found" about a
    // project it has never heard of.
    if project_id.is_some() && origin.as_deref() == Some("local") {
        let local: Vec<crate::agent::LocalProfile> = crate::agent::call(
            "profiles.list",
            serde_json::json!({ "project_id": project_id }),
        )
        .await?;
        return Ok(local
            .into_iter()
            .map(|p| UiProfile {
                proxy: p.proxy.map(|x| UiProxy {
                    display: format!("{}:{}", x.host, x.port),
                    country: x.last_country,
                    id: x.id,
                    name: x.name,
                    kind: x.kind,
                }),
                permissions: all_permissions(),
                lock: None,
                running: p.running,
                last_opened_at: p.last_opened_at,
                id: p.id,
                project_id: p.project_id,
                project_name: p.project_name,
                name: p.name,
                tags: p.tags,
                persona_id: p.persona_id,
                fp_seed: p.fp_seed,
                timezone: p.timezone,
                languages: p.languages,
                // A profile on this machine is held by nobody: there is nowhere
                // for a share to have been recorded.
                shared_with: 0,
                origin: "local",
            })
            .collect());
    }

    if mode_of(&state) == "local" {
        let local: Vec<crate::agent::LocalProfile> = crate::agent::call(
            "profiles.list",
            match &project_id {
                Some(id) => serde_json::json!({ "project_id": id }),
                None => serde_json::json!({}),
            },
        )
        .await?;
        return Ok(local
            .into_iter()
            .map(|p| UiProfile {
                proxy: p.proxy.map(|x| UiProxy {
                    display: format!("{}:{}", x.host, x.port),
                    country: x.last_country,
                    id: x.id,
                    name: x.name,
                    kind: x.kind,
                }),
                permissions: all_permissions(),
                lock: None,
                running: p.running,
                last_opened_at: p.last_opened_at,
                id: p.id,
                project_id: p.project_id,
                project_name: p.project_name,
                name: p.name,
                tags: p.tags,
                persona_id: p.persona_id,
                fp_seed: p.fp_seed,
                timezone: p.timezone,
                languages: p.languages,
                // A profile on this machine is held by nobody: there is nowhere
                // for a share to have been recorded.
                shared_with: 0,
                origin: "local",
            })
            .collect());
    }

    // No project means every profile the caller may see — the same flat list
    // local mode shows, resolved server-side where the permissions live.
    let path = match &project_id {
        Some(id) => format!("/v1/projects/{id}/profiles"),
        None => "/v1/profiles".to_string(),
    };
    let remote: Vec<ProfileSummary> = state
        .call(reqwest::Method::GET, &path, Body::None, true)
        .await?;
    let mut rows: Vec<UiProfile> = remote
        .into_iter()
        .map(|p| UiProfile {
            id: p.id.to_string(),
            project_id: Some(p.project_id.to_string()),
            project_name: Some(p.project_name),
            // The listing does not carry them; the edit dialog fetches what it
            // needs when it opens.
            timezone: None,
            languages: None,
            name: p.name,
            tags: p.tags,
            persona_id: p.persona_id,
            fp_seed: 0,
            proxy: p.proxy.map(|x| UiProxy {
                id: x.id.to_string(),
                name: x.name,
                kind: x.kind,
                display: x.display,
                country: x.country,
            }),
            permissions: p.permissions.iter().map(|v| perm_name(v)).collect(),
            lock: p.lock.as_ref().map(|l| serde_json::json!({
                "user_id": l.user_id.to_string(),
                "user_email": l.user_email,
                "machine_name": l.machine_name,
                "acquired_at": l.acquired_at,
                "expires_at": l.expires_at,
            })),
            running: false,
            last_opened_at: None,
            shared_with: p.shared_with,
            origin: "team",
        })
        .collect();

    // And this machine's own, beside them.
    //
    // Only on the flat list. Asked for a particular project the answer is the
    // server's project and nothing else, because a local profile is in no
    // server project and putting it there would be a lie about where it lives.
    //
    // A failure here is deliberately not fatal: an agent that is not answering
    // should cost the local rows, not the whole list. Losing the server's
    // profiles because the local half stumbled would be a worse trade than the
    // one this function exists to make.
    if project_id.is_none() {
        match local_profiles().await {
            Ok(mut mine) => rows.append(&mut mine),
            Err(e) => tracing::warn!("local profiles unavailable, showing the server's only: {e:?}"),
        }
    }
    Ok(rows)
}

/// Serde already spells these the way the interface expects; going through it
/// keeps one definition rather than a second list that can drift.
fn perm_name(p: &fury_shared::rbac::Perm) -> String {
    serde_json::to_value(p)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

#[derive(Serialize)]
pub struct LockHeld {
    pub expires_at: String,
    pub restrictions: serde_json::Value,
    /// False while nothing renews the lock. The heartbeat belongs to the agent.
    pub renewed: bool,
}

/// What the server grants, in full. Only part of it crosses to the webview.
#[derive(Deserialize)]
struct LockGrant {
    lock_token: String,
    expires_at: String,
    restrictions: serde_json::Value,
    spec: fury_shared::api::LaunchSpec,
}

/// Open a profile.
///
/// Local mode goes straight to the agent. Team mode takes the lock, which is
/// also what hands back everything needed to launch, opens the proxy
/// credentials here — where the organisation key lives and nowhere else — and
/// passes the assembled profile down the socket.
///
/// The agent is never given the organisation key. It receives one profile, with
/// one proxy's credentials in the clear, exactly as it would in local mode. A
/// compromised agent is then a compromised profile rather than a compromised
/// team.
///
/// Worth being plain about, because the interface must not imply otherwise:
/// anyone with `launch` ends up able to recover that proxy password. The relay
/// runs on their machine and needs the plaintext to dial. `reveal_secrets` is a
/// real control against someone who may only *see* a profile, and not against
/// someone who may open it.
#[tauri::command]
pub async fn launch(
    state: State<'_, AppState>,
    profile_id: String,
    force: bool,
    origin: Option<String>,
) -> R<serde_json::Value> {
    if local_for(&state, origin.as_deref()) {
        let out: serde_json::Value =
            crate::agent::call("profile.launch", serde_json::json!({ "id": profile_id })).await?;
        return Ok(serde_json::json!({ "launched": true, "pid": out.get("pid") }));
    }

    let body = serde_json::json!({
        "machine_id": state.machine_id(),
        "machine_name": settings::machine_name(),
        "force": force,
    });
    let grant: LockGrant = state
        .call(
            reqwest::Method::POST,
            &format!("/v1/profiles/{profile_id}/lock"),
            Body::Json(body),
            true,
        )
        .await?;

    state
        .locks
        .lock()
        .unwrap()
        .insert(profile_id.clone(), grant.lock_token.clone());

    // From here a failure has to release the lock. Leaving one behind on a
    // profile that never opened is the thing locks are supposed to prevent,
    // arrived at from the wrong direction — a colleague waiting ninety seconds
    // for a browser that does not exist.
    let launched = launch_from_spec(&state, &grant).await;
    if launched.is_err() {
        let _: R<serde_json::Value> = state
            .call(
                reqwest::Method::POST,
                &format!("/v1/profiles/{profile_id}/unlock"),
                Body::Json(serde_json::json!({ "lock_token": grant.lock_token })),
                true,
            )
            .await;
        state.locks.lock().unwrap().remove(&profile_id);
    }
    let out = launched?;

    Ok(serde_json::json!({
        "launched": true,
        "pid": out.get("pid"),
        "expires_at": grant.expires_at,
        "restrictions": grant.restrictions,
    }))
}

/// Turn a launch spec into a running browser.
async fn launch_from_spec(state: &AppState, grant: &LockGrant) -> R<serde_json::Value> {
    let spec = &grant.spec;

    let org_key = state
        .org_key()
        .ok_or_else(|| {
            ApiErr::local(
                "This machine does not hold the organisation key yet. An owner or admin has to                  hand it over before anything here can be decrypted — until then you can see the                  team and open nothing.",
            )
        })?;

    let credentials = crypto::open_proxy_credentials(
        &org_key,
        &spec.proxy.credentials_enc,
        &spec.proxy.wrapped_dek,
    )
    .map_err(|e| ApiErr::local(format!("Could not open this proxy's credentials: {e}")))?;

    let seed = fury_shared::persona::seed::from_hex(&spec.fp_seed).ok_or_else(|| {
        ApiErr::local("The server sent a fingerprint seed this version cannot read.")
    })?;

    // Shaped exactly like a local profile, because from the agent's side that
    // is what it is: one profile to run, with one proxy to run it through.
    let profile = serde_json::json!({
        "id": spec.profile_id.to_string(),
        "project_id": null,
        "name": spec.name,
        "notes": "",
        "tags": [],
        "persona_id": spec.persona_id,
        "fp_seed": fury_shared::persona::seed::to_i64(seed),
        "proxy": {
            "id": spec.proxy.id.to_string(),
            "name": "",
            "kind": spec.proxy.kind,
            "host": spec.proxy.host,
            "port": spec.proxy.port,
            "username": credentials.username,
            "password": credentials.password,
            "last_country": null,
            "last_ip": null,
            "rotate_url": null,
            "checker_url": null,
        },
        "timezone": spec.timezone,
        "languages": spec.languages,
        "start_urls": spec.start_urls,
        "last_opened_at": null,
    });

    let server = state.server_url()?;
    let token = state.session.token().ok_or_else(unauthenticated)?;

    // The key this profile's bundle is wrapped under. Derived from the
    // organisation key and this profile's id, so the agent can open the one
    // bundle it was asked for and no other — and so a colleague on a different
    // machine can open the same bundle, which is what a team profile is for.
    let profile_key = crypto::profile_key(&org_key, &spec.profile_id.to_string());

    Ok(crate::agent::call(
        "profile.launch",
        serde_json::json!({
            "id": spec.profile_id.to_string(),
            "profile": profile,
            "server": { "url": server, "token": token },
            "lock_token": grant.lock_token,
            "profile_key": profile_key.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        }),
    )
    .await?)
}

#[tauri::command]
pub async fn stop(
    state: State<'_, AppState>,
    profile_id: String,
    origin: Option<String>,
) -> R<serde_json::Value> {
    if local_for(&state, origin.as_deref()) {
        return Ok(crate::agent::call("profile.stop", serde_json::json!({ "id": profile_id })).await?);
    }

    // The token proves this machine is the holder. Without one there is nothing
    // to release: a lock this process never took belongs to somebody else, and
    // possibly to this same person on another machine with a browser open.
    let token = state.locks.lock().unwrap().get(&profile_id).cloned();
    let Some(token) = token else {
        return Ok(serde_json::json!({ "stopped": false }));
    };

    let _: serde_json::Value = state
        .call(
            reqwest::Method::POST,
            &format!("/v1/profiles/{profile_id}/unlock"),
            Body::Json(serde_json::json!({ "lock_token": token })),
            true,
        )
        .await?;
    state.locks.lock().unwrap().remove(&profile_id);
    Ok(serde_json::json!({ "stopped": true }))
}

// ---------------------------------------------------------------------------
// the team
// ---------------------------------------------------------------------------

/// Whether the organisation key is remembered between launches.
#[tauri::command]
pub async fn set_remember_org_key(state: State<'_, AppState>, remember: bool) -> R<Shell> {
    {
        let mut s = state.settings.lock().unwrap();
        s.remember_org_key = remember;
        s.save(&state.config_dir)
            .map_err(|e| ApiErr::local(format!("Could not save settings: {e}")))?;
    }
    if !remember {
        // Turned off, the copy on disk goes now rather than at the next sign
        // out — otherwise "do not remember it" leaves it remembered.
        state.session.forget_org_key();
    } else if let Some(key) = *state.org_key.lock().unwrap() {
        state.session.store_org_key(&key);
    }
    shell_state(state).await
}

/// Who did what. Owners and admins only — the server decides that, not this.
#[tauri::command]
pub async fn audit(state: State<'_, AppState>, before: Option<i64>) -> R<serde_json::Value> {
    let path = match before {
        Some(id) => format!("/v1/audit?limit=200&before={id}"),
        None => "/v1/audit?limit=200".to_string(),
    };
    state.call(reqwest::Method::GET, &path, Body::None, true).await
}

#[tauri::command]
pub async fn org_members(state: State<'_, AppState>) -> R<serde_json::Value> {
    state.call(reqwest::Method::GET, "/v1/org/members", Body::None, true).await
}

#[tauri::command]
pub async fn invite(
    state: State<'_, AppState>,
    email: String,
    role: String,
) -> R<serde_json::Value> {
    state
        .call(
            reqwest::Method::POST,
            "/v1/org/invitations",
            Body::Json(serde_json::json!({ "email": email, "role": role })),
            true,
        )
        .await
}

/// Let a member in.
///
/// The half that cannot happen on the server: their copy of the organisation
/// key is sealed to their published public key, here, by someone who already
/// holds it. The server stores the result and can do nothing with it.
#[tauri::command]
pub async fn hand_over_key(
    state: State<'_, AppState>,
    user_id: String,
    public_key: String,
) -> R<serde_json::Value> {
    let org_key = state.org_key().ok_or_else(|| {
        ApiErr::coded("err.noOrgKeyGive", 
            "You do not hold the organisation key on this machine, so you cannot hand it to \
             anyone. Sign in again, or ask an owner.",
        )
    })?;

    let wrapped = crypto::wrap_for_member(&org_key, &public_key)
        .map_err(|e| ApiErr::local(format!("Could not seal the key for them: {e}")))?;

    state
        .call(
            reqwest::Method::POST,
            &format!("/v1/org/members/{user_id}/key"),
            Body::Json(serde_json::json!({ "wrapped_ork": wrapped })),
            true,
        )
        .await
}

/// Remove a member, and replace the organisation key so it means something.
///
/// Every step that needs the key happens here. The server hands over public
/// keys and wrapped data keys, this process opens and re-wraps them, and the
/// results go back in one request that either lands whole or not at all.
///
/// What this cannot do, and the interface must not pretend otherwise: reach
/// what the removed person already downloaded. Bundles they opened are on their
/// machine. Rotation stops them opening anything from now on, which is the part
/// that is actually achievable.
#[tauri::command]
pub async fn remove_member(
    state: State<'_, AppState>,
    user_id: Option<String>,
) -> R<serde_json::Value> {
    let old = state.org_key().ok_or_else(|| {
        ApiErr::coded(
            "err.noOrgKeyGive",
            "You do not hold the organisation key on this machine, so you cannot rotate it.",
        )
    })?;

    let material: serde_json::Value = state
        .call(reqwest::Method::GET, "/v1/org/rotation", Body::None, true)
        .await?;

    let generation = material.get("generation").and_then(|v| v.as_i64()).unwrap_or(0);
    let new_key = fury_shared::keys::new_org_key();

    // Sealed to everyone who stays. A member left out of this list is a member
    // locked out of the organisation, and nothing later can repair it except
    // another rotation.
    let mut members = Vec::new();
    for m in material.get("members").and_then(|v| v.as_array()).into_iter().flatten() {
        let id = m.get("user_id").and_then(|v| v.as_str()).unwrap_or_default();
        if Some(id.to_string()) == user_id {
            continue;
        }
        let pk = m.get("public_key").and_then(|v| v.as_str()).unwrap_or_default();
        members.push(serde_json::json!({
            "user_id": id,
            "wrapped_ork": crypto::wrap_for_member(&new_key, pk)
                .map_err(|e| ApiErr::local(format!("Could not seal the new key for {id}: {e}")))?,
        }));
    }

    // Every proxy's data key moves to the new organisation key. The credentials
    // themselves are untouched — they are sealed under the data key, which does
    // not change, so nothing has to be decrypted and re-encrypted here.
    let mut proxies = Vec::new();
    for x in material.get("proxies").and_then(|v| v.as_array()).into_iter().flatten() {
        let id = x.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        let Some(wrapped) = x.get("wrapped_dek").and_then(|v| v.as_str()) else {
            continue;
        };
        let rewrapped = crypto::rewrap_data_key(&old, &new_key, wrapped)
            .map_err(|e| ApiErr::local(format!("Could not re-wrap the key for proxy {id}: {e}")))?;
        proxies.push(serde_json::json!({ "id": id, "wrapped_dek": rewrapped }));
    }

    let out: serde_json::Value = state
        .call(
            reqwest::Method::POST,
            "/v1/org/rotation",
            Body::Json(serde_json::json!({
                "remove_user_id": user_id,
                "members": members,
                "proxies": proxies,
                "generation": generation,
            })),
            true,
        )
        .await?;

    // Only after the server accepted it. Adopting the new key on a rotation
    // that was refused would leave this machine unable to open anything.
    *state.org_key.lock().unwrap() = Some(new_key);
    if state.settings.lock().unwrap().remember_org_key {
        state.session.store_org_key(&new_key);
    }
    if let Some(g) = out.get("generation").and_then(|v| v.as_i64()) {
        let mut seen = state.ork_generation.lock().unwrap();
        *seen = (*seen).max(g as i32);
        let mut s = state.settings.lock().unwrap();
        s.ork_generation = *seen;
        let _ = s.save(&state.config_dir);
    }
    Ok(out)
}

#[tauri::command]
pub async fn grants(state: State<'_, AppState>, project_id: String) -> R<serde_json::Value> {
    state
        .call(
            reqwest::Method::GET,
            &format!("/v1/projects/{project_id}/grants"),
            Body::None,
            true,
        )
        .await
}

#[tauri::command]
pub async fn grant_access(
    state: State<'_, AppState>,
    project_id: String,
    user_id: String,
    permissions: Vec<String>,
) -> R<serde_json::Value> {
    state
        .call(
            reqwest::Method::POST,
            &format!("/v1/projects/{project_id}/grants"),
            Body::Json(serde_json::json!({
                "user_id": user_id,
                "permissions": permissions,
                "expires_at": serde_json::Value::Null,
            })),
            true,
        )
        .await
}

#[tauri::command]
pub async fn revoke_access(
    state: State<'_, AppState>,
    project_id: String,
    user_id: String,
) -> R<serde_json::Value> {
    state
        .call(
            reqwest::Method::DELETE,
            &format!("/v1/projects/{project_id}/grants/{user_id}"),
            Body::None,
            true,
        )
        .await
}

// ---------------------------------------------------------------------------
// local-only editing
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn personas() -> R<serde_json::Value> {
    Ok(crate::agent::call("personas.list", serde_json::json!({})).await?)
}

/// What a profile will claim, before it exists. See the agent for why this is
/// shown rather than left to be discovered after the fact.
#[tauri::command]
pub async fn preview(spec: serde_json::Value) -> R<serde_json::Value> {
    Ok(crate::agent::call("profile.preview", spec).await?)
}

/// Logins stored beside a profile: username, password, two-factor seed.
///
/// Local only, and that is a decision rather than an omission. The server's
/// `credentials` table takes a blob sealed with the organisation key, and
/// sealing it correctly is the difference between a team feature and a way to
/// hand a server everybody's passwords. Doing the local half first means an
/// operator working alone — which is most of them — gets the feature now, and
/// the team half arrives without changing what it looks like.
#[tauri::command]
pub async fn credentials(
    state: State<'_, AppState>,
    profile_id: String,
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(
            crate::agent::call("credentials.list", serde_json::json!({ "profile_id": profile_id }))
                .await?,
        );
    }

    // Team mode. The server hands back a blob and a wrapped key; both are
    // opened here, in the application process, and never in the webview.
    let org_key = state.org_key().ok_or_else(|| {
        ApiErr::coded(
            "err.noOrgKeyOpen",
            "This machine does not hold the organisation key, so it cannot read the logins \
             stored for this profile. An owner or admin has to hand the key over first.",
        )
    })?;

    let rows: Vec<serde_json::Value> = state
        .call(
            reqwest::Method::GET,
            &format!("/v1/profiles/{profile_id}/credentials"),
            Body::None,
            true,
        )
        .await?;

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let get = |k: &str| row.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        // A login this machine cannot open is shown as one it cannot open,
        // rather than dropped. Dropping it would read as "somebody deleted the
        // password", and the real answer — the organisation key was rotated
        // and this copy is stale — is one the operator can act on.
        let payload = crypto::open_credential(&org_key, &get("payload_enc"), &get("wrapped_dek"))
            .unwrap_or(serde_json::Value::Null);
        out.push(serde_json::json!({
            "id": get("id"),
            "profile_id": profile_id,
            "label": get("label"),
            "site": get("site"),
            "username": payload.get("username").cloned().unwrap_or(serde_json::Value::Null),
            "password": payload.get("password").cloned().unwrap_or(serde_json::Value::Null),
            "totp": payload.get("totp").cloned().unwrap_or(serde_json::Value::Null),
            "notes": payload.get("notes").and_then(|v| v.as_str()).unwrap_or_default(),
            "unreadable": payload.is_null(),
        }));
    }
    Ok(serde_json::Value::Array(out))
}

#[tauri::command]
pub async fn save_credential(
    state: State<'_, AppState>,
    credential: serde_json::Value,
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(crate::agent::call("credentials.upsert", credential).await?);
    }

    let org_key = state.org_key().ok_or_else(|| {
        ApiErr::coded(
            "err.noOrgKeySeal",
            "This machine does not hold the organisation key yet, so it cannot seal a login. \
             An owner or admin has to hand the key over first.",
        )
    })?;

    let field = |k: &str| credential.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let profile_id = field("profile_id").unwrap_or_default();
    let id = field("id").unwrap_or_default();

    // Normalised BEFORE sealing, and refused here if it is not a seed. The
    // server cannot check it — the whole point is that the server cannot read
    // it — so this is the only place a mistyped secret can be caught in front
    // of the person who typed it rather than at the moment they need to log in.
    let totp = match field("totp").map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        Some(raw) => Some(
            fury_shared::totp::Totp::parse(&raw)
                .map_err(|e| ApiErr::local(e.to_string()))?
                .to_uri(),
        ),
        None => None,
    };

    let payload = serde_json::json!({
        "username": field("username").filter(|v| !v.is_empty()),
        "password": field("password").filter(|v| !v.is_empty()),
        "totp": totp,
        "notes": field("notes").unwrap_or_default(),
    });
    let (payload_enc, wrapped_dek) = crypto::seal_credential(&org_key, &payload)
        .map_err(|e| ApiErr::local(format!("Could not seal the login: {e}")))?;
    let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();

    let body = serde_json::json!({
        "label": field("label").unwrap_or_default(),
        "site": field("site").unwrap_or_default(),
        "payload_enc": hex(&payload_enc),
        "wrapped_dek": hex(&wrapped_dek),
    });

    if id.is_empty() {
        state
            .call(
                reqwest::Method::POST,
                &format!("/v1/profiles/{profile_id}/credentials"),
                Body::Json(body),
                true,
            )
            .await
    } else {
        state
            .call(
                reqwest::Method::PATCH,
                &format!("/v1/profiles/{profile_id}/credentials/{id}"),
                Body::Json(body),
                true,
            )
            .await
    }
}

#[tauri::command]
pub async fn delete_credential(
    state: State<'_, AppState>,
    id: String,
    profile_id: String,
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(crate::agent::call("credentials.delete", serde_json::json!({ "id": id })).await?);
    }
    state
        .call(
            reqwest::Method::DELETE,
            &format!("/v1/profiles/{profile_id}/credentials/{id}"),
            Body::None,
            true,
        )
        .await
}

/// Six digits and how long they last.
///
/// The seed itself never comes back here and never reaches the webview. A
/// webview is a browser engine: anything handed to it is in a heap that a page
/// bug, an extension or a crash dump can reach, and a two-factor seed is the
/// one value in this product that is worth more than the password beside it.
/// The agent holds it and returns the code.
#[tauri::command]
pub async fn totp_code(
    state: State<'_, AppState>,
    profile_id: String,
    id: String,
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(crate::agent::call(
            "credentials.code",
            serde_json::json!({ "profile_id": profile_id, "id": id }),
        )
        .await?);
    }

    // In team mode the seed lives on the server sealed, so it is fetched,
    // opened here, used, and dropped — it is never written to this machine and
    // never handed to the webview. The code is computed in this process, so
    // what crosses into the interface is six digits and a countdown.
    let org_key = state.org_key().ok_or_else(|| {
        ApiErr::coded(
            "err.noOrgKeyOpen",
            "This machine does not hold the organisation key, so it cannot read the \
             two-factor seed.",
        )
    })?;

    let rows: Vec<serde_json::Value> = state
        .call(
            reqwest::Method::GET,
            &format!("/v1/profiles/{profile_id}/credentials"),
            Body::None,
            true,
        )
        .await?;
    let row = rows
        .iter()
        .find(|r| r.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
        .ok_or_else(|| ApiErr::local("No such login."))?;

    let get = |k: &str| row.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let payload = crypto::open_credential(&org_key, &get("payload_enc"), &get("wrapped_dek"))
        .map_err(|_| {
            ApiErr::coded(
                "err.staleOrgKey",
                "This login was sealed with a different organisation key. The key was \
                 probably rotated after this machine received its copy.",
            )
        })?;
    let seed = payload
        .get("totp")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| ApiErr::local("This login has no two-factor seed."))?;

    let totp = fury_shared::totp::Totp::parse(seed).map_err(|e| ApiErr::local(e.to_string()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(serde_json::json!({
        "code": totp.code_at(now),
        "seconds_remaining": totp.seconds_remaining(now),
        "next": totp.code_at(now + totp.seconds_remaining(now)),
    }))
}

#[tauri::command]
pub async fn proxies(state: State<'_, AppState>) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(crate::agent::call("proxies.list", serde_json::json!({})).await?);
    }
    // Reshaped into what the local store returns, so the interface has one
    // proxy and not two. The password is absent rather than empty: the server
    // never sends it, and a blank string would read as "no password set".
    let remote: Vec<fury_shared::api::ProxySummary> = state
        .call(reqwest::Method::GET, "/v1/proxies", Body::None, true)
        .await?;
    Ok(serde_json::Value::Array(
        remote
            .into_iter()
            .map(|p| {
                let (host, port) = p
                    .display
                    .rsplit_once(':')
                    .map(|(h, port)| (h.to_string(), port.parse::<u16>().unwrap_or(0)))
                    .unwrap_or((p.display.clone(), 0));
                serde_json::json!({
                    "id": p.id.to_string(),
                    "name": p.name,
                    "kind": p.kind,
                    "host": host,
                    "port": port,
                    "username": serde_json::Value::Null,
                    "password": serde_json::Value::Null,
                    "last_country": p.country,
                    "last_ip": serde_json::Value::Null,
                    "rotate_url": serde_json::Value::Null,
                    "checker_url": serde_json::Value::Null,
                    "shared_with_profiles": p.shared_with_profiles,
                })
            })
            .collect(),
    ))
}

/// Save a proxy.
///
/// In team mode the credentials are sealed here, before anything leaves this
/// process: a per-proxy data key seals `{username, password}`, and only that
/// key is wrapped under the organisation key. The server receives two blobs it
/// cannot read.
///
/// Two layers rather than one because of what happens when somebody leaves.
/// Removing a member means rotating the organisation key, and with this shape
/// that is re-wrapping one small key per proxy — not re-encrypting every secret
/// the team owns.
#[tauri::command]
pub async fn save_proxy(
    state: State<'_, AppState>,
    proxy: serde_json::Value,
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(crate::agent::call("proxies.upsert", proxy).await?);
    }

    let id = proxy.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();

    let org_key = state.org_key().ok_or_else(|| {
        ApiErr::coded("err.noOrgKeySeal", 
            "This machine does not hold the organisation key yet, so it cannot seal a proxy's \
             credentials. An owner or admin has to hand the key over first.",
        )
    })?;

    let field = |k: &str| proxy.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let credentials = fury_shared::api::ProxyCredentials {
        username: field("username").filter(|v| !v.is_empty()),
        password: field("password").filter(|v| !v.is_empty()),
    };
    let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut body = serde_json::json!({
        "name": field("name").unwrap_or_default(),
        "kind": field("kind").unwrap_or_else(|| "socks5".into()),
        "host": field("host").unwrap_or_default(),
        "port": proxy.get("port").and_then(|v| v.as_i64()).unwrap_or(0),
    });

    // Editing without touching the credentials is the ordinary case — a
    // renamed exit, a corrected port — and the form cannot show a password it
    // never received. An empty password on an edit therefore means "leave it",
    // not "clear it", which is the only reading that does not silently break a
    // working proxy.
    let touching_credentials = id.is_empty()
        || credentials.username.is_some()
        || credentials.password.is_some();
    if touching_credentials {
        let (credentials_enc, wrapped_dek) = crypto::seal_proxy_credentials(&org_key, &credentials)
            .map_err(|e| ApiErr::local(format!("Could not seal the credentials: {e}")))?;
        body["credentials_enc"] = serde_json::json!(hex(&credentials_enc));
        body["wrapped_dek"] = serde_json::json!(hex(&wrapped_dek));
    }

    if id.is_empty() {
        body["project_id"] = serde_json::Value::Null;
        state.call(reqwest::Method::POST, "/v1/proxies", Body::Json(body), true).await
    } else {
        state
            .call(reqwest::Method::PATCH, &format!("/v1/proxies/{id}"), Body::Json(body), true)
            .await
    }
}

#[tauri::command]
pub async fn check_proxy(
    state: State<'_, AppState>,
    url: String,
    checker_url: Option<String>,
    // proxy_id is set when checking a proxy already stored on a server: the
    // interface has no password to build a URL from, so the credentials are
    // fetched and opened here instead.
    proxy_id: Option<String>,
) -> R<serde_json::Value> {
    let stored_id = proxy_id.clone();
    let url = match proxy_id.filter(|_| mode_of(&state) != "local") {
        None => url,
        Some(id) => {
            let org_key = state.org_key().ok_or_else(|| {
                ApiErr::coded(
                    "err.noOrgKeySeal",
                    "This machine does not hold the organisation key, so it cannot open this \
                     proxy's credentials to check it.",
                )
            })?;

            let sealed: serde_json::Value = state
                .call(
                    reqwest::Method::GET,
                    &format!("/v1/proxies/{id}/credentials"),
                    Body::None,
                    true,
                )
                .await?;

            let hex = |k: &str| -> R<Vec<u8>> {
                let v = sealed.get(k).and_then(|v| v.as_str()).unwrap_or_default();
                (0..v.len())
                    .step_by(2)
                    .map(|i| {
                        u8::from_str_radix(v.get(i..i + 2).unwrap_or(""), 16)
                            .map_err(|_| ApiErr::local(format!("{k} is not hex")))
                    })
                    .collect()
            };
            let creds =
                crypto::open_proxy_credentials(&org_key, &hex("credentials_enc")?, &hex("wrapped_dek")?)
                    .map_err(|e| {
                        ApiErr::local(format!("Could not open this proxy's credentials: {e}"))
                    })?;

            let s = |k: &str| sealed.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let port = sealed.get("port").and_then(|v| v.as_i64()).unwrap_or(0);
            match (creds.username, creds.password) {
                (Some(u), Some(pw)) => format!("{}://{u}:{pw}@{}:{port}", s("kind"), s("host")),
                _ => format!("{}://{}:{port}", s("kind"), s("host")),
            }
        }
    };

    // The check itself always runs on this machine, in both modes: it is the
    // machine whose exit the answer describes.
    Ok(crate::agent::call(
        "proxies.check",
        // The id, when there is one, so the agent can remember what it learned
        // and a launch that follows this exit does not ask again.
        serde_json::json!({ "url": url, "checker_url": checker_url, "id": stored_id }),
    )
    .await?)
}

#[tauri::command]
pub async fn rotate_proxy(state: State<'_, AppState>, id: String) -> R<serde_json::Value> {
    if mode_of(&state) != "local" {
        return Err(ApiErr::coded(
            "err.proxyCheckNotBuilt",
            "Checking a proxy on a team server is not built yet — its credentials are sealed, and \
             the check would run without them.",
        ));
    }
    Ok(crate::agent::call("proxies.rotate", serde_json::json!({ "id": id })).await?)
}

#[tauri::command]
pub async fn delete_proxy(state: State<'_, AppState>, id: String) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(crate::agent::call("proxies.delete", serde_json::json!({ "id": id })).await?);
    }
    state
        .call(reqwest::Method::DELETE, &format!("/v1/proxies/{id}"), Body::None, true)
        .await
}

#[tauri::command]
pub async fn save_profile(
    state: State<'_, AppState>,
    profile: serde_json::Value,
    origin: Option<String>,
) -> R<serde_json::Value> {
    if local_for(&state, origin.as_deref()) {
        return Ok(crate::agent::call("profiles.upsert", profile).await?);
    }

    let s = |k: &str| profile.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let list = |k: &str| -> Vec<String> {
        profile
            .get(k)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let proxy_id = profile
        .get("proxy")
        .and_then(|p| p.get("id"))
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .ok_or_else(|| {
            ApiErr::coded("err.teamProfileNeedsProxy", 
                "A team profile needs a proxy. Everything the browser does goes through one.",
            )
        })?;

    let body = serde_json::json!({
        "name": s("name"),
        "notes": s("notes"),
        "tags": list("tags"),
        "timezone": s("timezone"),
        "languages": list("languages"),
        "proxy_id": proxy_id,
        "start_urls": list("start_urls"),
    });

    let id = s("id");
    if id.is_empty() {
        // New. The seed is generated here, once, and never again — see the
        // comment on the server's create endpoint for why it cannot move.
        let seed: u64 = {
            use rand::Rng;
            rand::thread_rng().gen()
        };
        let project_id = s("project_id");
        if project_id.is_empty() {
            return Err(ApiErr::coded("err.teamProfileNeedsProject", 
                "A team profile has to live in a project — that is what carries access to it.",
            ));
        }
        let mut body = body;
        body["persona_id"] = serde_json::json!(s("persona_id"));
        body["fp_seed"] = serde_json::json!(fury_shared::persona::seed::to_hex(seed));
        state
            .call(
                reqwest::Method::POST,
                &format!("/v1/projects/{project_id}/profiles"),
                Body::Json(body),
                true,
            )
            .await
    } else {
        state
            .call(
                reqwest::Method::PATCH,
                &format!("/v1/profiles/{id}"),
                Body::Json(body),
                true,
            )
            .await
    }
}

#[tauri::command]
pub async fn delete_profile(
    state: State<'_, AppState>,
    id: String,
    origin: Option<String>,
) -> R<serde_json::Value> {
    if local_for(&state, origin.as_deref()) {
        return Ok(crate::agent::call("profiles.delete", serde_json::json!({ "id": id })).await?);
    }
    state
        .call(reqwest::Method::DELETE, &format!("/v1/profiles/{id}"), Body::None, true)
        .await
}

#[tauri::command]
pub async fn export_project(
    id: String,
    path: String,
    passphrase: String,
    with_data: bool,
) -> R<serde_json::Value> {
    Ok(crate::agent::call(
        "projects.export",
        serde_json::json!({ "id": id, "path": path, "passphrase": passphrase, "with_data": with_data }),
    )
    .await?)
}

#[tauri::command]
pub async fn import_project(path: String, passphrase: String) -> R<serde_json::Value> {
    Ok(crate::agent::call(
        "projects.import",
        serde_json::json!({ "path": path, "passphrase": passphrase }),
    )
    .await?)
}

#[tauri::command]
/// Everything deleted and not yet gone, from both worlds.
///
/// It used to show one, chosen by the shell's mode, and I wrote that down as a
/// deliberate limit -- restoring is a different call in each. The limit was
/// wrong and produced the obvious thing: delete a local profile while connected
/// to a server and it goes into a bin you cannot open. Three profiles, deleted
/// and reported missing, sitting in the store the whole time.
///
/// Both, then, tagged with where they came from, and restore and purge follow
/// the tag.
pub async fn trash(state: State<'_, AppState>) -> R<Vec<UiProfile>> {
    let is_local = mode_of(&state) == "local";
    let mut local: Vec<crate::agent::LocalProfile> =
        crate::agent::call("profiles.trash", serde_json::json!({}))
            .await
            .unwrap_or_default();
    let mut from_server: Vec<crate::agent::LocalProfile> = Vec::new();
    if !is_local {
        // A server that stumbles costs the server's rows, not this machine's.
        from_server = state
            .call(reqwest::Method::GET, "/v1/profiles/trash", Body::None, true)
            .await
            .unwrap_or_default();
    }
    let local_count = local.len();
    local.append(&mut from_server);
    Ok(local
        .into_iter()
        .enumerate()
        .map(|(i, p)| UiProfile {
            proxy: p.proxy.map(|x| UiProxy {
                display: format!("{}:{}", x.host, x.port),
                country: x.last_country,
                id: x.id,
                name: x.name,
                kind: x.kind,
            }),
            permissions: all_permissions(),
            lock: None,
            running: false,
            // Carries the deletion time, not the last launch: in the trash the
            // question is when it went, not when it last ran.
            last_opened_at: p.last_opened_at,
            id: p.id,
            project_id: p.project_id,
            project_name: p.project_name,
            name: p.name,
            tags: p.tags,
            persona_id: p.persona_id,
            fp_seed: p.fp_seed,
            timezone: p.timezone,
            languages: p.languages,
            shared_with: 0,
            // The local rows were appended first, so the boundary is an index
            // rather than a flag on the row -- the agent and the server return
            // the same shape and neither says which it is.
            origin: if i < local_count { "local" } else { "team" },
        })
        .collect())
}

/// Put profiles into a project, or take them out of every project.
#[tauri::command]
pub async fn move_profiles(
    state: State<'_, AppState>,
    ids: Vec<String>,
    project_id: Option<String>,
    origin: Option<String>,
) -> R<serde_json::Value> {
    // Routed by the row, like every other action on one. Without this, moving a
    // LOCAL profile while connected to a server sent its id to the server,
    // which had never heard of it and said so:
    //
    //     Not found, or you no longer have access.
    //
    // — which reads as the profile having been taken away rather than as a
    // request sent to the wrong machine.
    if !local_for(&state, origin.as_deref()) {
        // On a server a profile always belongs to a project, because the
        // project is what carries access to it. "Out of every project" is a
        // local idea and there is nothing sensible for it to mean here.
        let Some(project_id) = project_id else {
            return Err(ApiErr::coded(
                "err.teamProfileNeedsProject",
                "A team profile has to live in a project — that is what carries access to it.",
            ));
        };
        return state
            .call(
                reqwest::Method::POST,
                "/v1/profiles/move",
                Body::Json(serde_json::json!({ "ids": ids, "project_id": project_id })),
                true,
            )
            .await;
    }
    Ok(crate::agent::call(
        "profiles.move",
        serde_json::json!({ "ids": ids, "project_id": project_id }),
    )
    .await?)
}

#[tauri::command]
pub async fn restore_profile(
    state: State<'_, AppState>,
    id: String,
    origin: Option<String>,
) -> R<serde_json::Value> {
    if local_for(&state, origin.as_deref()) {
        return Ok(crate::agent::call("profiles.restore", serde_json::json!({ "id": id })).await?);
    }
    state
        .call(reqwest::Method::POST, &format!("/v1/profiles/{id}/restore"), Body::None, true)
        .await
}

#[tauri::command]
pub async fn purge_profile(
    state: State<'_, AppState>,
    id: String,
    origin: Option<String>,
) -> R<serde_json::Value> {
    if local_for(&state, origin.as_deref()) {
        return Ok(crate::agent::call("profiles.purge", serde_json::json!({ "id": id })).await?);
    }
    state
        .call(reqwest::Method::DELETE, &format!("/v1/profiles/{id}/purge"), Body::None, true)
        .await
}

#[tauri::command]
pub async fn rename_project(
    state: State<'_, AppState>,
    id: String,
    name: String,
    origin: Option<String>,
) -> R<serde_json::Value> {
    if local_for(&state, origin.as_deref()) {
        return Ok(
            crate::agent::call("projects.rename", serde_json::json!({ "id": id, "name": name }))
                .await?,
        );
    }
    state
        .call(
            reqwest::Method::PATCH,
            &format!("/v1/projects/{id}"),
            Body::Json(serde_json::json!({ "name": name })),
            true,
        )
        .await
}

#[tauri::command]
pub async fn delete_project(
    state: State<'_, AppState>,
    id: String,
    origin: Option<String>,
) -> R<serde_json::Value> {
    if local_for(&state, origin.as_deref()) {
        return Ok(crate::agent::call("projects.delete", serde_json::json!({ "id": id })).await?);
    }
    state
        .call(reqwest::Method::DELETE, &format!("/v1/projects/{id}"), Body::None, true)
        .await
}

#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>,
    name: String,
    origin: Option<String>,
) -> R<serde_json::Value> {
    if local_for(&state, origin.as_deref()) {
        return Ok(crate::agent::call("projects.create", serde_json::json!({ "name": name })).await?);
    }
    state
        .call(
            reqwest::Method::POST,
            "/v1/projects",
            Body::Json(serde_json::json!({ "name": name })),
            true,
        )
        .await
}

// ---------------------------------------------------------------------------
// working in bulk
// ---------------------------------------------------------------------------
//
// All three of these delegate to the single-item command beside them rather
// than growing a second path. On a team server that is not a tidiness
// preference: `save_profile` is where a profile's project and seed are decided
// and `save_proxy` is where credentials get sealed under the organisation key,
// and a bulk path that reimplemented either would be the one that quietly
// stopped sealing.
//
// The cost is that these are N round trips rather than one request, so partial
// failure is the normal outcome and every one of them reports which items
// failed and why — a count alone would leave someone guessing which of two
// hundred to redo.

/// Make many profiles from one template.
#[tauri::command]
pub async fn create_profiles(
    state: State<'_, AppState>,
    count: u32,
    name_pattern: String,
    template: serde_json::Value,
) -> R<serde_json::Value> {
    if !(1..=500).contains(&count) {
        return Err(ApiErr::local(
            "Between 1 and 500. Five hundred profiles is already more than a person can \
             supervise, and a typo in that field should not fill the database."
                .to_string(),
        ));
    }

    // The agent does the whole loop in one call when the profiles are local:
    // one round trip over a socket beats five hundred.
    if mode_of(&state) == "local" {
        return Ok(crate::agent::call(
            "profiles.createMany",
            serde_json::json!({
                "count": count,
                "name_pattern": name_pattern,
                "template": template,
            }),
        )
        .await?);
    }

    let pattern = naming(&name_pattern, template.get("name").and_then(|v| v.as_str()));
    // Absent means spread them across the catalogue by how common each machine
    // is — see `catalogue::pick_weighted`. Picked here rather than on the
    // server because the server has no reason to hold that policy.
    let pick = template
        .get("persona_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .is_empty();

    let mut created = Vec::new();
    let mut failed = Vec::new();
    for i in 1..=count {
        let mut one = template.clone();
        one["id"] = serde_json::json!("");
        one["name"] = serde_json::json!(pattern.replace("{n}", &i.to_string()));
        if pick {
            one["persona_id"] =
                serde_json::json!(fury_shared::catalogue::pick_weighted(rand::random()).id);
        }
        // No origin: bulk creation makes NEW profiles, and a new profile is
        // born in whichever world the shell is connected to. Only an existing
        // row carries an origin, because only an existing row already lives
        // somewhere.
        match save_profile(state.clone(), one, None).await {
            Ok(v) => created.push(v),
            Err(e) => failed.push(serde_json::json!({ "n": i, "error": e.message })),
        }
    }
    Ok(serde_json::json!({ "created": created, "failed": failed }))
}

/// Copy a profile's setup, never its identity.
///
/// One call in both modes, and in both the copy is made where the profile
/// actually lives. That is not symmetry for its own sake: the team listing
/// carries a name, tags, persona and proxy and NOT the timezone, languages,
/// notes or start URLs, so a copy assembled on this side would be silently
/// missing four fields somebody set on purpose. The agent and the server each
/// read their own row and generate a fresh seed per copy.
#[tauri::command]
pub async fn clone_profile(
    state: State<'_, AppState>,
    id: String,
    count: u32,
    name: Option<String>,
    origin: Option<String>,
) -> R<serde_json::Value> {
    if !(1..=500).contains(&count) {
        return Err(ApiErr::local("Between 1 and 500.".to_string()));
    }
    let name = name.filter(|n| !n.trim().is_empty());

    if local_for(&state, origin.as_deref()) {
        let mut params = serde_json::json!({ "id": id, "count": count });
        if let Some(name) = name {
            params["name"] = serde_json::json!(name);
        }
        return Ok(crate::agent::call("profiles.clone", params).await?);
    }

    state
        .call(
            reqwest::Method::POST,
            &format!("/v1/profiles/{id}/clone"),
            Body::Json(serde_json::json!({ "count": count, "name": name })),
            true,
        )
        .await
}

/// A pasted supplier block.
#[tauri::command]
pub async fn import_proxies(
    state: State<'_, AppState>,
    text: String,
    name_prefix: String,
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(crate::agent::call(
            "proxies.importMany",
            serde_json::json!({ "text": text, "name_prefix": name_prefix }),
        )
        .await?);
    }

    // Parsed here, with the same parser the agent uses — one reading of a line,
    // whichever mode the operator happens to be in. Then each one goes through
    // `save_proxy`, which is what seals the credentials.
    let prefix = name_prefix.trim();
    let mut saved = Vec::new();
    let mut rejected = Vec::new();
    for (line, parsed) in fury_shared::proxy_list::parse_block(&text) {
        match parsed {
            Ok(p) => {
                let name = if prefix.is_empty() {
                    format!("{}:{}", p.host, p.port)
                } else {
                    format!("{prefix} {}:{}", p.host, p.port)
                };
                let body = serde_json::json!({
                    "id": "",
                    "name": name,
                    "kind": p.scheme,
                    "host": p.host,
                    "port": p.port,
                    "username": p.username,
                    "password": p.password,
                });
                match save_proxy(state.clone(), body).await {
                    Ok(v) => saved.push(serde_json::json!({
                        "id": v.get("id").cloned().unwrap_or(serde_json::Value::Null),
                        "line": line,
                        "host": p.host,
                        "port": p.port,
                        "shape": format!("{:?}", p.shape),
                    })),
                    Err(e) => {
                        rejected.push(serde_json::json!({ "line": line, "error": e.message }))
                    }
                }
            }
            Err(e) => rejected.push(
                serde_json::json!({ "line": line, "error": e.message, "code": e.code }),
            ),
        }
    }
    Ok(serde_json::json!({ "saved": saved, "rejected": rejected }))
}

/// The name pattern, with `{n}` guaranteed to be in it.
///
/// Without a placeholder the number is appended rather than dropped: two
/// hundred profiles sharing one name is a list nobody can work in.
fn naming(pattern: &str, fallback: Option<&str>) -> String {
    let pattern = pattern.trim();
    if pattern.contains("{n}") {
        return pattern.to_string();
    }
    if !pattern.is_empty() {
        return format!("{pattern} {{n}}");
    }
    match fallback.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => format!("{f} {{n}}"),
        None => "Profile {n}".to_string(),
    }
}

/// Cookies out of a profile, and cookies in.
///
/// Both modes: the agent opens the profile through the ordinary launch path, so
/// a team profile still pulls its bundle, takes its lock and pushes what
/// changed.
#[tauri::command]
pub async fn export_cookies(id: String) -> R<serde_json::Value> {
    Ok(crate::agent::call("profile.cookies.export", serde_json::json!({ "id": id })).await?)
}

#[tauri::command]
pub async fn import_cookies(id: String, cookies: serde_json::Value) -> R<serde_json::Value> {
    Ok(crate::agent::call(
        "profile.cookies.import",
        serde_json::json!({ "id": id, "cookies": cookies }),
    )
    .await?)
}

/// Write the server kit out to a directory the operator picks.
///
/// The other half of the self-hosting instructions. They said "from a clone of
/// the repository" and the person reading them has an application, not a
/// checkout — so this hands over the 410 KB the server is built from and the
/// instructions become four commands that all work.
///
/// Carried inside the binary rather than downloaded: there is no published
/// repository to download from yet, it works with no network, and a kit that
/// ships with the app cannot be a different version from the app that wrote it.
#[tauri::command]
pub async fn save_server_kit(dir: String) -> Result<serde_json::Value, ApiErr> {
    const KIT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/server-kit.tar.gz"));

    let dir = std::path::PathBuf::from(shellexpand(&dir));
    std::fs::create_dir_all(&dir)
        .map_err(|e| ApiErr::local(format!("Could not create {}: {e}", dir.display())))?;

    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(KIT));
    // Refuses to write outside the chosen directory. tar entries can carry ..
    // and absolute paths, and this one is ours — but a kit that unpacks
    // anywhere is a habit, not a one-off.
    archive.set_overwrite(true);
    let mut count = 0usize;
    for entry in archive
        .entries()
        .map_err(|e| ApiErr::local(format!("The server kit is unreadable: {e}")))?
    {
        let mut entry = entry.map_err(|e| ApiErr::local(format!("Bad entry: {e}")))?;
        let path = entry
            .path()
            .map_err(|e| ApiErr::local(format!("Bad path in the kit: {e}")))?
            .into_owned();
        if path.is_absolute() || path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(ApiErr::local(format!("The kit contains an unsafe path: {}", path.display())));
        }
        // The parent has to exist first. tar::Entry::unpack writes exactly the
        // path it is given and does not create directories on the way, so the
        // very first entry — server/Cargo.toml — failed with "failed to unpack
        // into .../fury-server/server/Cargo.toml" on a directory that was never
        // made. Found by pressing the button; the code and the types were happy.
        let target = dir.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                ApiErr::local(format!("Could not create {}: {e}", parent.display()))
            })?;
        }
        entry
            .unpack(&target)
            .map_err(|e| ApiErr::local(format!("Could not write {}: {e}", path.display())))?;
        count += 1;
    }

    Ok(serde_json::json!({ "path": dir.display().to_string(), "files": count }))
}

fn shellexpand(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::Path::new(&home).join(rest).display().to_string();
        }
    }
    p.to_string()
}


// ---------------------------------------------------------------------------
// Giving one profile to one person
// ---------------------------------------------------------------------------
//
// The sealing happens HERE and not on the server, and that is the whole design
// rather than a detail. A member derives a profile's key from the organisation
// key; somebody in another organisation never can. So the owner's shell derives
// it, seals it to the recipient's public key, and posts a blob the server
// stores without being able to open -- exactly what it already does with
// wrapped_ork.
//
// The proxy key goes with it. A profile handed over without its proxy leaves
// through the receiver's own address, and an account that has only ever been
// seen from one country appearing from another is the failure every patch in
// core/patches exists to prevent. It is fetched rather than derived because it
// is wrapped under the organisation key and stored on the server.

/// Percent-encode an address for a query string.
///
/// Written out rather than pulled in: the shell has no URL crate, and the set
/// that matters for an email address is small and known. `+` is the one that
/// bites -- it is legal in an address, means a space in a query, and
/// alice+shop@example.com would otherwise be looked up as
/// "alice shop@example.com" and not found.
fn query_escape(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[derive(serde::Deserialize)]
struct FoundUser {
    user_id: String,
    public_key: String,
}

#[tauri::command]
pub async fn share_profile(
    state: State<'_, AppState>,
    profile_id: String,
    email: String,
    permissions: Vec<String>,
    expires_at: Option<String>,
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Err(ApiErr::local(
            "This profile is on this machine only. Sharing needs a server: send \
             it to one first, or export the project and hand over the file."
                .to_string(),
        ));
    }

    let org_key = state.org_key().ok_or_else(|| {
        ApiErr::local(
            "This machine is not holding the organisation key, and the profile \
             cannot be sealed for anybody without it."
                .to_string(),
        )
    })?;

    // Whole address, and a 404 here means exactly one thing worth saying: no
    // such account. The server deliberately cannot be asked "who is on you".
    let found: FoundUser = state
        .call(
            reqwest::Method::GET,
            &format!("/v1/users/lookup?email={}", query_escape(email.trim())),
            Body::None,
            true,
        )
        .await
        .map_err(|e| match e.status {
            404 => ApiErr::local(format!(
                "Nobody on this server uses {}. They need an account here before \
                 a profile can be given to them.",
                email.trim()
            )),
            _ => e,
        })?;

    let profile_key = crypto::profile_key(&org_key, &profile_id);
    let wrapped_key = crypto::wrap_for_member(&profile_key, &found.public_key)
        .map_err(|e| ApiErr::local(format!("Could not seal the profile key: {e}")))?;

    // The proxy, when there is one.
    let material: serde_json::Value = state
        .call(
            reqwest::Method::GET,
            &format!("/v1/profiles/{profile_id}/share-material"),
            Body::None,
            true,
        )
        .await?;
    let wrapped_proxy_key = match material.get("wrapped_proxy_dek").and_then(|v| v.as_str()) {
        Some(wrapped) => {
            let dek = crypto::unwrap_data_key(&org_key, wrapped)
                .map_err(|e| ApiErr::local(format!("Could not open the proxy key: {e}")))?;
            Some(
                crypto::wrap_for_member(&dek, &found.public_key)
                    .map_err(|e| ApiErr::local(format!("Could not seal the proxy key: {e}")))?,
            )
        }
        None => None,
    };

    state
        .call(
            reqwest::Method::POST,
            &format!("/v1/profiles/{profile_id}/shares"),
            Body::Json(serde_json::json!({
                "user_id": found.user_id,
                "permissions": permissions,
                "wrapped_key": wrapped_key,
                "wrapped_proxy_key": wrapped_proxy_key,
                "expires_at": expires_at,
            })),
            true,
        )
        .await
}

#[tauri::command]
pub async fn profile_shares(
    state: State<'_, AppState>,
    profile_id: String,
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(serde_json::json!([]));
    }
    state
        .call(
            reqwest::Method::GET,
            &format!("/v1/profiles/{profile_id}/shares"),
            Body::None,
            true,
        )
        .await
}

#[tauri::command]
pub async fn revoke_share(
    state: State<'_, AppState>,
    profile_id: String,
    user_id: String,
) -> R<serde_json::Value> {
    state
        .call(
            reqwest::Method::DELETE,
            &format!("/v1/profiles/{profile_id}/shares/{user_id}"),
            Body::None,
            true,
        )
        .await
}


/// Profiles other people have given to this account.
///
/// A separate list rather than more rows in the main one, and deliberately: a
/// profile somebody lent you is not a profile you own. It can be taken back
/// while you are looking at it, its name is somebody else's choice, and two
/// people can easily have one called "Shop DE". The list says who gave it.
#[tauri::command]
pub async fn shared_with_me(state: State<'_, AppState>) -> R<Vec<UiProfile>> {
    if mode_of(&state) == "local" {
        return Ok(Vec::new());
    }

    #[derive(serde::Deserialize)]
    struct Row {
        id: String,
        name: String,
        persona_id: String,
        tags: Vec<String>,
        shared_by: String,
        permissions: i64,
        proxy_display: Option<String>,
    }

    let rows: Vec<Row> = state
        .call(
            reqwest::Method::GET,
            "/v1/profiles/shared-with-me",
            Body::None,
            true,
        )
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| UiProfile {
            id: r.id,
            // Not in any project of this account's: it is in the OWNER's
            // project, which this account cannot see and has no business
            // seeing. The name of who lent it goes where the project would be,
            // because that is the useful fact about a borrowed profile.
            project_id: None,
            project_name: Some(r.shared_by),
            name: r.name,
            tags: r.tags,
            persona_id: r.persona_id,
            fp_seed: 0,
            proxy: r.proxy_display.map(|display| UiProxy {
                id: String::new(),
                name: String::new(),
                kind: String::new(),
                display,
                country: None,
            }),
            timezone: None,
            languages: None,
            // PermSet is a bitmask with no iterator, so the names are filtered
            // out of the canonical list rather than invented here -- one
            // spelling of a permission in this repository, not two.
            permissions: fury_shared::rbac::Perm::ALL
                .iter()
                .filter(|p| fury_shared::rbac::PermSet(r.permissions).has(**p))
                .map(perm_name)
                .collect(),
            lock: None,
            running: false,
            last_opened_at: None,
            // A profile lent TO this account: how many others hold it is the
            // owner's business, and the server does not say.
            shared_with: 0,
            origin: "shared",
        })
        .collect())
}


/// Send a profile that lives on this machine to the team server.
///
/// The missing half of showing both worlds in one list: local profiles became
/// visible beside the server's, and there was still no way to move one across.
/// Sharing needs the profile to be on a server, so without this the answer to
/// "give this to a colleague" was export a file and hand it over -- a copy
/// nobody can ever take back.
///
/// WHAT TRAVELS: the settings, and the browser data. The persona and the seed
/// above all -- a profile that arrives with a fresh fingerprint is a different
/// machine as far as every site it is logged into is concerned, which would
/// undo the warming it exists for.
///
/// THE PROXY GOES TOO, and the first version of this left it behind. That was
/// wrong for a reason worth stating: a profile without its exit leaves through
/// the receiver's own address, and an account only ever seen from one country
/// appearing from another is the failure the whole patch series exists to
/// prevent. A profile that arrives unusable is not a profile that arrived.
///
/// What that costs, said here and in the interface rather than discovered: the
/// credentials become the organisation's. They are re-sealed under the
/// organisation key -- they have to be, or nobody else's machine can open them
/// -- and anyone the profile is shared with can USE them. Whether they can READ
/// them is a separate right, `reveal_secrets`, which the share dialog leaves
/// off by default.
///
/// And the limit of that right, because a promise here would be a lie: the
/// proxy has to work on their machine, so the password has to be usable on
/// their machine, so somebody determined and technical can extract it. Not
/// showing it is real; making it unreadable to its own user is not possible.
///
/// An exit already in the organisation's list is REUSED rather than duplicated,
/// matched on host and port. Ten operators uploading profiles that share one
/// provider should not produce ten copies of one proxy, each with its own name
/// and its own separately-entered password.
///
/// The local original is left alone. This is a copy, and the machine it came
/// from keeps working while somebody decides whether the move was right.
/// A local profile as the server's create endpoint wants it.
///
/// A function, and tested, because this body was wrong in three places at once
/// and every one of them failed the same way: axum rejects a body that does not
/// match `NewProfileRequest` before the handler runs, with 422 and no text. The
/// interface showed "Request failed (422)" over a profile that was in every way
/// fine, and no log anywhere said which field.
///
///  * `fp_seed` went as a number. The server wants the wire form — sixteen hex
///    characters — and a seed is always a number locally, so EVERY upload that
///    has ever been attempted failed here. That is also why sending a folder to
///    the server produced a folder with nothing in it.
///  * `timezone` and `languages` went as `null` for a profile that follows its
///    exit, which is the ordinary case. The server means the same thing by an
///    empty string and an empty list — it stores NULL and '{}' — and `null` is
///    the one shape it will not parse.
///
/// `from_i64` reinterprets the eight bytes rather than converting them: half of
/// all seeds are negative as an i64, and a numeric cast would hand the profile
/// a different machine.
fn upload_body(me: &crate::agent::LocalProfile, proxy_id: Option<String>) -> serde_json::Value {
    serde_json::json!({
        "name": me.name,
        "tags": me.tags,
        "persona_id": me.persona_id,
        "fp_seed": fury_shared::persona::seed::to_hex(
            fury_shared::persona::seed::from_i64(me.fp_seed),
        ),
        "timezone": me.timezone.clone().unwrap_or_default(),
        "languages": me.languages.clone().unwrap_or_default(),
        "proxy_id": proxy_id,
    })
}

#[tauri::command]
pub async fn upload_profile(
    state: State<'_, AppState>,
    id: String,
    project_id: String,
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Err(ApiErr::local(
            "There is no server to send it to. Connect one first.".to_string(),
        ));
    }
    let org_key = state.org_key().ok_or_else(|| {
        ApiErr::local(
            "This machine is not holding the organisation key, and a profile \
             cannot be sealed for the team without it."
                .to_string(),
        )
    })?;

    // Read it from the agent rather than trusting what the interface last drew:
    // the seed and the persona are the things that must survive exactly, and a
    // stale row would move a profile that quietly differs from the one on disk.
    let local: Vec<crate::agent::LocalProfile> =
        crate::agent::call("profiles.list", serde_json::json!({})).await?;
    let me = local
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiErr::local("That profile is not on this machine.".to_string()))?;

    // A team profile goes out through a proxy, always, and that is a rule of the
    // product rather than of this function: the server's launch spec refuses a
    // profile without one, because launching it would send a colleague's
    // traffic from their own address. So it is said here, in a sentence, before
    // anything is created — the alternative is a profile that uploads, lists,
    // and dies the first time somebody opens it, on their machine and not ours.
    //
    // The same code the team-profile form raises, because it is the same rule
    // and a second wording for one rule is how two screens end up disagreeing
    // about it.
    if me.proxy.is_none() {
        return Err(ApiErr::coded(
            "err.uploadNeedsProxy",
            "A team profile needs a proxy. Everything the browser does goes through one.",
        ));
    }

    // The exit's credentials, read from the proxy list and not from the profile.
    //
    // `profiles.list` returns every proxy with `password: None`, on purpose and
    // correctly — a list view renders a host and a country, and a secret that is
    // never decrypted is a secret that cannot leak through one. `proxies.list`
    // is the call that opens the machine vault, and it is the one this needed.
    //
    // Taking the password from the profile row meant sealing `password: None`
    // under the organisation key and posting that. The colleague's machine
    // unsealed it faithfully, got a username and no password, and offered that
    // to a proxy which requires both — so the browser opened, every request
    // died in the relay, and Chrome said ERR_TUNNEL_CONNECTION_FAILED over a
    // proxy that was reachable and working. Reported as exactly that, from a
    // machine where `curl` reached the same proxy and was refused for want of
    // credentials.
    let local_proxies: Vec<crate::agent::LocalProxy> =
        crate::agent::call("proxies.list", serde_json::json!({})).await?;
    let full = me
        .proxy
        .as_ref()
        .and_then(|p| local_proxies.into_iter().find(|x| x.id == p.id));

    // The exit, before the profile, so the profile can be created pointing at
    // it. Failing here fails the whole upload rather than producing a profile
    // with no way out.
    let proxy_id = match full.as_ref().or(me.proxy.as_ref()) {
        None => None,
        Some(p) => {
            let existing: serde_json::Value = state
                .call(reqwest::Method::GET, "/v1/proxies", Body::None, true)
                .await?;
            let matching = existing
                .as_array()
                .into_iter()
                .flatten()
                .find(|x| {
                    x.get("host").and_then(|v| v.as_str()) == Some(p.host.as_str())
                        && x.get("port").and_then(|v| v.as_i64()) == Some(p.port as i64)
                })
                .and_then(|x| x.get("id").and_then(|v| v.as_str()).map(str::to_string));

            let credentials = fury_shared::api::ProxyCredentials {
                username: p.username.clone().filter(|v| !v.is_empty()),
                password: p.password.clone().filter(|v| !v.is_empty()),
            };
            let (enc, dek) = crypto::seal_proxy_credentials(&org_key, &credentials)
                .map_err(|e| ApiErr::local(format!("Could not seal the proxy credentials: {e}")))?;
            let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();

            match matching {
                // Found, and re-sealed anyway when this machine holds a
                // password for it.
                //
                // Every proxy the broken version of this function created is
                // sitting on the server with a username and no password, and
                // matching one and moving on would leave it that way for ever:
                // the profile would upload cleanly and fail in the relay, which
                // is the failure that is hardest to attribute to an upload
                // three days earlier.
                //
                // It writes over what is stored, and that is the trade. Two
                // operators holding different passwords for one host and port
                // would take turns; against that, a proxy nobody can use is
                // certain rather than possible, and the operator sending the
                // profile is the one who just proved the credentials work.
                Some(id) if credentials.password.is_some() => {
                    let updated: R<serde_json::Value> = state
                        .call(
                            reqwest::Method::PATCH,
                            &format!("/v1/proxies/{id}"),
                            Body::Json(serde_json::json!({
                                "name": p.name,
                                "kind": p.kind,
                                "host": p.host,
                                "port": p.port,
                                "credentials_enc": hex(&enc),
                                "wrapped_dek": hex(&dek),
                            })),
                            true,
                        )
                        .await;
                    if let Err(e) = &updated {
                        tracing::warn!(proxy = %id, error = %e.message, "could not refresh the stored proxy credentials");
                    }
                    Some(id)
                }
                Some(id) => Some(id),
                None => {
                    let made: serde_json::Value = state
                        .call(
                            reqwest::Method::POST,
                            "/v1/proxies",
                            Body::Json(serde_json::json!({
                                "name": p.name,
                                "kind": p.kind,
                                "host": p.host,
                                "port": p.port,
                                "credentials_enc": hex(&enc),
                                "wrapped_dek": hex(&dek),
                            })),
                            true,
                        )
                        .await?;
                    made.get("id").and_then(|v| v.as_str()).map(str::to_string)
                }
            }
        }
    };

    let created: serde_json::Value = state
        .call(
            reqwest::Method::POST,
            &format!("/v1/projects/{project_id}/profiles"),
            Body::Json(upload_body(&me, proxy_id)),
            true,
        )
        .await?;
    let remote_id = created
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ApiErr::local("The server created no profile.".to_string()))?
        .to_string();

    // A bundle upload needs a lock, because every bundle upload does: the lock
    // is what makes "nobody else is writing this" true rather than hoped.
    let grant: LockGrant = state
        .call(
            reqwest::Method::POST,
            &format!("/v1/profiles/{remote_id}/lock"),
            Body::Json(serde_json::json!({
                "machine_id": state.machine_id(),
                "machine_name": settings::machine_name(),
                "force": false,
            })),
            true,
        )
        .await?;

    let key = crypto::profile_key(&org_key, &remote_id);
    let key_hex: String = key.iter().map(|b| format!("{b:02x}")).collect();

    let uploaded: R<serde_json::Value> = crate::agent::call(
        "profile.uploadTo",
        serde_json::json!({
            "id": id,
            "remote_id": remote_id,
            "key": key_hex,
            "url": state.server_url()?,
            "token": state.session.token().ok_or_else(unauthenticated)?,
            "lock_token": grant.lock_token,
        }),
    )
    .await
    .map_err(Into::into);

    // The lock goes back whatever happened. A profile that arrived and stayed
    // locked by a machine that is not opening it is worse than one that did not
    // arrive: somebody has to be found to release it.
    //
    // WITH THE TOKEN, which is what this was missing. `release_lock` matches on
    // `lock_token` and refuses a body without one — deliberately, so that one
    // person signed in on a laptop and a desktop cannot close the other's
    // browser — and this call sent no body at all. It failed every time, and
    // the failure was discarded, so the profile stayed locked by the machine
    // that had just uploaded it.
    //
    // What that looked like: the uploader's own row went green and said "open",
    // and a colleague's said "In use — MacBook Air" over a browser nobody had
    // started. It cleared itself after ninety seconds, when the lock lapsed
    // with no heartbeat to renew it — which is why it reads as the application
    // being confused rather than as a lock, and why it was reported as "it is
    // not open, so why does it say it is".
    let released: R<serde_json::Value> = state
        .call(
            reqwest::Method::POST,
            &format!("/v1/profiles/{remote_id}/unlock"),
            Body::Json(serde_json::json!({ "lock_token": grant.lock_token })),
            true,
        )
        .await;
    if let Err(e) = &released {
        // Not fatal — the bundle is on the server and the lock lapses on its
        // own in ninety seconds — but never silent again: silence is what let
        // this survive.
        tracing::warn!(error = %e.message, profile = %remote_id, "could not release the upload lock");
    }

    let uploaded = uploaded?;
    Ok(serde_json::json!({
        "id": remote_id,
        "bytes": uploaded.get("bytes"),
        // True when a proxy was carried across, so the interface can say whose
        // it has become -- the organisation's, visible in the team's proxy list
        // and usable by anyone the profile is shared with.
        "proxy_moved": me.proxy.is_some(),
    }))
}

#[cfg(test)]
mod upload_tests {
    use super::*;
    use crate::agent::LocalProfile;

    /// The profile that met the 422, as the local store actually holds it:
    /// a seed that is a number, and no timezone or languages, because it
    /// follows whatever its exit turns out to be.
    fn follows_its_exit() -> LocalProfile {
        LocalProfile {
            id: "019fc39d-e44e-7b60-ad56-3297061f04bd".into(),
            project_id: None,
            project_name: None,
            name: "RU".into(),
            tags: vec![],
            persona_id: "win11-chrome".into(),
            fp_seed: 4985899071852176304,
            proxy: None,
            timezone: None,
            languages: None,
            running: false,
            last_opened_at: None,
        }
    }

    #[test]
    fn the_seed_crosses_in_the_form_the_server_parses() {
        let body = upload_body(&follows_its_exit(), Some("p".into()));
        let seed = body["fp_seed"].as_str().expect("a string, not a number");
        assert_eq!(seed.len(), 16, "the wire form is sixteen hex characters");
        // Parsed back exactly the way the server parses it, and equal to what
        // the local store holds. A seed that survives the trip as a different
        // number is a warmed account with a different machine.
        assert_eq!(
            fury_shared::persona::seed::from_hex(seed),
            Some(4985899071852176304u64),
        );
    }

    #[test]
    fn a_seed_with_the_top_bit_set_survives() {
        // Half of all seeds are negative as an i64. This is the half a numeric
        // conversion would lose, silently.
        let mut p = follows_its_exit();
        p.fp_seed = -1234567890123456789;
        let body = upload_body(&p, Some("p".into()));
        let seed = body["fp_seed"].as_str().unwrap();
        assert_eq!(
            fury_shared::persona::seed::from_hex(seed),
            Some((-1234567890123456789i64) as u64),
        );
    }

    #[test]
    fn following_the_exit_is_sent_as_empty_and_never_as_null() {
        // The server reads an empty string and an empty list as "this profile
        // follows its exit" and stores NULL and '{}'. It cannot parse null into
        // either field, so null is a 422 for the commonest profile there is.
        let body = upload_body(&follows_its_exit(), Some("p".into()));
        assert_eq!(body["timezone"], serde_json::json!(""));
        assert_eq!(body["languages"], serde_json::json!([]));
        assert!(!body["timezone"].is_null() && !body["languages"].is_null());
    }

    #[test]
    fn a_zone_that_was_chosen_is_carried_across_unchanged() {
        let mut p = follows_its_exit();
        p.timezone = Some("Europe/Berlin".into());
        p.languages = Some(vec!["de-DE".into(), "de".into()]);
        let body = upload_body(&p, Some("p".into()));
        assert_eq!(body["timezone"], serde_json::json!("Europe/Berlin"));
        assert_eq!(body["languages"], serde_json::json!(["de-DE", "de"]));
    }
}
