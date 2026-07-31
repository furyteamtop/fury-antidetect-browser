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
}

#[tauri::command]
pub fn shell_state(state: State<'_, AppState>) -> Shell {
    Shell {
        server_url: state.settings.lock().unwrap().server_url.clone(),
        machine_name: settings::machine_name(),
        signed_in: state.session.is_signed_in(),
        native: true,
    }
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

    Ok(shell_state(state))
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

    // Fetch identity immediately: every later screen needs it, and a login that
    // succeeds but whose session cannot be used should fail here, visibly.
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

#[tauri::command]
pub async fn projects(state: State<'_, AppState>) -> R<Vec<ProjectSummary>> {
    state
        .call(reqwest::Method::GET, "/v1/projects", Body::None, true)
        .await
}

#[tauri::command]
pub async fn profiles(state: State<'_, AppState>, project_id: String) -> R<Vec<ProfileSummary>> {
    state
        .call(
            reqwest::Method::GET,
            &format!("/v1/projects/{project_id}/profiles"),
            Body::None,
            true,
        )
        .await
}

/// What the server grants, in full. Only part of it crosses to the webview.
#[derive(Deserialize)]
struct LockGrant {
    lock_token: String,
    expires_at: String,
    restrictions: serde_json::Value,
}

/// What the interface is told: when the lock lapses, and how a launch would be
/// constrained. Not the token.
#[derive(Serialize)]
pub struct LockHeld {
    pub expires_at: String,
    pub restrictions: serde_json::Value,
    /// Seconds until the lock lapses unless something renews it. Nothing does
    /// yet — the heartbeat belongs to the agent (docs/01) — so the interface has
    /// to be able to say so rather than imply the lock is durable.
    pub renewed: bool,
}

#[tauri::command]
pub async fn lock(state: State<'_, AppState>, profile_id: String, force: bool) -> R<LockHeld> {
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
        .insert(profile_id, grant.lock_token);

    Ok(LockHeld {
        expires_at: grant.expires_at,
        restrictions: grant.restrictions,
        renewed: false,
    })
}

#[tauri::command]
pub async fn unlock(state: State<'_, AppState>, profile_id: String) -> R<()> {
    let _: serde_json::Value = state
        .call(
            reqwest::Method::POST,
            &format!("/v1/profiles/{profile_id}/unlock"),
            Body::None,
            true,
        )
        .await?;
    // Dropped only after the server agreed. Forgetting it on a failed release
    // would leave this process unable to prove it is the holder of a lock it
    // still has.
    state.locks.lock().unwrap().remove(&profile_id);
    Ok(())
}
