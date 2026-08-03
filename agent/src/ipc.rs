// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! The socket the desktop shell talks to.
//!
//! A Unix socket rather than a TCP port, deliberately (docs/01): nothing is
//! listening on the network, and file permissions are the access control. On a
//! machine where a browser is running arbitrary sites' JavaScript all day, a
//! `127.0.0.1` port is reachable from a page; a socket under a 0700 directory
//! is not.
//!
//! The protocol is newline-delimited JSON, one request per line, one response
//! per line. No framing beyond that, no streaming, no subscriptions — the shell
//! polls, and a protocol it can be debugged with by `nc` is worth more here than
//! one that saves a few bytes.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

use crate::launcher;
use crate::paths;
use crate::store::{Profile, Proxy, Store};

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    id: u64,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct Response {
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    ok: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    err: Option<String>,
}

/// A profile that is open right now.
struct Running {
    child: std::process::Child,
    /// Set when this launch came from a team server: what to push back to, and
    /// the version that was pulled, so the upload can refuse to clobber someone
    /// who saved in between.
    server: Option<(crate::sync::Server, i32)>,
    /// For a team profile: the key its bundle is wrapped under, derived by the
    /// shell from the organisation key. Absent for a profile that lives only
    /// here, which uses this machine's vault instead.
    profile_key: Option<[u8; 32]>,
    /// Proof that this machine is the one running the browser. The server
    /// requires it on upload, so it has to outlive the heartbeat that also
    /// uses it — the push happens after the browser is already gone.
    lock_token: Option<String>,
    /// Renews the lock while the browser runs. Aborted on stop — a lock kept
    /// alive after the browser closed is a profile nobody else can open.
    heartbeat: Option<tokio::task::JoinHandle<()>>,
    #[allow(dead_code)] // reported by `status`; kept for the automation API
    relay_port: u16,
    /// Present when this launch was allowed CDP. Kept so a cookie read on an
    /// already-open profile reuses the browser that is there rather than
    /// starting a second one on the same directory — two Chromiums on one
    /// user-data-dir is how a cookie jar gets corrupted.
    ws_endpoint: Option<String>,
    /// Must be aborted explicitly when the profile closes.
    ///
    /// Dropping a JoinHandle does not stop the task — tokio detaches it — so
    /// the first version leaked a listening relay per launch. Each one still
    /// forwarded to the customer's upstream, so a day of opening and closing
    /// profiles left a machine full of live exits nothing was using.
    relay: tokio::task::JoinHandle<()>,
}

pub struct Agent {
    store: Store,
    running: Mutex<HashMap<String, Running>>,
    core: Option<std::path::PathBuf>,
}

/// What the exit checker can tell us about where a proxy comes out.
///
/// Every field optional and independently so: a checker that answers with an
/// address and no timezone is a real thing, and a struct that made them travel
/// together would force a caller to choose between using nothing and using a
/// guess.
#[derive(Debug, Default, Clone)]
pub struct ExitFacts {
    pub ip: Option<String>,
    pub country: Option<String>,
    pub timezone: Option<String>,
    /// "lat,lng" as reported. Kept as the checker wrote it so that whatever
    /// stores it and whatever reads it cannot disagree about rounding.
    pub location: Option<String>,
}

/// Split "lat,lng" into a pair, or nothing.
///
/// Refuses anything outside the real ranges rather than clamping. A clamped
/// coordinate is a coordinate somebody typed wrong, and answering a page with
/// latitude 90 because the input said 900 is worse than answering with an
/// error — a browser sitting exactly on the north pole is a browser nobody has.
pub fn parse_location(raw: &str) -> Option<(f64, f64)> {
    let (lat, lng) = raw.trim().split_once(',')?;
    let lat: f64 = lat.trim().parse().ok()?;
    let lng: f64 = lng.trim().parse().ok()?;
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lng) {
        return None;
    }
    Some((lat, lng))
}

/// What to open on this launch.
///
/// Start URLs open on the FIRST launch and never again. The browser restores its
/// previous session (see launcher.rs), and start URLs passed on top of a
/// restored session are ADDED to it rather than replacing it — so the tenth
/// launch of a profile opened its start page for the tenth time and the
/// operator closed nine tabs by hand. Measured: two launches, one extra copy.
///
/// "Has it been opened before" is asked of the directory rather than of a flag
/// in the database, for the same reason `running` is: a flag can be wrong after
/// a crash, or after a bundle is unpacked from a colleague's machine, and the
/// Sessions directory is the very thing the browser will read a moment later.
fn start_urls_for(dir: &std::path::Path, configured: &[String]) -> Vec<String> {
    if dir.join("Default").join("Sessions").is_dir() {
        return Vec::new();
    }
    if configured.is_empty() {
        // Nothing configured means the start page, not a blank tab: the first
        // thing an operator needs after opening a profile is proof that the
        // disguise and the exit are working.
        return vec![crate::relay::START_URL.to_string()];
    }
    configured.to_vec()
}

impl Agent {
    pub async fn new() -> anyhow::Result<Arc<Self>> {
        paths::ensure_data_dir()?;
        let store = Store::open(&paths::db_path()).await?;
        // Every installation starts with somewhere to put a profile.
        store.default_project().await?;
        // Warm the keychain on its own thread. If the OS wants to ask about it,
        // it asks while the agent is already listening rather than instead of.
        store.vault_handle().prime();

        // Put the CDM where the browser looks, if this machine has one to give.
        //
        // Never fatal, and never blocking a launch: a machine with no Chrome
        // installed is a machine that gets a browser without DRM, which is a
        // detectable browser but a working one. Refusing to start over it would
        // trade a fingerprint problem for an outage.
        //
        // Done once at startup rather than per launch — it is a file copy of
        // about 19 MB, it is idempotent, and doing it while a user waits for a
        // profile to open would be felt.
        if let Some(core) = crate::core_binary() {
            match crate::widevine::destination_for(&core) {
                Some(dest) => match crate::widevine::stage(&dest) {
                    Ok(s) => tracing::info!(
                        bytes = s.bytes,
                        from = %s.from.display(),
                        "staged the Widevine CDM from the Chrome on this machine"
                    ),
                    Err(e) => tracing::warn!(
                        error = %e,
                        "no Widevine CDM staged — DRM video will not play, and \
                         requestMediaKeySystemAccess will refuse where real Chrome accepts"
                    ),
                },
                None => tracing::warn!("could not work out where the CDM belongs for this core"),
            }
        }

        Ok(Arc::new(Self {
            store,
            running: Mutex::new(HashMap::new()),
            core: crate::core_binary(),
        }))
    }

