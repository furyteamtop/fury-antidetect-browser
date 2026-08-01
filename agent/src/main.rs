//! Fury agent — the local daemon.
//!
//! Runs on the operator's machine and is the only component that ever holds
//! decrypted secrets. Owns the proxy relays, launches the core, keeps profile
//! locks alive, and serves the local automation API.
//!
//! Only the pieces that stand on their own today are wired up; see
//! docs/09-roadmap.md for what is still missing.

mod bundle;
mod http;
mod ipc;
mod launcher;
mod paths;
mod personas;
mod relay;
mod store;
mod sync;
#[cfg(test)]
mod tmp;
mod transfer;
mod vault;

use std::time::Duration;

use fury_shared::fingerprint::samples;
use relay::{Credentials, Relay, Upstream};

/// The port for the local automation API, or `None` when it is off.
///
/// `FURY_API_PORT=35000` turns it on; `0` and anything unparseable leave it
/// off, so a typo fails closed.
fn api_port() -> Option<u16> {
    std::env::var("FURY_API_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|p| *p > 0)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fury_agent=info".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") => {
            let agent = ipc::Agent::new().await?;

            // Only when asked for. See http.rs: a loopback port is reachable
            // from a page the browser itself opens, and the defences are worth
            // nothing on a machine whose owner never wanted the port.
            if let Some(port) = api_port() {
                let for_api = std::sync::Arc::clone(&agent);
                tokio::spawn(async move {
                    if let Err(e) = http::serve(for_api, port).await {
                        tracing::error!(error = %e, "the local API stopped");
                    }
                });
            }

            agent.serve().await
        }
        Some("relay") => cmd_relay(&args[1..]).await,
        Some("launch") => cmd_launch(&args[1..]).await,
        Some("check-fingerprint") => cmd_check_fingerprint(&args[1..]),
        _ => {
            eprintln!(
                "fury-agent {}\n\
                 \n\
                 USAGE:\n  \
                   fury-agent serve\n      \
                     Run the local daemon: profiles, proxies, launching.\n      \
                     This is what the desktop app talks to.\n  \
                   fury-agent relay <upstream-url> [--port N]\n      \
                     Start a profile relay. Upstream may be:\n        \
                       http://user:pass@host:port\n        \
                       socks5://user:pass@host:port\n  \
                   fury-agent launch <persona.json> [options]\n      \
                     Derive a profile from a persona and run the core on it.\n        \
                       --seed N            profile seed (default 1)\n        \
                       --proxy URL         upstream proxy; everything goes through it\n        \
                       --timezone ZONE     IANA zone the profile reports\n        \
                       --country CC        ISO country: sets --lang and the UI locale\n                              \
                                           to what a Chrome installed there sends\n        \
                       --lang a,b          BCP-47 list, most preferred first\n        \
                       --profile-dir DIR   user-data-dir (default a temp dir)\n        \
                       --core PATH         core binary (or set FURY_CORE)\n        \
                       --url URL           page to open\n        \
                       --debug-port N      expose CDP on 127.0.0.1:N\n  \
                   fury-agent check-fingerprint <config.json>\n      \
                     Validate a fingerprint config for internal consistency.\n      \
                     Pass '-' to check the built-in sample.\n",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
    }
}

async fn cmd_relay(args: &[String]) -> anyhow::Result<()> {
    let url = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing upstream url"))?;
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);

    let upstream = parse_upstream(url)?;
    tracing::info!(?upstream, "starting relay");

    let (bound, handle) = Relay::new(upstream).serve(port).await?;
    tracing::info!(
        port = bound,
        "relay listening — launch the core with --proxy-server=http://127.0.0.1:{bound}"
    );

    // Kill-switch note: if the upstream dies, sessions fail closed. The relay
    // stays up so the browser keeps failing rather than silently going direct.
    handle.await?;
    Ok(())
}

