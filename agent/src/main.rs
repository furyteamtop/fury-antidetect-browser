// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Fury agent — the local daemon.
//!
//! Runs on the operator's machine and is the only component that ever holds
//! decrypted secrets. Owns the proxy relays, launches the core, keeps profile
//! locks alive, and serves the local automation API.
//!
//! Only the pieces that stand on their own today are wired up; see
//! docs/09-roadmap.md for what is still missing.

mod blocklist;
mod bundle;
mod cookies;
mod core_download;
mod ext;
mod http;
mod import_browser;
mod install_core;
mod ipc;
mod launcher;
mod paths;
mod personas;
mod relay;
mod store;
mod sync;
#[cfg(test)]
mod sync_tests;
#[cfg(test)]
mod tmp;
mod transfer;
mod usage;
mod vault;
mod widevine;
mod wg_stack;
mod wg_tunnel;
mod wireguard;

use std::time::Duration;

use fury_shared::fingerprint::samples;
use relay::{Credentials, Relay, Upstream};

/// The port for the local automation API, or `None` when it is off.
///
/// `FURY_API_PORT=35000` turns it on; `0` and anything unparseable leave it
/// off, so a typo fails closed.
///
/// There is no default port on purpose. A local HTTP port is a way into every
/// logged-in profile on the machine, and it should exist because somebody asked
/// for it, not because they installed something. [`http::DEFAULT_PORT`] is the
/// number to use when you do — see examples/.
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
        Some("install-core") => match args.get(1) {
            Some(src) => install_core::install(std::path::Path::new(src)).map(|_| ()),
            None => install_core::status(),
        },
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
                       --geo lat,lng       position the profile reports\n        \
                       --profile-dir DIR   user-data-dir (default a temp dir)\n        \
                       --core PATH         core binary (or set FURY_CORE)\n        \
                       --url URL           page to open\n        \
                       --debug-port N      expose CDP on 127.0.0.1:N\n  \
                   fury-agent check-fingerprint <config.json>\n      \
                     Validate a fingerprint config for internal consistency.\n      \
                     Pass '-' to check the built-in sample.\n  \
                   fury-agent install-core [file]\n      \
                     Install a downloaded core, or report the installed one.\n      \
                     Takes a .tar.xz, .tar.gz, .app or directory — not a URL.\n",
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
        // The bare CLI resolves no exit; --geo takes "lat,lng" for testing one
        // on purpose.
        geolocation: opt("--geo").as_deref().and_then(ipc::parse_location),
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

    // The directory is named after a digest of the persona and seed, not after
    // the persona itself, and that is not tidiness.
    //
    // It used to be `fury-profile-{persona.id}-{seed}`, and persona ids describe
    // the machine they imitate: windows-11-rtx4060-1920x1080. The directory ends
    // up in --user-data-dir, Chromium passes that switch to EVERY child process,
    // and a command line is readable by any process this user runs. So the whole
    // point of carrying the config in an inherited handle -- that argv holds a
    // slot number and nothing else -- was undone by the folder it ran in.
    //
    // Measured 16.08.2026 on Windows: eight processes, and seven of them
    // advertised "rtx4060-1920x1080" in argv while the browser process itself
    // was clean. Found by the verify-windows section written to prove the
    // opposite, which is what that section is for.
    //
    // Eight hex characters, the same shape the IPC endpoint tag uses. Same
    // persona and seed still means the same directory, so a repeated launch
    // reuses its profile rather than growing a new one each time.
    let profile_dir = opt("--profile-dir").map(std::path::PathBuf::from).unwrap_or_else(|| {
        use sha2::Digest;
        let mut h = sha2::Sha256::new();
        h.update(persona.id.as_bytes());
        h.update(b"\0");
        h.update(seed.to_le_bytes());
        let tag = h.finalize();
        std::env::temp_dir().join(format!(
            "fury-profile-{:02x}{:02x}{:02x}{:02x}",
            tag[0], tag[1], tag[2], tag[3]
        ))
    });
    std::fs::create_dir_all(&profile_dir)?;

    let urls: Vec<String> = opt("--url").into_iter().collect();

    let spec = launcher::LaunchSpec {
        core_binary: &core,
        user_data_dir: &profile_dir,
        config: &config,
        relay_port,
        // The CLI launch takes a persona file rather than a stored profile, so
        // there is no profile whose extensions these would be.
        extensions: &[],
        // A CLI launch is the operator working on their own machine, so nothing
        // is withheld. The server decides this when a profile comes from a team.
        restrictions: fury_shared::rbac::LaunchRestrictions::for_perms(
            fury_shared::rbac::PermSet::full_profile_work(),
        ),
        start_urls: &urls,
        // The bare CLI has no organisation key and no profile key, so there is
        // nothing to derive one from — the machine's own keychain keeps the
        // cookies, as it does for any profile that never leaves.
        os_crypt_key: None,
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
        if path.exists() {
            return Some(path);
        }
        // A setting that names nothing does NOT stop the search, and this is a
        // reversal of how it was written a day earlier.
        //
        // The argument then was that silently running a different browser from
        // the one the environment names is worse than running none. That is
        // true when the named binary exists and a different one gets used. It
        // is not this case: the named path does not exist, so there is no
        // browser it could have meant, and the choice is between a working
        // application and a dead one.
        //
        // What it cost, measured rather than imagined. A FURY_CORE left in
        // launchd from before the branding patch renamed Chromium.app made the
        // application unusable; the first fix could not work, because a GUI
        // process takes its environment from launchd and not from any shell;
        // and the second could not work either, because the agent is a separate
        // long-lived process that an application restart does not touch. Three
        // rounds, for a variable naming a file that is not there.
        //
        // So: carry on looking, and say so loudly. core_lookup_problem() keeps
        // reporting it, the desktop shows it in a bar, and this line puts it in
        // the log for anybody driving the CLI. Not silent, and not stuck.
        tracing::warn!(
            wanted = %path.display(),
            "FURY_CORE names a path that does not exist; looking for a core elsewhere"
        );
    }

    // Beside the agent first, because a development tree and a bundle that
    // shipped both together are both that shape, and an explicitly placed
    // binary should win over an installed one.
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(std::path::Path::to_path_buf)) {
        if let Some(path) = core_leaves().iter().map(|l| dir.join(l)).find(|p| p.exists()) {
            return Some(path);
        }
    }

    // Then the installed location. This is the ordinary case for anybody who
    // did not build the browser: the shell is 12 MB and downloads with the
    // application, the core is 134 MB and arrives separately.
    //
    // The directory gained a `.bundle` extension so that Spotlight stops
    // offering the browser as a second application called Fury; anything
    // installed before that has to be brought along, or it silently stops
    // being found.
    crate::install_core::migrate_legacy_dir();
    let dir = crate::paths::core_dir();
    core_leaves().iter().map(|l| dir.join(l)).find(|p| p.exists())
}

