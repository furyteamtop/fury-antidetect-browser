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
}

impl From<crate::agent::AgentError> for ApiErr {
    fn from(e: crate::agent::AgentError) -> Self {
        // Status 0 is "never reached a server", which is exactly what an agent
        // failure is from the interface's point of view.
        ApiErr::local(e.to_string())
    }
}

impl ApiErr {
    fn local(message: impl Into<String>) -> Self {
        Self {
            status: 0,
            body: serde_json::Value::Null,
            message: message.into(),
        }
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
    }
}

// ---------------------------------------------------------------------------
// settings & session
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct Shell {
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
}

fn mode_of(state: &AppState) -> &'static str {
    if state.settings.lock().unwrap().server_url.is_some() {
        "team"
    } else {
        "local"
    }
}

#[tauri::command]
pub async fn shell_state(state: State<'_, AppState>) -> Result<Shell, ApiErr> {
    let mode = mode_of(&state);
    // Started on demand rather than announced as missing: telling someone to
    // open a terminal would make the desktop app useless to the people it is
    // for.
    let agent_ready = crate::agent::ensure_running().await.is_ok();

    Ok(Shell {
        server_url: state.settings.lock().unwrap().server_url.clone(),
        machine_name: settings::machine_name(),
        signed_in: state.session.is_signed_in(),
        native: true,
        mode,
        agent_ready,
        version: env!("CARGO_PKG_VERSION"),
        org_key_ready: state.org_key.lock().unwrap().is_some(),
        last_email: state.settings.lock().unwrap().last_email.clone(),
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

    match crypto::unlock_org_key(&password, &ok.kdf_salt, &ok.wrapped_private_key, ok.wrapped_ork.as_deref()) {
        Ok(key) => *state.org_key.lock().unwrap() = key,
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

    state
        .call(reqwest::Method::GET, "/v1/me", Body::None, true)
        .await
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
    pub permissions: Vec<String>,
    pub lock: Option<serde_json::Value>,
    /// Only ever true in local mode today: the agent knows what it launched.
    /// In team mode a colleague's browser is visible through `lock`, not here.
    pub running: bool,
    pub last_opened_at: Option<String>,
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
pub async fn projects(state: State<'_, AppState>) -> R<Vec<UiProject>> {
    if mode_of(&state) == "local" {
        let local: Vec<crate::agent::LocalProject> =
            crate::agent::call("projects.list", serde_json::json!({})).await?;
        return Ok(local
            .into_iter()
            .map(|p| UiProject {
                id: p.id,
                name: p.name,
                profile_count: p.profile_count,
            })
            .collect());
    }

    let remote: Vec<ProjectSummary> = state
        .call(reqwest::Method::GET, "/v1/projects", Body::None, true)
        .await?;
    Ok(remote
        .into_iter()
        .map(|p| UiProject {
            id: p.id.to_string(),
            name: p.name,
            profile_count: p.profile_count,
        })
        .collect())
}

#[tauri::command]
/// `project_id` absent means every profile on this machine — the Profiles view.
pub async fn profiles(
    state: State<'_, AppState>,
    project_id: Option<String>,
) -> R<Vec<UiProfile>> {
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
    Ok(remote
        .into_iter()
        .map(|p| UiProfile {
            id: p.id.to_string(),
            project_id: Some(p.project_id.to_string()),
            project_name: Some(p.project_name),
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
        })
        .collect())
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
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
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
        .org_key
        .lock()
        .unwrap()
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

    Ok(crate::agent::call(
        "profile.launch",
        serde_json::json!({
            "id": spec.profile_id.to_string(),
            "profile": profile,
            "server": { "url": server, "token": token },
            "lock_token": grant.lock_token,
        }),
    )
    .await?)
}

#[tauri::command]
pub async fn stop(state: State<'_, AppState>, profile_id: String) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
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
    let org_key = state.org_key.lock().unwrap().ok_or_else(|| {
        ApiErr::local(
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

#[tauri::command]
pub async fn proxies(state: State<'_, AppState>) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(crate::agent::call("proxies.list", serde_json::json!({})).await?);
    }
    state.call(reqwest::Method::GET, "/v1/proxies", Body::None, true).await
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

    // Editing a stored proxy needs a PATCH the server does not have yet, and
    // silently creating a second one instead would leave the team with two
    // exits where they asked for one.
    if proxy.get("id").and_then(|v| v.as_str()).is_some_and(|v| !v.is_empty()) {
        return Err(ApiErr::local(
            "Changing a proxy on a team server is not built yet — add a new one and move the \
             profiles across.",
        ));
    }

    let org_key = state.org_key.lock().unwrap().ok_or_else(|| {
        ApiErr::local(
            "This machine does not hold the organisation key yet, so it cannot seal a proxy's \
             credentials. An owner or admin has to hand the key over first.",
        )
    })?;

    let field = |k: &str| proxy.get(k).and_then(|v| v.as_str()).map(str::to_string);
    let credentials = fury_shared::api::ProxyCredentials {
        username: field("username").filter(|v| !v.is_empty()),
        password: field("password").filter(|v| !v.is_empty()),
    };
    let (credentials_enc, wrapped_dek) = crypto::seal_proxy_credentials(&org_key, &credentials)
        .map_err(|e| ApiErr::local(format!("Could not seal the credentials: {e}")))?;

    let hex = |v: &[u8]| v.iter().map(|b| format!("{b:02x}")).collect::<String>();
    state
        .call(
            reqwest::Method::POST,
            "/v1/proxies",
            Body::Json(serde_json::json!({
                "name": field("name").unwrap_or_default(),
                "kind": field("kind").unwrap_or_else(|| "socks5".into()),
                "host": field("host").unwrap_or_default(),
                "port": proxy.get("port").and_then(|v| v.as_i64()).unwrap_or(0),
                "project_id": serde_json::Value::Null,
                "credentials_enc": hex(&credentials_enc),
                "wrapped_dek": hex(&wrapped_dek),
            })),
            true,
        )
        .await
}

#[tauri::command]
pub async fn check_proxy(url: String, checker_url: Option<String>) -> R<serde_json::Value> {
    Ok(crate::agent::call(
        "proxies.check",
        serde_json::json!({ "url": url, "checker_url": checker_url }),
    )
    .await?)
}

#[tauri::command]
pub async fn rotate_proxy(id: String) -> R<serde_json::Value> {
    Ok(crate::agent::call("proxies.rotate", serde_json::json!({ "id": id })).await?)
}

#[tauri::command]
pub async fn delete_proxy(id: String) -> R<serde_json::Value> {
    Ok(crate::agent::call("proxies.delete", serde_json::json!({ "id": id })).await?)
}

#[tauri::command]
pub async fn save_profile(
    state: State<'_, AppState>,
    profile: serde_json::Value,
) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
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
            ApiErr::local(
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
            return Err(ApiErr::local(
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
pub async fn delete_profile(state: State<'_, AppState>, id: String) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
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
pub async fn trash() -> R<Vec<UiProfile>> {
    let local: Vec<crate::agent::LocalProfile> =
        crate::agent::call("profiles.trash", serde_json::json!({})).await?;
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
        })
        .collect())
}

/// Put profiles into a project, or take them out of every project.
#[tauri::command]
pub async fn move_profiles(
    state: State<'_, AppState>,
    ids: Vec<String>,
    project_id: Option<String>,
) -> R<serde_json::Value> {
    if mode_of(&state) != "local" {
        return Err(ApiErr::local(
            "Moving a profile between projects needs a server endpoint that does not exist yet.",
        ));
    }
    Ok(crate::agent::call(
        "profiles.move",
        serde_json::json!({ "ids": ids, "project_id": project_id }),
    )
    .await?)
}

#[tauri::command]
pub async fn restore_profile(id: String) -> R<serde_json::Value> {
    Ok(crate::agent::call("profiles.restore", serde_json::json!({ "id": id })).await?)
}

#[tauri::command]
pub async fn purge_profile(id: String) -> R<serde_json::Value> {
    Ok(crate::agent::call("profiles.purge", serde_json::json!({ "id": id })).await?)
}

#[tauri::command]
pub async fn rename_project(id: String, name: String) -> R<serde_json::Value> {
    Ok(crate::agent::call("projects.rename", serde_json::json!({ "id": id, "name": name })).await?)
}

#[tauri::command]
pub async fn delete_project(id: String) -> R<serde_json::Value> {
    Ok(crate::agent::call("projects.delete", serde_json::json!({ "id": id })).await?)
}

#[tauri::command]
pub async fn create_project(name: String) -> R<serde_json::Value> {
    Ok(crate::agent::call("projects.create", serde_json::json!({ "name": name })).await?)
}