/// Derive a profile from a persona and run the core on it.
///
/// This is the whole launch path from docs/01 minus the parts that need a
/// server: acquire the exit, decide what the profile claims about itself, and
/// hand the core a config it never lets out of the process. Bundle sync and the
/// distributed lock join later; neither is needed to open a browser.
async fn cmd_launch(args: &[String]) -> anyhow::Result<()> {
    let persona_path = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or_else(|| anyhow::anyhow!("missing persona path"))?;

    let opt = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let persona: fury_shared::persona::Persona =
        serde_json::from_str(&std::fs::read_to_string(persona_path)?)?;
    let seed: u64 = opt("--seed").map_or(Ok(1), |s| s.parse())?;

    // Start the exit first. The timezone a profile reports has to match where it
    // actually leaves the network (docs/05), so the proxy has to exist before
    // the config is derived, not after.
    let relay = match opt("--proxy") {
        Some(url) => {
            let upstream = parse_upstream(&url)?;
            tracing::info!(?upstream, "starting relay");
            let (port, handle) = Relay::new(upstream).serve(0).await?;
            Some((port, handle))
        }
        None => None,
    };
    let relay_port = match &relay {
        Some((port, _)) => *port,
        None => {
            // Without a proxy the core would still be pointed at a relay that is
            // not there, and every request would fail. Refuse rather than
            // quietly launching a browser that goes out on the real IP.
            anyhow::bail!(
                "--proxy is required: the core is launched with --proxy-server pointing at the \
                 relay, and running without one would send traffic from this machine's own IP"
            )
        }
    };

    // `--country DE` is the short way to say all of it: the languages a German
    // Chrome sends and the UI locale it runs as, straight out of the generated
    // table. `--lang` still overrides, for testing a combination on purpose.
    let country_locale = opt("--country")
        .as_deref()
        .and_then(fury_shared::locale::for_country);
    let locale = country_locale.unwrap_or(fury_shared::locale::FALLBACK);

    let languages: Vec<String> = match opt("--lang") {
        Some(l) => l.split(',').map(str::trim).map(str::to_string).collect(),
        None => locale.languages.iter().map(|s| (*s).to_string()).collect(),
    };
    // Follows the languages, whether they came from --country or --lang, so a
    // hand-run launch cannot end up formatting dates in a language it does not
    // claim to speak.
    let ui_locale = match (&country_locale, opt("--lang")) {
        (Some(l), None) => l.ui.to_string(),
        _ => fury_shared::locale::ui_locale_for(languages.first().map(|s| s.as_str())),
    };

    let ctx = fury_shared::persona::ProfileContext {
        // TODO: resolve from the relay's effective exit IP, per docs/05. Until
        // that lands the operator has to say, and a mismatch is on them. The
        // launch path in ipc.rs does resolve it; this is the bare CLI.
        timezone: opt("--timezone").unwrap_or_else(|| "Europe/Berlin".into()),
        languages,
        ui_locale: ui_locale.clone(),
        chrome_major: CHROME_MAJOR,
        chrome_full_version: CHROME_FULL_VERSION.to_string(),
    };

    // Fail closed before deriving: a config is a pure function of the persona,
    // so an inconsistent persona is the only way to get an inconsistent profile.
    if let Err(errs) = persona.validate() {
        anyhow::bail!(
            "persona {} is inconsistent and would stand out:\n  {}",
            persona.id,
            errs.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("\n  ")
        );
    }
    let config = persona.derive_core_config(seed, &ctx);

    let core = opt("--core")
        .or_else(|| std::env::var("FURY_CORE").ok())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("no core binary: pass --core or set FURY_CORE"))?;

    let profile_dir = opt("--profile-dir").map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::env::temp_dir().join(format!("fury-profile-{}-{seed}", persona.id))
    });
    std::fs::create_dir_all(&profile_dir)?;

    let urls: Vec<String> = opt("--url").into_iter().collect();

    let spec = launcher::LaunchSpec {
        core_binary: &core,
        user_data_dir: &profile_dir,
        config: &config,
        relay_port,
        // A CLI launch is the operator working on their own machine, so nothing
        // is withheld. The server decides this when a profile comes from a team.
        restrictions: fury_shared::rbac::LaunchRestrictions::for_perms(
            fury_shared::rbac::PermSet::full_profile_work(),
        ),
        start_urls: &urls,
        ui_locale: &ui_locale,
        // Opt-in: CDP is full cookie access, so it is never on by default even
        // for a local launch.
        debug_port: opt("--debug-port").and_then(|p| p.parse().ok()),
    };

    let mut child = launcher::spawn(&spec)?;
    tracing::info!(
        pid = child.id(),
        persona = %persona.id,
        seed,
        relay_port,
        timezone = %ctx.timezone,
        dir = %profile_dir.display(),
        "core running"
    );

    let status = tokio::task::spawn_blocking(move || child.wait()).await??;
    tracing::info!(?status, "core exited");
    Ok(())
}