/// Something wrong with how the core is being found, in a sentence.
///
/// No longer a reason there is no core — [`core_binary`] carries on looking —
/// but still worth saying, because a FURY_CORE naming a deleted build is
/// somebody's leftover and will confuse them again next week.
pub fn core_lookup_problem() -> Option<String> {
    let explicit = std::env::var("FURY_CORE").ok()?;
    if std::path::Path::new(&explicit).exists() {
        return None;
    }
    let found = core_binary().is_some();
    Some(format!(
        "FURY_CORE is set to {explicit}, which does not exist. {} {}.",
        if found {
            "It is being ignored and the installed browser used instead."
        } else {
            "There is no installed browser to fall back to either."
        },
        how_to_unset(&explicit)
    ))
}

/// How to actually get rid of it, which is not always what it looks like.
///
/// "unset it" is true of a shell and useless to somebody running the desktop
/// application: a GUI process on macOS inherits its environment from launchd,
/// not from any shell, so `unset FURY_CORE` in Terminal changes nothing and the
/// application keeps failing in exactly the same way. `launchctl setenv` is how
/// the value gets there — a debugging command typed once and forgotten — and
/// `launchctl unsetenv` is the only thing that removes it.
///
/// Found the hard way: a FURY_CORE left over from before the branding patch
/// renamed Chromium.app to Fury.app, living in launchd and in no dotfile, so it
/// was invisible to every obvious place to look.
///
/// And then found the hard way a second time. "Restart Fury" was the next
/// sentence, and it was also wrong: the agent is a separate process that
/// deliberately outlives the window (docs/01 — it holds locks and serves the
/// automation API with the UI closed), so quitting and reopening the
/// application finds the same agent still holding the same environment. A
/// process cannot be told to forget a variable it started with; it has to end.
fn how_to_unset(current: &str) -> String {
    // Whatever the source, the agent has to be restarted for the change to
    // reach it, and that is the part everybody gets wrong.
    const THEN_RESTART: &str = "Then quit Fury and stop the agent — \
                                `pkill -f fury-agent` — because it is a separate \
                                process that an application restart does not touch";

    #[cfg(target_os = "macos")]
    {
        let from_launchd = std::process::Command::new("launchctl")
            .args(["getenv", "FURY_CORE"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim() == current)
            .unwrap_or(false);

        if from_launchd {
            return format!(
                "It comes from launchd, so `unset FURY_CORE` in a terminal will not \
                 reach this application — run `launchctl unsetenv FURY_CORE`. {THEN_RESTART}"
            );
        }
    }
    let _ = current;
    format!("Unset it, or point it at a core. {THEN_RESTART}")
}

/// The path from a directory holding a core to the executable inside it.
///
/// macOS wraps the browser in an application bundle; Windows is a directory
/// with the executable at the top. The `.exe` is not optional — `Path::exists`
/// is how the core is found, and a file named `fury` with no extension does not
/// exist under the name `fury.exe` that everything else would then look for.
fn core_leaf() -> &'static str {
    core_leaves()[0]
}

