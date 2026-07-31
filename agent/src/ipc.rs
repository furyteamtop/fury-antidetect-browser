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

impl Agent {
    pub async fn new() -> anyhow::Result<Arc<Self>> {
        paths::ensure_data_dir()?;
        let store = Store::open(&paths::db_path()).await?;
        // Every installation starts with somewhere to put a profile.
        store.default_project().await?;
        // Warm the keychain on its own thread. If the OS wants to ask about it,
        // it asks while the agent is already listening rather than instead of.
        store.vault_handle().prime();

        Ok(Arc::new(Self {
            store,
            running: Mutex::new(HashMap::new()),
            core: crate::core_binary(),
        }))
    }

    /// Listen until the process is stopped.
    pub async fn serve(self: Arc<Self>) -> anyhow::Result<()> {
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
                        .record_exit(id, s("ip"), s("country"), s("timezone"))
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
                let running = self.running.lock().await;
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
                let ctx = fury_shared::persona::ProfileContext {
                    timezone: params
                        .get("timezone")
                        .and_then(|v| v.as_str())
                        .unwrap_or("UTC")
                        .to_string(),
                    languages: params
                        .get("languages")
                        .and_then(|v| v.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_else(|| vec!["en-US".into(), "en".into()]),
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
                self.launch(&str_param(&params, "id")?, server, lock_token, inline, profile_key)
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
    ) -> anyhow::Result<(Option<String>, Option<String>, Option<String>)> {
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
        Ok((s("ip"), s("country"), s("timezone")))
    }

    async fn launch(
        &self,
        profile_id: &str,
        server: Option<crate::sync::Server>,
        lock_token: Option<String>,
        inline: Option<Profile>,
        profile_key: Option<[u8; 32]>,
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
            anyhow::anyhow!(
                "no core binary found. Build it, or point FURY_CORE at one"
            )
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

        // A profile with no timezone of its own follows its exit.
        //
        // Claiming UTC while leaving through Berlin is the cheapest thing a
        // detector checks: one subtraction between
        // Intl.DateTimeFormat().resolvedOptions().timeZone and the geolocation
        // of the address the request came from. The stored answer is used when
        // there is one, and asked for when there is not — a second in front of
        // a launch is worth less than an account.
        let timezone = match profile.timezone.clone() {
            Some(tz) => tz,
            None => {
                let known = proxy.last_timezone.clone();
                let resolved = match known {
                    Some(tz) => Some(tz),
                    None => match Self::resolve_exit(&proxy.url(), proxy.checker_url.as_deref()).await {
                        Ok((ip, country, tz)) => {
                            let _ = self
                                .store
                                .record_exit(&proxy.id, ip.as_deref(), country.as_deref(), tz.as_deref())
                                .await;
                            tz
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "could not resolve the proxy exit");
                            None
                        }
                    },
                };
                // UTC only when the exit could not be reached at all. It is a
                // guess, and the log says so rather than letting a launch look
                // like it followed something.
                resolved.unwrap_or_else(|| {
                    tracing::warn!(
                        profile = %profile.name,
                        "launching with UTC: this profile follows its exit and the exit did not answer"
                    );
                    "UTC".into()
                })
            }
        };

        let ctx = fury_shared::persona::ProfileContext {
            timezone,
            languages: profile
                .languages
                .clone()
                .unwrap_or_else(|| vec!["en-US".into(), "en".into()]),
            chrome_major: crate::CHROME_MAJOR,
            chrome_full_version: crate::CHROME_FULL_VERSION.to_string(),
        };
        let config = persona.derive_core_config(profile.fp_seed as u64, &ctx);

        let dir = paths::profile_dir(&profile.id);
        std::fs::create_dir_all(&dir)?;

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

        // Nothing configured means the start page, not a blank tab: the first
        // thing an operator needs after opening a profile is proof that the
        // disguise and the exit are working.
        let start_urls = if profile.start_urls.is_empty() {
            vec![crate::relay::START_URL.to_string()]
        } else {
            profile.start_urls.clone()
        };

        let child = launcher::spawn(&launcher::LaunchSpec {
            core_binary: &core,
            user_data_dir: &dir,
            config: &config,
            relay_port,
            // Local mode has no roles to enforce: whoever can reach this socket
            // owns the machine and the data. Restrictions are a team-server
            // concept and are computed there.
            restrictions: fury_shared::rbac::LaunchRestrictions::for_perms(
                fury_shared::rbac::PermSet::full_profile_work(),
            ),
            start_urls: &start_urls,
            debug_port: None,
        })?;

        let pid = child.id();
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
                relay: relay_task,
                server: server.map(|s| (s, pulled_version)),
                profile_key,
                lock_token,
                heartbeat,
            },
        );

        tracing::info!(profile = %profile.name, pid, relay_port, "launched");
        Ok(serde_json::json!({ "pid": pid, "relay_port": relay_port }))
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
        // Terminate, not kill, when there is something to save. Chromium flushes
        // its cookie jar on exit, and a bundle packed from a SIGKILLed profile
        // is the last write the browser managed rather than the session the
        // operator just finished. Without a server there is nothing to upload,
        // so the old, faster path stands.
        if entry.server.is_some() {
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