/// Where the core binary is, if it can be found.
///
/// Looked up rather than configured, because the common case is that the app
/// bundle ships one next to the agent. FURY_CORE overrides for a development
/// tree, where the build output is somewhere only the developer knows.
pub fn core_binary() -> Option<std::path::PathBuf> {
    if let Ok(explicit) = std::env::var("FURY_CORE") {
        let path = std::path::PathBuf::from(explicit);
        return path.exists().then_some(path);
    }
    let beside = std::env::current_exe().ok()?.parent()?.join(
        if cfg!(target_os = "macos") { "Fury.app/Contents/MacOS/Fury" } else { "fury-core" },
    );
    beside.exists().then_some(beside)
}

/// The core this agent expects to drive. Read from core/CHROMIUM_VERSION at
/// build time would be better; hard-coded until the two are built together.
pub const CHROME_MAJOR: u32 = 150;
pub const CHROME_FULL_VERSION: &str = "150.0.7871.187";

fn cmd_check_fingerprint(args: &[String]) -> anyhow::Result<()> {
    let path = args
        .first()
        .ok_or_else(|| anyhow::anyhow!("missing config path (or '-' for the sample)"))?;

    let config = if path == "-" {
        samples::macos_arm64()
    } else {
        serde_json::from_str(&std::fs::read_to_string(path)?)?
    };

    match config.validate() {
        Ok(()) => {
            println!("consistent — no contradictions found");
            Ok(())
        }
        Err(errs) => {
            eprintln!("{} inconsistency(ies) — this profile would stand out:", errs.len());
            for e in &errs {
                eprintln!("  - {e}");
            }
            std::process::exit(1);
        }
    }
}

/// Parse `scheme://[user:pass@]host:port`.
///
/// Deliberately hand-rolled rather than pulled from a URL crate: proxy strings
/// in the wild contain characters real URL parsers reject, and silently
/// mis-parsing one means connecting somewhere unintended.
pub fn parse_upstream(url: &str) -> anyhow::Result<Upstream> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| anyhow::anyhow!("expected scheme://, got {url:?}"))?;

    let (auth, hostport) = match rest.rsplit_once('@') {
        Some((a, hp)) => {
            let (u, p) = a
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("credentials must be user:pass"))?;
            (
                Some(Credentials {
                    username: u.to_string(),
                    password: p.to_string(),
                }),
                hp,
            )
        }
        None => (None, rest),
    };

    let (host, port) = hostport
        .rsplit_once(':')
        .ok_or_else(|| anyhow::anyhow!("expected host:port"))?;
    let port: u16 = port.parse()?;
    let host = host.to_string();

    Ok(match scheme {
        "http" | "https" => Upstream::Http { host, port, auth },
        // socks5 is treated as socks5h unconditionally: local DNS resolution
        // would leak the operator's resolver. See docs/05.
        "socks5" | "socks5h" => Upstream::Socks5 { host, port, auth },
        other => anyhow::bail!("unsupported proxy scheme {other:?}"),
    })
}

#[allow(dead_code)]
const LOCK_HEARTBEAT: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_authenticated_socks5() {
        let u = parse_upstream("socks5://bob:s3cr3t@10.0.0.1:1080").unwrap();
        match u {
            Upstream::Socks5 { host, port, auth } => {
                assert_eq!(host, "10.0.0.1");
                assert_eq!(port, 1080);
                let a = auth.unwrap();
                assert_eq!(a.username, "bob");
                assert_eq!(a.password, "s3cr3t");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn passwords_may_contain_at_signs() {
        // rsplit_once('@') matters: 'p@ss' would otherwise split in the wrong place.
        let u = parse_upstream("http://bob:p@ss@proxy.example:8080").unwrap();
        match u {
            Upstream::Http { host, auth, .. } => {
                assert_eq!(host, "proxy.example");
                assert_eq!(auth.unwrap().password, "p@ss");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn anonymous_proxies_parse() {
        assert!(matches!(
            parse_upstream("socks5://127.0.0.1:9050").unwrap(),
            Upstream::Socks5 { auth: None, .. }
        ));
    }

    #[test]
    fn unknown_schemes_are_refused() {
        assert!(parse_upstream("ftp://x:1").is_err());
        assert!(parse_upstream("no-scheme:1080").is_err());
    }

    #[test]
    fn the_sample_persona_is_self_consistent() {
        samples::macos_arm64().validate().unwrap();
    }
}