    /// Notice browsers the operator closed from their own window.
    ///
    /// Until this existed the shell learned a profile had closed only when
    /// somebody pressed Close in the shell, or tried to launch it again — the
    /// list reported `running` straight from the supervisor map and nothing
    /// ever took a dead child out of it. So a profile closed by its own red
    /// button stayed "open" in the interface, could not be reopened, and its
    /// relay went on listening and forwarding to the customer's upstream.
    ///
    /// A poll rather than a wait per child: `stop` already does the whole
    /// teardown — SIGTERM, relay, heartbeat, lock, and the bundle push for a
    /// team profile — and reaching it from one place keeps a closed-by-hand
    /// profile and a closed-by-button profile on exactly the same path. Two
    /// seconds is far below what anyone notices and costs a `waitpid` per open
    /// profile.
    fn start_reaper(self: &Arc<Self>) {
        let agent = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let gone: Vec<String> = {
                    let mut running = agent.running.lock().await;
                    running
                        .iter_mut()
                        .filter_map(|(id, entry)| match entry.child.try_wait() {
                            Ok(Some(_)) => Some(id.clone()),
                            _ => None,
                        })
                        .collect()
                };
                for id in gone {
                    tracing::info!(profile = %id, "browser closed from its own window");
                    if let Err(e) = agent.stop(&id).await {
                        tracing::warn!(profile = %id, error = %e, "could not tidy up after it");
                    }
                }
            }
        });
    }

    /// Listen until the process is stopped.
    pub async fn serve(self: Arc<Self>) -> anyhow::Result<()> {
        self.start_reaper();
        let path = paths::socket_path();

        // A socket file left by a crash would make bind() fail with "address in
        // use" forever. Removing one that is still live would be worse, so
        // check first: if something answers, another agent owns this machine.
        if path.exists() {
            if UnixStream::connect(&path).await.is_ok() {
                anyhow::bail!(
                    "another fury-agent is already running on this machine ({})",
                    path.display()
                );
            }
            std::fs::remove_file(&path)?;
        }

        let listener = UnixListener::bind(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The socket is the whole authorisation story: anyone who can open
            // it can launch profiles and read proxy credentials.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }

        tracing::info!(socket = %path.display(), "agent listening");

        loop {
            let (stream, _) = listener.accept().await?;
            let agent = Arc::clone(&self);
            tokio::spawn(async move {
                if let Err(e) = agent.handle_connection(stream).await {
                    tracing::debug!(error = %e, "connection ended");
                }
            });
        }
    }

    async fn handle_connection(self: Arc<Self>, stream: UnixStream) -> anyhow::Result<()> {
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Request>(&line) {
                Ok(req) => {
                    let id = req.id;
                    match self.dispatch(&req.method, req.params).await {
                        Ok(value) => Response { id, ok: Some(value), err: None },
                        Err(e) => Response { id, ok: None, err: Some(e.to_string()) },
                    }
                }
                Err(e) => Response {
                    id: 0,
                    ok: None,
                    err: Some(format!("malformed request: {e}")),
                },
            };
            let mut bytes = serde_json::to_vec(&response)?;
            bytes.push(b'\n');
            write.write_all(&bytes).await?;
        }
        Ok(())
    }

    /// The same dispatch the socket uses, for the local HTTP API.
    ///
    /// Deliberately the same and not a parallel surface: two implementations of
    /// "launch a profile" is one of them being separately wrong, and the one
    /// nobody exercises is the one that rots.
    pub async fn dispatch_public(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.dispatch(method, params).await
    }

    async fn dispatch(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        use serde_json::json;

        match method {
            "status" => {
                let running = self.running.lock().await;
                Ok(json!({
                    "version": env!("CARGO_PKG_VERSION"),
                    "core": self.core.as_ref().map(|p| p.display().to_string()),
                    "running": running.keys().collect::<Vec<_>>(),
                    "data_dir": paths::data_dir().display().to_string(),
                }))
            }

            "personas.list" => Ok(serde_json::to_value(crate::personas::catalogue())?),

            "projects.list" => Ok(serde_json::to_value(self.store.projects().await?)?),
            "projects.create" => {
                let name = str_param(&params, "name")?;
                let notes = params.get("notes").and_then(|v| v.as_str()).unwrap_or("");
                Ok(json!({ "id": self.store.create_project(&name, notes).await? }))
            }
            "projects.rename" => {
                let id = str_param(&params, "id")?;
                let name = str_param(&params, "name")?;
                let notes = params.get("notes").and_then(|v| v.as_str()).unwrap_or("");
                self.store.rename_project(&id, &name, notes).await?;
                Ok(json!({}))
            }
            "projects.delete" => {
                self.store.delete_project(&str_param(&params, "id")?).await?;
                Ok(json!({}))
            }

            "proxies.list" => Ok(serde_json::to_value(self.store.proxies().await?)?),
            "proxies.upsert" => {
                let proxy: Proxy = serde_json::from_value(params)?;
                Ok(json!({ "id": self.store.upsert_proxy(&proxy).await? }))
            }
            // A pasted block from a supplier.
            //
            // Parsed line by line, and every line reported back with its own
            // number and its own error — a block of two hundred with three bad
            // lines should save a hundred and ninety-seven and say which three.
            // Silently dropping the three is how an operator later finds
            // profiles pointing at nothing.
            //
            // Nothing is checked against the network here. Two hundred proxy
            // checks is two hundred round trips through two hundred different
            // exits, which takes minutes; the operator can select and check
            // afterwards, and a proxy that saves is not a proxy that works.
            "proxies.importMany" => {
                let text = str_param(&params, "text")?;
                let prefix = params
                    .get("name_prefix")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();

                let mut saved = Vec::new();
                let mut rejected = Vec::new();
                for (line_no, parsed) in fury_shared::proxy_list::parse_block(&text) {
                    match parsed {
                        Ok(p) => {
                            // The name has to be unique enough to pick out of a
                            // list. host:port is what a person recognises, and
                            // the prefix is there for "acme batch 3".
                            let name = if prefix.is_empty() {
                                format!("{}:{}", p.host, p.port)
                            } else {
                                format!("{prefix} {}:{}", p.host, p.port)
                            };
                            let proxy = Proxy {
                                id: String::new(),
                                name,
                                kind: p.scheme.clone(),
                                host: p.host.clone(),
                                port: p.port,
                                username: p.username.clone(),
                                password: p.password.clone(),
                                last_country: None,
                                last_ip: None,
                                last_timezone: None,
                                last_location: None,
                                rotate_url: None,
                                checker_url: None,
                            };
                            match self.store.upsert_proxy(&proxy).await {
                                Ok(id) => saved.push(json!({
                                    "id": id,
                                    "line": line_no,
                                    "host": p.host,
                                    "port": p.port,
                                    // Shown back before anything is used: the
                                    // difference between host:port:user:pass and
                                    // user:pass:host:port is invisible in the
                                    // result and expensive when guessed wrong.
                                    "shape": format!("{:?}", p.shape),
                                })),
                                Err(e) => rejected.push(
                                    json!({ "line": line_no, "error": e.to_string() }),
                                ),
                            }
                        }
                        Err(e) => rejected.push(json!({
                            "line": line_no,
                            "error": e.message,
                            // A stable name for the reason, so the desktop can
                            // say it in the operator's own language rather than
                            // printing an English sentence beside a Russian
                            // interface.
                            "code": e.code,
                        })),
                    }
                }
                Ok(json!({ "saved": saved, "rejected": rejected }))
            }

            // Does this exit actually work, and where does it come out?
            //
            // The one place the agent talks to a third party, and only when
            // someone presses the button. It has to: the exit IP is by
            // definition something only the far end can report. The service is
            // configurable for exactly that reason — an operator who does not
            // want to tell ipinfo.io that this proxy exists can point
            // FURY_IP_CHECK at their own.
            "proxies.check" => {
                let url = str_param(&params, "url")?;
                let upstream = crate::parse_upstream(&url)?;
                // Parsed first so a malformed proxy string fails here rather
                // than as an opaque transport error thirty seconds later.
                tracing::info!(?upstream, "checking proxy");

                let endpoint = params
                    .get("checker_url")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .map(str::to_string)
                    .or_else(|| std::env::var("FURY_IP_CHECK").ok())
                    .unwrap_or_else(|| "https://ipinfo.io/json".to_string());

                let client = reqwest::Client::builder()
                    .proxy(reqwest::Proxy::all(&url)?)
                    // Short: a proxy that takes longer than this to answer is
                    // not one anybody wants to browse through.
                    .timeout(std::time::Duration::from_secs(15))
                    .build()?;

                let started = std::time::Instant::now();
                let body: serde_json::Value = match client.get(&endpoint).send().await {
                    Ok(res) => res.json().await.unwrap_or(serde_json::Value::Null),
                    Err(e) => {
                        return Ok(json!({
                            "ok": false,
                            "error": if e.is_timeout() {
                                "The proxy did not answer in time.".to_string()
                            } else if e.is_connect() {
                                "Could not connect to the proxy.".to_string()
                            } else {
                                format!("The check failed: {e}")
                            },
                        }))
                    }
                };

                // Remembered, so a profile that follows its exit does not pay
                // for this round trip again at launch.
                if let Some(id) = params.get("id").and_then(|v| v.as_str()) {
                    let s = |k: &str| body.get(k).and_then(|v| v.as_str());
                    let _ = self
                        .store
                        .record_exit(id, s("ip"), s("country"), s("timezone"), s("loc"))
                        .await;
                }

                Ok(json!({
                    "ok": true,
                    "ip": body.get("ip"),
                    "country": body.get("country"),
                    "city": body.get("city"),
                    "timezone": body.get("timezone"),
                    "org": body.get("org"),
                    "ms": started.elapsed().as_millis() as u64,
                }))
            }

            // Ask the provider for a new exit.
            //
            // Rotating residential and mobile proxies are sold with a link that
            // does this, and an operator who has to leave the app to curl it
            // will forget to — then wonder why two profiles share an IP. The
            // request goes out *through* the proxy being rotated, because
            // providers key rotation to the session that asks.
            "proxies.rotate" => {
                let id = str_param(&params, "id")?;
                let proxy = self
                    .store
                    .proxies()
                    .await?
                    .into_iter()
                    .find(|p| p.id == id)
                    .ok_or_else(|| anyhow::anyhow!("no such proxy"))?;

                let rotate = proxy
                    .rotate_url
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("this proxy has no rotation link")
                    })?;

                let client = reqwest::Client::builder()
                    .proxy(reqwest::Proxy::all(&proxy.url())?)
                    .timeout(std::time::Duration::from_secs(30))
                    .build()?;

                match client.get(rotate).send().await {
                    Ok(res) => Ok(json!({ "ok": res.status().is_success(), "status": res.status().as_u16() })),
                    Err(e) => Ok(json!({ "ok": false, "error": format!("Rotation failed: {e}") })),
                }
            }

            "proxies.delete" => {
                self.store.delete_proxy(&str_param(&params, "id")?).await?;
                Ok(json!({}))
            }

            "profiles.list" => {
                // No project_id means every profile on this machine. That is
                // the Profiles view and the ordinary case; a project narrows it.
                let project = params.get("project_id").and_then(|v| v.as_str());
                let mut profiles = self.store.profiles(project).await?;
                // try_wait here as well as in the reaper: the shell asks for
                // this list the instant a window closes, and answering "open"
                // for two seconds is the difference between the interface
                // feeling alive and feeling stuck.
                let mut running = self.running.lock().await;
                running.retain(|_, entry| !matches!(entry.child.try_wait(), Ok(Some(_))));
                let running = running;
                // The list is the only place the shell learns what is open, so
                // the answer has to come from the supervisor rather than from a
                // flag in the database that a crash would leave stale.
                let out: Vec<serde_json::Value> = profiles
                    .drain(..)
                    .map(|p| {
                        let mut v = serde_json::to_value(&p).unwrap_or_default();
                        v["running"] = json!(running.contains_key(&p.id));
                        v
                    })
                    .collect();
                Ok(serde_json::to_value(out)?)
            }
            "profiles.upsert" => {
                let profile: Profile = serde_json::from_value(params)?;
                Ok(json!({ "id": self.store.upsert_profile(&profile).await? }))
            }
            // Make many at once. The one-at-a-time dialog is fine for a first
            // profile and useless for the two hundredth, which is the number
            // people actually run.
            //
            // Every profile gets its OWN seed and its own persona roll. That is
            // the whole safety property: the seed drives the canvas, audio and
            // geometry noise, so two profiles sharing one would produce
            // byte-identical fingerprints and link the accounts to each other
            // permanently — the exact failure this product exists to prevent,
            // committed two hundred times in one click. `upsert_profile`
            // generates the seed when it sees zero; nothing here supplies one.
            //
            // Sharing a persona between profiles is fine and is not the same
            // thing. A persona describes a machine millions of people own.
            "profiles.createMany" => {
                let count = params.get("count").and_then(|v| v.as_u64()).unwrap_or(0);
                if !(1..=500).contains(&count) {
                    anyhow::bail!(
                        "count must be between 1 and 500 — 500 profiles is already more \
                         than a person can supervise, and a typo in this field should not \
                         fill the database"
                    );
                }
                let template: Profile = serde_json::from_value(
                    params.get("template").cloned().unwrap_or(serde_json::Value::Null),
                )
                .map_err(|e| anyhow::anyhow!("template is not a profile: {e}"))?;

                // "Shop {n}" numbers from 1. Without a placeholder the names are
                // suffixed, because two hundred profiles called the same thing
                // is a list nobody can work in.
                let pattern = params
                    .get("name_pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let pattern = if pattern.is_empty() {
                    if template.name.trim().is_empty() { "Profile {n}".to_string() }
                    else { format!("{} {{n}}", template.name.trim()) }
                } else if pattern.contains("{n}") {
                    pattern
                } else {
                    format!("{pattern} {{n}}")
                };

                // Absent means "spread them over the catalogue by how common
                // each machine is" — see catalogue::pick_weighted.
                let pick_persona = template.persona_id.trim().is_empty();

                let mut made = Vec::new();
                let mut failed = Vec::new();
                for i in 1..=count {
                    let mut p = template.clone();
                    p.id = String::new();
                    p.fp_seed = 0;
                    p.name = pattern.replace("{n}", &i.to_string());
                    if pick_persona {
                        p.persona_id = fury_shared::catalogue::pick_weighted(rand::random()).id;
                    }
                    match self.store.upsert_profile(&p).await {
                        Ok(id) => made.push(id),
                        // Partial success is the normal outcome of a loop over a
                        // database, and reporting only the count would leave an
                        // operator guessing which of the two hundred to redo.
                        Err(e) => failed.push(json!({ "n": i, "error": e.to_string() })),
                    }
                }
                Ok(json!({ "created": made, "failed": failed }))
            }

            // Copy a profile's setup without copying its identity.
            //
            // The line this feature lives or dies on: a clone that copies the
            // seed is not a second account, it is the same machine twice. The
            // canvas, audio and client-rects noise all derive from the seed, so
            // two profiles sharing one are byte-identical to every detector that
            // reads a canvas — which is every one of them. The accounts are then
            // linked for as long as they exist, and nothing about the interface
            // would have suggested it.
            //
            // So the seed is regenerated, and the browser data is not copied
            // either: cookies and local storage ARE the logged-in account, and a
            // clone that carried them would be two windows on one account rather
            // than two accounts. What is copied is the setup a person spent time
            // on — persona, proxy, timezone, languages, tags, notes, start URLs.
            //
            // The proxy IS shared, deliberately. Two profiles behind one exit is
            // a normal household; the operator can reassign afterwards, and
            // silently leaving the copy with no proxy would produce a profile
            // that refuses to launch for a reason nobody wrote down.
            "profiles.clone" => {
                let id = str_param(&params, "id")?;
                let source = self
                    .store
                    .profile(&id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("no such profile"))?;

                let count = params.get("count").and_then(|v| v.as_u64()).unwrap_or(1);
                if !(1..=500).contains(&count) {
                    anyhow::bail!("count must be between 1 and 500");
                }

                let mut made = Vec::new();
                let mut failed = Vec::new();
                for i in 1..=count {
                    let mut p = source.clone();
                    p.id = String::new();
                    // Regenerated by upsert_profile. Explicit rather than
                    // inherited: this zero is the whole safety property.
                    p.fp_seed = 0;
                    p.last_opened_at = None;
                    p.name = match params.get("name").and_then(|v| v.as_str()) {
                        Some(pattern) if pattern.contains("{n}") => {
                            pattern.replace("{n}", &i.to_string())
                        }
                        Some(pattern) if count == 1 => pattern.to_string(),
                        Some(pattern) => format!("{pattern} {i}"),
                        None if count == 1 => format!("{} copy", source.name),
                        None => format!("{} copy {i}", source.name),
                    };
                    match self.store.upsert_profile(&p).await {
                        Ok(new_id) => made.push(new_id),
                        Err(e) => failed.push(json!({ "n": i, "error": e.to_string() })),
                    }
                }
                Ok(json!({ "created": made, "failed": failed }))
            }

            "profiles.move" => {
                let ids: Vec<String> = params
                    .get("ids")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .ok_or_else(|| anyhow::anyhow!("missing parameter \"ids\""))?;
                // Absent or null means "out of every project", which is a
                // destination like any other, not a missing argument.
                let project = params.get("project_id").and_then(|v| v.as_str());
                Ok(json!({ "moved": self.store.move_profiles(&ids, project).await? }))
            }
            "profiles.delete" => {
                self.store.delete_profile(&str_param(&params, "id")?).await?;
                Ok(json!({}))
            }

            // What a profile will claim to be, before it exists.
            //
            // The competition builds a fingerprint from independent dropdowns —
            // pick a user agent here, a GPU renderer there — which lets anyone
            // assemble a machine that cannot exist: a macOS user agent with an
            // NVIDIA renderer, a 15px Windows scrollbar on a Mac. Those are not
            // weaker disguises, they are *signals*: no real device looks like
            // that, so the combination itself is the tell. Deriving from a
            // measured persona and showing the result before saving is the
            // difference between choosing a device and inventing one.
            "profile.preview" => {
                let persona = crate::personas::load(&str_param(&params, "persona_id")?)?;
                let seed = params.get("fp_seed").and_then(|v| v.as_i64()).unwrap_or(1);
                let languages: Vec<String> = params
                    .get("languages")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_else(|| vec!["en-US".into(), "en".into()]);
                let ctx = fury_shared::persona::ProfileContext {
                    timezone: params
                        .get("timezone")
                        .and_then(|v| v.as_str())
                        .unwrap_or("UTC")
                        .to_string(),
                    // The preview is what the profile WILL claim, so it resolves
                    // the UI locale exactly as the launch does. A preview that
                    // showed a different locale from the launch would be worse
                    // than no preview: it is read as a check that passed.
                    ui_locale: fury_shared::locale::ui_locale_for(
                        languages.first().map(|s| s.as_str()),
                    ),
                    // The preview has no proxy to ask, so it cannot know where
                    // the profile will come out. Showing a position here would
                    // be showing one the launch may not use.
                    geolocation: None,
                    languages,
                    chrome_major: crate::CHROME_MAJOR,
                    chrome_full_version: crate::CHROME_FULL_VERSION.to_string(),
                };

                let problems: Vec<String> = persona
                    .validate()
                    .err()
                    .map(|errs| errs.iter().map(|e| e.to_string()).collect())
                    .unwrap_or_default();

                let cfg = persona.derive_core_config(seed.max(1) as u64, &ctx);
                let get = |path: &str| -> serde_json::Value {
                    path.split('.')
                        .try_fold(&cfg, |n, part| n.get(part))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null)
                };

                Ok(json!({
                    "user_agent": get("navigator.userAgent"),
                    "platform": get("navigator.platform"),
                    "languages": get("navigator.languages"),
                    "timezone": get("locale.timezone"),
                    "hardware_concurrency": get("navigator.hardwareConcurrency"),
                    "device_memory": get("navigator.deviceMemory"),
                    "screen": format!(
                        "{}×{}",
                        get("screen.width").as_i64().unwrap_or(0),
                        get("screen.height").as_i64().unwrap_or(0)
                    ),
                    "gpu_vendor": get("gpu.webglParams.UNMASKED_VENDOR_WEBGL"),
                    "gpu_renderer": get("gpu.webglParams.UNMASKED_RENDERER_WEBGL"),
                    "client_hints_platform": get("clientHints.platform"),
                    "fonts": get("fonts").as_array().map(|a| a.len()).unwrap_or(0),
                    // Named individually rather than as one "noise: on": these
                    // are independent streams, and a profile with canvas noise
                    // off is a materially different thing from one with it on.
                    "noise": {
                        "canvas": !get("noise.canvasSeed").is_null(),
                        "audio": !get("noise.audioSeed").is_null(),
                        "client_rects": !get("noise.clientRectsSeed").is_null(),
                    },
                    "problems": problems,
                }))
            }

            "projects.export" => {
                let id = str_param(&params, "id")?;
                let dest = std::path::PathBuf::from(str_param(&params, "path")?);
                let passphrase = str_param(&params, "passphrase")?;
                let with_data = params
                    .get("with_data")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);

                // Nothing may be open: a browser writes its cookie jar on its
                // own schedule, so exporting a running profile copies a
                // half-written database that restores as a logged-out account.
                {
                    let running = self.running.lock().await;
                    let live = self.store.profiles(Some(&id)).await?;
                    if live.iter().any(|p| running.contains_key(&p.id)) {
                        anyhow::bail!(
                            "close the profiles in this project first — a browser writes its \
                             cookies on its own schedule, and copying them while it runs \
                             produces a profile that restores logged out"
                        );
                    }
                }

                let bytes = crate::transfer::export_project(
                    &self.store, &id, &dest, &passphrase, with_data, paths::profile_dir,
                )
                .await?;
                Ok(json!({ "path": dest.display().to_string(), "bytes": bytes }))
            }

            "projects.import" => {
                let src = std::path::PathBuf::from(str_param(&params, "path")?);
                let passphrase = str_param(&params, "passphrase")?;
                let (project_id, count) = crate::transfer::import_project(
                    &self.store, &src, &passphrase, paths::profile_dir,
                )
                .await?;
                Ok(json!({ "project_id": project_id, "profiles": count }))
            }

            // Seal a profile into a bundle, and open one back.
            //
            // Here before sync needs it, and reachable on its own, because the
            // encryption is the part that has to be right: a sync layer built
            // on top of an untested seal would be discovered wrong by a server
            // holding readable cookies.
            // Cookies out, and cookies in.
            //
            // Both go through the browser rather than through its SQLite file —
            // see cookies.rs for why the file is not readable out-of-browser on
            // Windows at all, and why a badly written row costs a whole site's
            // session rather than one cookie.
            //
            // A closed profile is opened for the exchange and closed again.
            // Deliberately the ordinary launch and the ordinary stop, not a
            // quiet side path: in team mode the launch pulls the bundle and
            // takes the lock, and the stop packs and pushes. An import that
            // spawned its own browser would skip all four, and the cookies
            // would be on this machine and nowhere else.
            "profile.cookies.export" | "profile.cookies.import" => {
                let id = str_param(&params, "id")?;
                let importing = method.ends_with("import");

                let incoming: Vec<serde_json::Value> = if importing {
                    let raw = params
                        .get("cookies")
                        .ok_or_else(|| anyhow::anyhow!("missing parameter \"cookies\""))?;
                    serde_json::from_value(raw.clone())
                        .map_err(|e| anyhow::anyhow!("cookies must be a JSON array: {e}"))?
                } else {
                    Vec::new()
                };

                let open = self
                    .running
                    .lock()
                    .await
                    .get(&id)
                    .and_then(|r| r.ws_endpoint.clone());

                let (ws, opened_here) = match open {
                    Some(ws) => (ws, false),
                    None => {
                        if self.running.lock().await.contains_key(&id) {
                            anyhow::bail!(
                                "this profile is open without a debugging port, so its cookies \
                                 cannot be reached. Close it and try again, or launch it with \
                                 CDP enabled"
                            );
                        }
                        let out = self.launch(&id, None, None, None, None, true).await?;
                        let ws = out
                            .get("ws_endpoint")
                            .and_then(|v| v.as_str())
                            .map(str::to_string)
                            .ok_or_else(|| {
                                anyhow::anyhow!(
                                    "the browser started but never wrote a debugging endpoint, so \
                                     there is nothing to read cookies through"
                                )
                            })?;
                        (ws, true)
                    }
                };

                let result = if importing {
                    crate::cookies::import(&ws, &incoming).await.map(|r| {
                        json!({
                            "imported": r.accepted,
                            // Named rather than buried: a session cookie does
                            // not survive the profile closing, and this call
                            // closes it.
                            "session_only": r.session_only,
                            "skipped": r.skipped,
                        })
                    })
                } else {
                    crate::cookies::export(&ws)
                        .await
                        .map(|c| json!({ "cookies": c }))
                };

                // Closed whatever happened. A browser left running because the
                // import failed is a locked profile nobody else can open, and
                // the operator has no reason to know it is there.
                if opened_here {
                    if let Err(e) = self.stop(&id).await {
                        tracing::warn!(error = %e, "could not close the profile after a cookie exchange");
                    }
                }
                result
            }

            "profile.pack" => {
                let id = str_param(&params, "id")?;
                {
                    let running = self.running.lock().await;
                    if running.contains_key(&id) {
                        anyhow::bail!(
                            "close the profile first — a browser writes its cookie jar on its \
                             own schedule, and a bundle taken mid-run restores logged out"
                        );
                    }
                }
                let sealed = crate::bundle::pack(
                    &paths::profile_dir(&id),
                    &crate::bundle::Sealer::Machine(self.store.vault()),
                )?;
                let out = paths::data_dir().join("bundles");
                std::fs::create_dir_all(&out)?;
                let path = out.join(format!("{id}.bundle"));
                std::fs::write(&path, &sealed.bytes)?;
                Ok(json!({
                    "path": path.display().to_string(),
                    "bytes": sealed.bytes.len(),
                    "sha256": sealed.sha256,
                    "wrapped_key": sealed.wrapped_key,
                }))
            }

            "profile.unpack" => {
                let id = str_param(&params, "id")?;
                let path = std::path::PathBuf::from(str_param(&params, "path")?);
                let wrapped_key = str_param(&params, "wrapped_key")?;
                let files = crate::bundle::unpack(
                    &std::fs::read(&path)?,
                    &wrapped_key,
                    &crate::bundle::Sealer::Machine(self.store.vault()),
                    &paths::profile_dir(&id),
                )?;
                Ok(json!({ "files": files }))
            }

            "profiles.trash" => {
                // Carries `running: false` explicitly. Nothing in the trash can
                // be open, but the shell reads one shape for both listings, and
                // a field that is merely usually absent is a field that breaks
                // something later.
                let rows = self.store.deleted_profiles().await?;
                let out: Vec<serde_json::Value> = rows
                    .into_iter()
                    .map(|p| {
                        let mut v = serde_json::to_value(&p).unwrap_or_default();
                        v["running"] = json!(false);
                        v
                    })
                    .collect();
                Ok(serde_json::to_value(out)?)
            }
            "profiles.restore" => {
                self.store.restore_profile(&str_param(&params, "id")?).await?;
                Ok(json!({}))
            }
            "profiles.purge" => {
                let id = str_param(&params, "id")?;
                self.store.purge_profile(&id, &paths::profile_dir(&id)).await?;
                Ok(json!({}))
            }

            "profile.launch" => {
                let server: Option<crate::sync::Server> = params
                    .get("server")
                    .cloned()
                    .and_then(|v| serde_json::from_value(v).ok());
                let lock_token = params
                    .get("lock_token")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                // Present for a team profile, whose record lives on the
                // server rather than in this machine's store.
                let inline: Option<Profile> = params
                    .get("profile")
                    .cloned()
                    .map(serde_json::from_value)
                    .transpose()?;
                // Hex, because it is a key and hex is what the rest of this
                // protocol uses for one. Absent means "use this machine's
                // vault", which is what a local profile wants.
                let profile_key = params
                    .get("profile_key")
                    .and_then(|v| v.as_str())
                    .map(|v| {
                        let bytes: Option<Vec<u8>> = (v.len() == 64
                            && v.bytes().all(|b| b.is_ascii_hexdigit()))
                        .then(|| {
                            (0..64)
                                .step_by(2)
                                .filter_map(|i| u8::from_str_radix(&v[i..i + 2], 16).ok())
                                .collect()
                        });
                        bytes
                            .and_then(|b| <[u8; 32]>::try_from(b).ok())
                            .ok_or_else(|| anyhow::anyhow!("profile_key must be 64 hex characters"))
                    })
                    .transpose()?;
                // Off unless asked for. A debugging port nobody requested is a
                // way into the browser nobody is watching.
                let cdp = params.get("cdp").and_then(|v| v.as_bool()).unwrap_or(false);
                self.launch(
                    &str_param(&params, "id")?,
                    server,
                    lock_token,
                    inline,
                    profile_key,
                    cdp,
                )
                .await
            }
            "profile.stop" => self.stop(&str_param(&params, "id")?).await,

            other => anyhow::bail!("unknown method {other:?}"),
        }
    }

    /// Ask a proxy where its traffic comes out.
    ///
    /// The same request the operator's "where does it come out?" button makes,
    /// so a launch and a check can never disagree about an exit.
    async fn resolve_exit(
        url: &str,
        checker_url: Option<&str>,
    ) -> anyhow::Result<ExitFacts> {
        let endpoint = checker_url
            .map(str::to_string)
            .filter(|s| !s.trim().is_empty())
            .or_else(|| std::env::var("FURY_IP_CHECK").ok())
            .unwrap_or_else(|| "https://ipinfo.io/json".to_string());

        let client = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(url)?)
            .timeout(std::time::Duration::from_secs(15))
            .build()?;

        let body: serde_json::Value = client.get(&endpoint).send().await?.json().await?;
        let s = |k: &str| body.get(k).and_then(|v| v.as_str()).map(str::to_string);
        // `loc` is "lat,lng" and was being thrown away. It comes from the same
        // request as the timezone and describes the same place, which is exactly
        // the property geolocation needs: a profile whose clock follows Berlin
        // while its position follows the host machine is the contradiction the
        // timezone work existed to remove, one field over.
        Ok(ExitFacts {
            ip: s("ip"),
            country: s("country"),
            timezone: s("timezone"),
            location: s("loc").filter(|v| parse_location(v).is_some()),
        })
    }

    async fn launch(
        &self,
        profile_id: &str,
        server: Option<crate::sync::Server>,
        lock_token: Option<String>,
        inline: Option<Profile>,
        profile_key: Option<[u8; 32]>,
        cdp: bool,
    ) -> anyhow::Result<serde_json::Value> {
        {
            let mut running = self.running.lock().await;
            if let Some(existing) = running.get_mut(profile_id) {
                // try_wait rather than a stored flag: the browser may have been
                // closed from its own window, and reporting it as open would
                // leave the operator with a profile they cannot start.
                if existing.child.try_wait()?.is_none() {
                    anyhow::bail!("this profile is already open");
                }
                // Closed from its own window rather than through us. The relay
                // is still up and has to go with it.
                if let Some(dead) = running.remove(profile_id) {
                    dead.relay.abort();
                }
            }
        }

        // A team profile does not exist in this machine's store — it lives on
        // the server, and the shell has already fetched it, opened the proxy
        // credentials with the organisation key, and handed the result down.
        //
        // The shell does that unsealing, not the agent, and deliberately: the
        // organisation key opens every proxy and every profile in the team, and
        // this is the process that runs a browser executing other people's
        // JavaScript all day. It gets the one profile it was asked for.
        let from_server = inline.is_some();
        let profile = match inline {
            Some(p) => p,
            None => self
                .store
                .profile(profile_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("no such profile"))?,
        };

        let proxy = profile.proxy.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "this profile has no proxy. Everything the core does goes through one, so \
                 launching without it would send traffic from this machine's own address"
            )
        })?;

        let core = self.core.clone().ok_or_else(|| {
            // The reason matters more than the fact. "No core" with a stale
            // FURY_CORE set reads as "you never installed one", and the person
            // goes and installs a second copy that is also not used.
            match crate::core_lookup_problem() {
                Some(why) => anyhow::anyhow!("no core binary: {why}"),
                None => anyhow::anyhow!(
                    "no core binary found. Install one with `fury-agent install-core <file>`, \
                     build it, or point FURY_CORE at one"
                ),
            }
        })?;

        let upstream = crate::parse_upstream(&proxy.url())?;
        let (relay_port, relay_task) = crate::relay::Relay::new(upstream).serve(0).await?;

        let persona = crate::personas::load(&profile.persona_id)?;
        if let Err(errs) = persona.validate() {
            anyhow::bail!(
                "persona {} is inconsistent and would stand out:\n  {}",
                persona.id,
                errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n  ")
            );
        }

        // A profile that pins neither its timezone nor its languages follows its
        // exit for both, and both come out of one lookup.
        //
        // Claiming UTC while leaving through Berlin is the cheapest thing a
        // detector checks: one subtraction between
        // Intl.DateTimeFormat().resolvedOptions().timeZone and the geolocation
        // of the address the request came from. Announcing `en-US,en` from that
        // same Berlin exit is the same check one field over — Accept-Language
        // against the exit country — and it used to be hardcoded here.
        //
        // Asked for once. The stored answer is used when there is one; a second
        // in front of a launch is worth less than an account, but two seconds
        // for two questions with one answer is waste.
        let want_timezone = profile.timezone.is_none();
        let want_languages = profile.languages.is_none();
        let mut exit_country = proxy.last_country.clone();
        let mut exit_timezone = proxy.last_timezone.clone();
        let mut exit_location = proxy.last_location.clone();

        // The position joins the same lookup: one round trip answers where the
        // exit is, what time it is there and what language it speaks, and asking
        // separately would let the three drift apart.
        if (want_timezone && exit_timezone.is_none())
            || (want_languages && exit_country.is_none())
            || exit_location.is_none()
        {
            match Self::resolve_exit(&proxy.url(), proxy.checker_url.as_deref()).await {
                Ok(facts) => {
                    let _ = self
                        .store
                        .record_exit(
                            &proxy.id,
                            facts.ip.as_deref(),
                            facts.country.as_deref(),
                            facts.timezone.as_deref(),
                            facts.location.as_deref(),
                        )
                        .await;
                    // Only overwrite with an answer. A checker that returns a
                    // timezone and no country must not blank a country we
                    // already had.
                    exit_country = facts.country.or(exit_country);
                    exit_timezone = facts.timezone.or(exit_timezone);
                    exit_location = facts.location.or(exit_location);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "could not resolve the proxy exit");
                }
            }
        }

        let timezone = profile.timezone.clone().unwrap_or_else(|| {
            // UTC only when the exit could not be reached at all. It is a guess,
            // and the log says so rather than letting a launch look like it
            // followed something.
            exit_timezone.clone().unwrap_or_else(|| {
                tracing::warn!(
                    profile = %profile.name,
                    "launching with UTC: this profile follows its exit and the exit did not answer"
                );
                "UTC".into()
            })
        });

        // What a Chrome installed in that country would say. Not our opinion of
        // it: `fury_shared::locale` is generated from Chromium's own shipped
        // locales and its own IDS_ACCEPT_LANGUAGES values, so a German exit
        // gets `de-DE,de,en-US,en` because that is the string Chrome ships for
        // a German install — including the English tail, which is why this does
        // not make the profile look like a monolingual German.
        let exit_locale = exit_country
            .as_deref()
            .and_then(fury_shared::locale::for_country);
        if want_languages && exit_locale.is_none() {
            tracing::warn!(
                profile = %profile.name,
                country = ?exit_country,
                "launching with en-US: this profile follows its exit and the exit country \
                 is unknown or has no locale row"
            );
        }
        let locale = exit_locale.unwrap_or(fury_shared::locale::FALLBACK);

        let languages = profile.languages.clone().unwrap_or_else(|| {
            locale.languages.iter().map(|s| (*s).to_string()).collect()
        });

        // The UI locale for --lang, which is how a real Chrome ends up with a
        // matching ICU default. A profile that pins its own languages picks the
        // UI locale from the first of them, so the two never disagree; see
        // `ui_locale_for`.
        let ui_locale = match &profile.languages {
            Some(langs) => fury_shared::locale::ui_locale_for(langs.first().map(|s| s.as_str())),
            None => locale.ui.to_string(),
        };

        let ctx = fury_shared::persona::ProfileContext {
            timezone,
            languages,
            ui_locale: ui_locale.clone(),
            // Same lookup as the timezone, so the two cannot disagree.
            geolocation: exit_location.as_deref().and_then(parse_location),
            chrome_major: crate::CHROME_MAJOR,
            chrome_full_version: crate::CHROME_FULL_VERSION.to_string(),
        };
        let config = persona.derive_core_config(profile.fp_seed as u64, &ctx);

        // Local mode has no roles to enforce: whoever can reach this socket
        // owns the machine and the data. Restrictions are a team-server concept
        // and are computed there.
        let restrictions = fury_shared::rbac::LaunchRestrictions::for_perms(
            fury_shared::rbac::PermSet::full_profile_work(),
        );
        // Asked for, and allowed. A profile whose restrictions deny CDP passes
        // no flag, so waiting for a file the browser will never write would
        // stall every such launch for the length of the timeout.
        let wants_cdp = cdp && !restrictions.deny_cdp;

        let dir = paths::profile_dir(&profile.id);
        std::fs::create_dir_all(&dir)?;
        // Left behind by the previous run, and indistinguishable from this
        // one's once the browser is up.
        let _ = std::fs::remove_file(dir.join("DevToolsActivePort"));

        // Pull before launching, not after. A browser started on stale data and
        // then overwritten mid-session is worse than one that waited: the pages
        // it already loaded belong to the old cookie jar.
        let sealer = match profile_key {
            Some(key) => crate::bundle::Sealer::Shared {
                key,
                vault: Some(self.store.vault()),
            },
            None => crate::bundle::Sealer::Machine(self.store.vault()),
        };

        // The key the browser will encrypt this profile's cookies with, and the
        // guard that stops a wrong one destroying them.
        //
        // Only for a profile that has a key of its own — a team profile. A local
        // one keeps using the machine's keychain, which is what it does today
        // and which is correct for something that never leaves.
        //
        // The guard matters more than the key. Chromium does not skip a cookie
        // row it cannot decrypt: it deletes every row in that eTLD+1 group, and
        // `stop` then packs the emptied jar and pushes it. So a mismatch has to
        // be caught HERE, before the browser sees the directory, because after
        // that the damage is done and saved. Refusing to launch is the only
        // answer that leaves the data intact.
        let os_crypt_key = profile_key
            .map(|k| fury_shared::keys::os_crypt_key(&k, &profile.id));
        let key_marker = dir.join("Fury Key Id");
        if let Some(key) = os_crypt_key.as_ref() {
            let want = fury_shared::keys::key_fingerprint(key);
            if let Ok(found) = std::fs::read_to_string(&key_marker) {
                let found = found.trim();
                if !found.is_empty() && found != want {
                    anyhow::bail!(
                        "this profile's cookies were encrypted with a different key, and \
                         opening it now would make the browser delete them. That happens when \
                         the organisation key has been replaced since the profile was last \
                         used. Ask an owner to hand you the current key, then try again"
                    );
                }
            }
        }

        let mut pulled_version = 0;
        if let Some(srv) = server.as_ref() {
            match srv.fetch_bundle(&profile.id).await? {
                Some((bytes, wrapped, version)) => {
                    crate::bundle::unpack(&bytes, &wrapped, &sealer, &dir)?;
                    pulled_version = version;
                    tracing::info!(profile = %profile.name, version, "pulled bundle");
                }
                // A profile shared before it was ever opened has a row and no
                // bytes. Starting empty is correct; refusing would make the
                // first launch of every shared profile fail.
                None => tracing::info!(profile = %profile.name, "no bundle on the server yet"),
            }
        }

        // Start URLs open on the FIRST launch and never again.
        //
        // The browser now restores its previous session (see launcher.rs), and
        // start URLs passed on top of a restored session are added to it rather
        // than replacing it — so the tenth launch of a profile opened its start
        // page for the tenth time and the operator closed nine tabs by hand.
        // Measured: two launches, one extra copy; the pattern is obvious from
        // there.
        //
        // "Has it been opened before" is asked of the directory rather than of
        // a flag in the database, for the same reason `running` is: a flag can
        // be wrong after a crash or after a bundle is unpacked from a
        // colleague's machine, and the Sessions directory is the thing the
        // browser itself will be reading a moment later.
        let start_urls = start_urls_for(&dir, &profile.start_urls);

        let child = launcher::spawn(&launcher::LaunchSpec {
            core_binary: &core,
            user_data_dir: &dir,
            config: &config,
            relay_port,
            // Local mode has no roles to enforce: whoever can reach this socket
            // owns the machine and the data. Restrictions are a team-server
            // concept and are computed there.
            restrictions: restrictions.clone(),
            start_urls: &start_urls,
            os_crypt_key,
            ui_locale: &ui_locale,
            // Port 0 asks Chromium to pick one and write it to
            // DevToolsActivePort in the profile directory. Choosing a free port
            // here instead would race: something else can take it between the
            // check and the exec, and the failure lands as a browser that
            // started without CDP for no visible reason.
            debug_port: wants_cdp.then_some(0),
        })?;

        let pid = child.id();

        // The address a script attaches to, if one was asked for.
        //
        // Read from the file rather than guessed: line one is the port, line two
        // is the browser's WebSocket path, and both are what Puppeteer's
        // connect_over_cdp wants. Absent after a second and a half means the
        // core refused the flag — deny_cdp does exactly that — and the answer
        // says so by carrying no endpoint rather than a wrong one.
        let cdp_endpoint = if wants_cdp {
            devtools_endpoint(&dir).await
        } else {
            None
        };
        // Only for a profile this machine owns. A team profile's last-opened
        // time belongs to the server, which already records the launch in its
        // audit log when the lock is taken.
        if !from_server {
            self.store.touch_opened(&profile.id).await?;
        }

        let heartbeat = match (server.as_ref(), lock_token.as_ref()) {
            (Some(srv), Some(token)) => Some(crate::sync::keep_alive(
                srv.clone(),
                profile.id.clone(),
                token.clone(),
            )),
            _ => None,
        };

        self.running.lock().await.insert(
            profile.id.clone(),
            Running {
                child,
                relay_port,
                ws_endpoint: cdp_endpoint.as_ref().map(|(_, ws)| ws.clone()),
                relay: relay_task,
                server: server.map(|s| (s, pulled_version)),
                profile_key,
                lock_token,
                heartbeat,
            },
        );

        // Written only once the browser is actually up: a marker recorded for a
        // launch that failed would refuse the next, correct one.
        //
        // It goes inside the profile directory, so it travels in the bundle. A
        // colleague unpacking it finds the fingerprint of the key they are about
        // to derive — matching, when they hold the current organisation key, and
        // not matching when the key has been replaced since. The second case is
        // the one worth catching, and this is what catches it.
        if let Some(key) = os_crypt_key.as_ref() {
            let id = fury_shared::keys::key_fingerprint(key);
            if let Err(e) = std::fs::write(&key_marker, &id) {
                tracing::warn!(error = %e, "could not record the cookie key id");
            }
        }

        tracing::info!(profile = %profile.name, pid, relay_port, "launched");
        let mut out = serde_json::json!({ "pid": pid, "relay_port": relay_port });
        if let Some((port, ws)) = cdp_endpoint {
            out["debug_port"] = serde_json::json!(port);
            out["ws_endpoint"] = serde_json::json!(ws);
            // Both names, because the two libraries people actually use spell
            // it differently and neither accepts the other's.
            out["ws"] = serde_json::json!({ "puppeteer": ws, "selenium": ws });
        }
        Ok(out)
    }

    async fn stop(&self, profile_id: &str) -> anyhow::Result<serde_json::Value> {
        // Taken out of the map before the upload, which can take a while: the
        // list must stop reporting it as open the moment the browser is gone.
        let mut entry = {
            let mut running = self.running.lock().await;
            match running.remove(profile_id) {
                Some(e) => e,
                None => return Ok(serde_json::json!({ "stopped": false })),
            }
        };
        // Terminate, not kill. Chromium flushes its cookie jar on exit, and a
        // profile directory taken from a SIGKILLed browser holds the last write
        // it happened to manage rather than the session the operator just
        // finished.
        //
        // This used to apply only when there was a server to upload to, on the
        // reasoning that a local profile has nothing to push. It has something
        // to LOSE: the directory is the account. Measured — an import that
        // wrote two cookies over CDP and closed the profile exported zero
        // afterwards, because the browser never got to write them down. Local
        // profiles were quietly dropping the tail of every session.
        #[cfg(unix)]
        unsafe {
            libc::kill(entry.child.id() as i32, libc::SIGTERM);
        }
        // Bounded: a browser that will not close must not hold the UI.
        for _ in 0..50 {
            if entry.child.try_wait()?.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        let _ = entry.child.kill();
        let _ = entry.child.wait();

        // The lock stops being renewed the moment the browser is gone, not when
        // the next request happens to notice.
        if let Some(beat) = entry.heartbeat.take() {
            beat.abort();
        }

        let mut pushed = serde_json::Value::Null;
        if let Some((srv, base)) = entry.server.take() {
            // A server launch always carries a lock; the pair is set together at
            // launch. Refusing here rather than uploading without one keeps the
            // impossible case from becoming an unauthenticated write.
            let token = entry.lock_token.take().ok_or_else(|| {
                anyhow::anyhow!("this profile was launched against a server without a lock")
            })?;
            let sealer = match entry.profile_key {
                Some(key) => crate::bundle::Sealer::Shared {
                    key,
                    vault: Some(self.store.vault()),
                },
                None => crate::bundle::Sealer::Machine(self.store.vault()),
            };
            let sealed = crate::bundle::pack(&paths::profile_dir(profile_id), &sealer)?;
            let version = srv
                .push_bundle(
                    profile_id,
                    &sealed.bytes,
                    &sealed.wrapped_key,
                    &sealed.sha256,
                    base,
                    &token,
                )
                .await?;
            pushed = serde_json::json!(version);
        }
        // The exit closes with the profile. Leaving it up would keep a port
        // forwarding to the customer's proxy with no browser behind it.
        entry.relay.abort();
        Ok(serde_json::json!({ "stopped": true, "uploaded_version": pushed }))
    }
}

