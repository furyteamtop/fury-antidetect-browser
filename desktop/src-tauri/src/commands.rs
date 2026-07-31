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
    pub project_id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub persona_id: String,
    /// Zero in team mode: the server never exposes a seed, and nothing in the
    /// interface may invent one.
    pub fp_seed: i64,
    /// Local mode only; a team server has no group concept yet.
    pub group_name: Option<String>,
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
pub async fn profiles(state: State<'_, AppState>, project_id: String) -> R<Vec<UiProfile>> {
    if mode_of(&state) == "local" {
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
                name: p.name,
                tags: p.tags,
                persona_id: p.persona_id,
                fp_seed: p.fp_seed,
                group_name: p.group_name,
            })
            .collect());
    }

    let remote: Vec<ProfileSummary> = state
        .call(
            reqwest::Method::GET,
            &format!("/v1/projects/{project_id}/profiles"),
            Body::None,
            true,
        )
        .await?;
    Ok(remote
        .into_iter()
        .map(|p| UiProfile {
            id: p.id.to_string(),
            project_id: p.project_id.to_string(),
            name: p.name,
            tags: p.tags,
            persona_id: p.persona_id,
            fp_seed: 0,
            group_name: None,
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
}

/// Open a profile.
///
/// In local mode this actually launches a browser. In team mode it still only
/// takes the lock — the agent does not yet know how to fetch a bundle from a
/// server, so saying anything else would be a lie the operator discovers by
/// waiting for a window that never appears.
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
        .insert(profile_id, grant.lock_token);

    Ok(serde_json::json!({
        "launched": false,
        "expires_at": grant.expires_at,
        "restrictions": grant.restrictions,
        "renewed": false,
    }))
}

#[tauri::command]
pub async fn stop(state: State<'_, AppState>, profile_id: String) -> R<serde_json::Value> {
    if mode_of(&state) == "local" {
        return Ok(crate::agent::call("profile.stop", serde_json::json!({ "id": profile_id })).await?);
    }

    let _: serde_json::Value = state
        .call(
            reqwest::Method::POST,
            &format!("/v1/profiles/{profile_id}/unlock"),
            Body::None,
            true,
        )
        .await?;
    state.locks.lock().unwrap().remove(&profile_id);
    Ok(serde_json::json!({ "stopped": true }))
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
pub async fn proxies() -> R<serde_json::Value> {
    Ok(crate::agent::call("proxies.list", serde_json::json!({})).await?)
}

#[tauri::command]
pub async fn save_proxy(proxy: serde_json::Value) -> R<serde_json::Value> {
    Ok(crate::agent::call("proxies.upsert", proxy).await?)
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
pub async fn save_profile(profile: serde_json::Value) -> R<serde_json::Value> {
    Ok(crate::agent::call("profiles.upsert", profile).await?)
}

#[tauri::command]
pub async fn delete_profile(id: String) -> R<serde_json::Value> {
    Ok(crate::agent::call("profiles.delete", serde_json::json!({ "id": id })).await?)
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
            name: p.name,
            tags: p.tags,
            persona_id: p.persona_id,
            fp_seed: p.fp_seed,
            group_name: p.group_name,
        })
        .collect())
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
pub async fn create_project(name: String) -> R<serde_json::Value> {
    Ok(crate::agent::call("projects.create", serde_json::json!({ "name": name })).await?)
}
