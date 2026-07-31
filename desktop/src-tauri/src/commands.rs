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

    // /v1/me without a token: a Fury server answers 401 with a JSON body. Any
    // 2xx/4xx JSON response proves something is listening and speaking our
    // dialect; a connection error does not.
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
    let looks_like_fury = probe
        .text()
        .await
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|v| v.get("error").is_some() || v.get("user_id").is_some());

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

#[tauri::command]
pub async fn lock(
    state: State<'_, AppState>,
    profile_id: String,
    force: bool,
) -> R<serde_json::Value> {
    let body = serde_json::json!({
        "machine_id": state.machine_id(),
        "machine_name": settings::machine_name(),
        "force": force,
    });
    state
        .call(
            reqwest::Method::POST,
            &format!("/v1/profiles/{profile_id}/lock"),
            Body::Json(body),
            true,
        )
        .await
}

#[tauri::command]
pub async fn unlock(state: State<'_, AppState>, profile_id: String) -> R<serde_json::Value> {
    state
        .call(
            reqwest::Method::POST,
            &format!("/v1/profiles/{profile_id}/unlock"),
            Body::None,
            true,
        )
        .await
}