/// Every name a core may go by, most-preferred first.
///
/// The second name in each list is the UNBRANDED one, and it is here because a
/// core built from a clean checkout of this repository has it. Patch 0900 --
/// branding -- is not written: docs/09 says so, and it is blocked on icon
/// assets rather than on code.
///
/// What that meant in practice, found 16.08.2026 by installing a published
/// release onto a Windows machine:
///
///     Error: no core found in fury-core-0.1.0-pre2-windows-x64.tar.xz
///     -- expected fury.exe somewhere inside it
///
/// The Windows core is chrome.exe, because a clean checkout produces chrome.exe.
/// The macOS core is Fury.app only because the developer's Chromium tree carries
/// uncommitted edits to chrome/app/theme/chromium/BRANDING that no patch in this
/// repository reproduces -- so the macOS build was branded and the Windows build,
/// built from what is actually committed, was not. The agent worked on one
/// machine and could not have worked on any other, which is the same shape of
/// bug ci.yml already documents about the sidecar.
///
/// Accepting both is not a workaround for that. An agent's job is to drive the
/// core it was given, and refusing one over its filename would be refusing a
/// browser that is otherwise correct. Branding still has to become patch 0900,
/// and until it does this list is what makes a clean checkout usable.
fn core_leaves() -> &'static [&'static str] {
    #[cfg(target_os = "macos")]
    {
        &[
            "Fury.app/Contents/MacOS/Fury",
            "Chromium.app/Contents/MacOS/Chromium",
        ]
    }
    #[cfg(windows)]
    {
        &["fury.exe", "chrome.exe"]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        &["fury-core", "chrome"]
    }
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
    #[test]
    fn a_core_path_that_exists_is_not_a_problem() {
        // The guard that keeps this off the ordinary path: a FURY_CORE pointing
        // at a real binary is a normal development setup, not a fault.
        let real = std::env::current_exe().unwrap();
        // SAFETY: single-threaded test, and the variable is read back at once.
        unsafe { std::env::set_var("FURY_CORE", &real) };
        assert_eq!(super::core_lookup_problem(), None);
        unsafe { std::env::remove_var("FURY_CORE") };
    }

    #[test]
    fn a_core_path_that_does_not_exist_names_itself_and_a_way_out() {
        unsafe { std::env::set_var("FURY_CORE", "/nowhere/Chromium.app/Contents/MacOS/Chromium") };
        let said = super::core_lookup_problem().expect("a missing path is a problem");
        // The path, because "no core found" sends people to look in the wrong
        // place — which is exactly what happened before this existed.
        assert!(said.contains("/nowhere/Chromium.app"), "{said}");
        // Something to do about it.
        assert!(said.contains("unsetenv") || said.contains("Unset it"), "{said}");
        // And the part that took two failed attempts to get right: the agent
        // outlives the window, so "restart Fury" is not enough and the advice
        // must say what is.
        assert!(said.contains("pkill -f fury-agent"), "{said}");
        unsafe { std::env::remove_var("FURY_CORE") };
    }

    #[test]
    fn a_core_path_that_does_not_exist_does_not_block_the_search() {
        // The reversal. A variable naming a file that is not there used to make
        // the application unusable; now it is ignored and reported. Nothing
        // else could be found in a test environment either, so what is asserted
        // is that the variable itself is no longer the thing standing in the
        // way — the lookup proceeds past it rather than returning at it.
        unsafe { std::env::set_var("FURY_CORE", "/nowhere/at/all") };
        let with_bad_var = super::core_binary();
        unsafe { std::env::remove_var("FURY_CORE") };
        let without = super::core_binary();
        assert_eq!(
            with_bad_var, without,
            "a FURY_CORE pointing nowhere changed the outcome; it should be ignored"
        );
    }

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