/// Where Chromium wrote the port it chose, and the browser's WebSocket path.
///
/// Polled rather than waited on once: the file appears a moment after the
/// process does, and how long that takes depends on whether the profile
/// directory is cold.
async fn devtools_endpoint(profile_dir: &std::path::Path) -> Option<(u16, String)> {
    let path = profile_dir.join("DevToolsActivePort");
    // Twenty seconds, because that is what a cold start costs: a fresh profile
    // directory, a first run, and a machine doing other things. Measured at
    // about six on a warm one, and the first version waited one and a half and
    // reported no endpoint for a browser that was about to publish one.
    //
    // The file survives a previous run, so a stale one would be read as this
    // run's. Removed before the browser starts — see `launch`.
    for _ in 0..200 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut lines = text.lines();
        let (Some(port), Some(ws_path)) = (lines.next(), lines.next()) else {
            continue;
        };
        let Ok(port) = port.trim().parse::<u16>() else {
            continue;
        };
        return Some((port, format!("ws://127.0.0.1:{port}{}", ws_path.trim())));
    }
    None
}

fn str_param(params: &serde_json::Value, name: &str) -> anyhow::Result<String> {
    params
        .get(name)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing parameter {name:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coordinate_is_taken_or_refused_but_never_repaired() {
        assert_eq!(parse_location("60.1695,24.9354"), Some((60.1695, 24.9354)));
        // ipinfo sends it with no space; other checkers pad it. Both are the
        // same place and both must parse, because a profile that silently
        // reports the host machine's position because of a space is exactly the
        // failure this whole field exists to prevent.
        assert_eq!(parse_location("  -33.87, 151.21 "), Some((-33.87, 151.21)));
        assert_eq!(parse_location("0,0"), Some((0.0, 0.0)));

        // Refused rather than clamped or reordered. A checker that answers with
        // a longitude where the latitude goes is a checker we do not understand,
        // and using half of what it said is how a profile ends up somewhere its
        // address is not.
        assert_eq!(parse_location("900,10"), None);
        assert_eq!(parse_location("24.9354,60.1695,7"), None);
        assert_eq!(parse_location("60.1695"), None);
        assert_eq!(parse_location("Helsinki"), None);
        assert_eq!(parse_location(""), None);
    }

    #[test]
    fn start_urls_open_once_and_then_the_session_takes_over() {
        let tmp = std::env::temp_dir().join(format!("fury-su-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // First launch: what the profile asks for.
        assert_eq!(
            start_urls_for(&tmp, &["https://shop.example/".to_string()]),
            vec!["https://shop.example/".to_string()]
        );
        // First launch with nothing configured: the page that proves the
        // disguise and the exit are working.
        assert_eq!(start_urls_for(&tmp, &[]), vec![crate::relay::START_URL.to_string()]);

        // Once the browser has been here, the session is the answer and passing
        // anything would stack a duplicate on top of it.
        std::fs::create_dir_all(tmp.join("Default").join("Sessions")).unwrap();
        assert!(start_urls_for(&tmp, &["https://shop.example/".to_string()]).is_empty());
        assert!(start_urls_for(&tmp, &[]).is_empty());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_missing_parameter_is_named() {
        let err = str_param(&serde_json::json!({}), "project_id").unwrap_err();
        assert!(err.to_string().contains("project_id"));
    }

    #[test]
    fn responses_omit_the_half_that_did_not_happen() {
        // The shell distinguishes success from failure by which key is present,
        // so serialising both would make every error look like a success.
        let ok = Response { id: 1, ok: Some(serde_json::json!({})), err: None };
        let text = serde_json::to_string(&ok).unwrap();
        assert!(text.contains("\"ok\""));
        assert!(!text.contains("\"err\""));

        let bad = Response { id: 2, ok: None, err: Some("no".into()) };
        let text = serde_json::to_string(&bad).unwrap();
        assert!(text.contains("\"err\""));
        assert!(!text.contains("\"ok\""));
    }
}
