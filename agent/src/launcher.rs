// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Launching the patched core.
//!
//! # Why the fingerprint config is not passed on the command line
//!
//! Process arguments are readable by any other process on the machine (`ps`,
//! Process Explorer, and any native helper an anti-fraud vendor ships). A
//! command line containing the whole spoofed persona is a gift. The config
//! travels over an inherited pipe instead; only the descriptor number appears
//! in the arguments.

use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use fury_shared::rbac::LaunchRestrictions;

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("fingerprint config is inconsistent and would be a detection signal:\n{0}")]
    Inconsistent(String),
    #[error("core binary not found at {0}")]
    CoreMissing(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub struct LaunchSpec<'a> {
    pub core_binary: &'a Path,
    pub user_data_dir: &'a Path,

    /// The **core's** config, as produced by `Persona::derive_core_config`.
    ///
    /// Not `FingerprintConfig`. The two are different shapes and were never
    /// convertible: the core reads dotted camelCase paths — `audio.sampleRate`,
    /// `noise.canvasSeed`, `gpu.webglParams.RENDERER`, `clientHints.brands` —
    /// while `FingerprintConfig` is snake_case and has no client-hints or
    /// webglParams section at all. Handing it the wrong one is worse than
    /// handing it nothing: every lookup misses, every vector silently falls back
    /// to stock Chromium, and the profile *looks* configured. The contract is
    /// pinned by `core_config_covers_every_key_the_core_reads` in shared-rs.
    pub config: &'a serde_json::Value,
    pub relay_port: u16,
    pub restrictions: LaunchRestrictions,
    pub start_urls: &'a [String],
    /// Only set when the caller is allowed CDP at all; see `LaunchRestrictions`.
    pub debug_port: Option<u16>,

    /// The key the core encrypts this profile's cookies with, or `None` to
    /// leave that to the machine's own keychain.
    ///
    /// Present for every profile that has one, which after patch 0110 is every
    /// profile. The point is that it does NOT come from this machine: it is
    /// derived from the profile's own key material, so the ciphertext in the
    /// cookie jar can be read wherever the bundle can be opened. Without it, a
    /// colleague unpacking a shared profile cannot decrypt its cookies — and
    /// Chromium answers that by DELETING them, then Fury packs the empty jar
    /// and pushes it.
    ///
    /// Travels on its own descriptor, not in the fingerprint config: that
    /// config is republished into shared memory for every child process,
    /// renderers included, and the renderer is the one process on the machine
    /// that runs the visited site's code.
    pub os_crypt_key: Option<[u8; 32]>,

    /// The UI locale, one of Chrome's shipped ones. See `fury_shared::locale`.
    ///
    /// This is what makes `Intl.DateTimeFormat().resolvedOptions().locale`,
    /// `toLocaleString`, collation and every other ICU-backed answer agree with
    /// the languages the profile announces — and it needs no patch, because
    /// `--lang` is the switch Chrome already uses for exactly this. Measured
    /// against the captures in tools/detect-suite/baselines: a real Chrome
    /// answering `navigator.languages = ru-RU,ru,en-US,en` reports `locale.locale
    /// = ru`, and so does AdsPower. The UI locale IS the Intl locale.
    ///
    /// It has to be a shipped locale. ResourceBundle finds no .pak for anything
    /// else and quietly falls back to en-US, which would leave a profile
    /// announcing German and formatting numbers in English.
    pub ui_locale: &'a str,
}

/// Build the argument vector for the core.
///
/// Split out from spawning so it can be unit-tested — a missing hardening flag
/// is a silent security regression otherwise.
pub fn build_args(spec: &LaunchSpec) -> Vec<String> {
    let mut args = vec![
        format!("--user-data-dir={}", spec.user_data_dir.display()),
        // Everything goes through the relay. No exceptions, no bypass list:
        // an entry in --proxy-bypass-list is a direct connection from the real IP.
        format!("--proxy-server=http://127.0.0.1:{}", spec.relay_port),
        "--proxy-bypass-list=<-loopback>".to_string(),
        // The config itself arrives over fd 3; only the number is visible here.
        "--fury-fp-fd=3".to_string(),
        "--no-default-browser-check".to_string(),
        "--no-first-run".to_string(),
        // Anything that phones home outside the profile proxy is a real-IP leak.
        "--disable-background-networking".to_string(),
        // --disable-component-update is NOT here, and its absence is load-bearing.
        //
        // It was, and it broke Widevine on every profile. bundle_widevine_cdm
        // puts the CDM inside the app bundle, but Chromium REGISTERS it through
        // the component updater's registration path — so with updates disabled
        // the blob sits there and nothing ever tells the browser about it.
        // Measured 02.08.2026: the same binary answers
        // requestMediaKeySystemAccess("com.widevine.alpha") with "available"
        // when launched directly and NotSupportedError when launched with this
        // switch, and removing it from the launcher's own argument set is enough
        // to bring it back. It was the one gate check failing.
        //
        // That cost every profile Netflix, Spotify and Disney+, and it cost the
        // thing the gate exists to protect: real Chrome accepts Widevine, and a
        // browser that refuses it is a browser that is not Chrome.
        //
        // What it bought, measured the same day: nothing. With the switch and
        // without it, a browser left alone for 90 seconds opens connections to
        // the same set of Google addresses. And under Fury every one of those
        // goes through the relay, because --proxy-server points at it and the
        // loopback bypass is removed — so the question was never whether the
        // browser talks, only whose address it talks from.
        "--disable-domain-reliability".to_string(),
        "--disable-breakpad".to_string(),
        "--safebrowsing-disable-auto-update".to_string(),
        // QUIC would carry UDP straight past an HTTP relay.
        "--disable-quic".to_string(),
        // WebRTC used to be handled here, by a switch, and the switch did
        // nothing at all.
        //
        // The reasoning was right: WebRTC gathers ICE candidates on its own
        // sockets, past the relay, so a page opening a peer connection reads the
        // machine's real address — identifying the proxy as a proxy AND handing
        // every profile on the machine the same address, which clusters exactly
        // the accounts the product exists to keep apart.
        //
        // What was wrong was believing --force-webrtc-ip-handling-policy did
        // something about it. In M150 that switch is read by
        // content/shell/browser/shell.cc and by browser tests; nothing in
        // //chrome reads it. Chrome takes the value from a profile preference.
        // Measured 02.08.2026 with the switch present: a page still received an
        // srflx candidate carrying this machine's real public address.
        //
        // Now patch 0070, which overrides the preference itself. Nothing to
        // pass here — and that is the point: a switch is visible in `ps` to any
        // process on the machine, a preference is not.
        // Turn off Chromium's field-trial testing config.
        //
        // Not a tuning knob — a divergence from real Chrome that our own build
        // settings create. `is_chrome_branded = false` means GOOGLE_CHROME_
        // BRANDING is off, and `ShouldUseFieldTrialTestingConfig`
        // (variations_field_trial_creator.cc:142) then applies
        // testing/variations/fieldtrial_testing_config.json by default. That
        // file force-enables features real stable Chrome ships disabled —
        // `ReduceAcceptLanguage` and `ReduceAcceptLanguageHTTP` among them, on
        // mac and windows both, and those two cut Accept-Language down to a
        // single language. A browser announcing four languages in JS and one in
        // its headers contradicts itself, and it took reading the build config
        // to find, because nothing about it is visible in the source of the
        // patches.
        //
        // Also set as `disable_fieldtrial_testing_config = true` in core/args.
        // One decision, stated where it belongs and again where it takes effect
        // today: the GN flag is right and needs a rebuild, and this switch works
        // on the binaries that already exist.
        "--disable-field-trial-config".to_string(),
        // The Intl locale, and the browser's own UI language with it — which is
        // the honest pairing. A German exit whose menus are in German and whose
        // dates format in German is a German install; one that formats dates in
        // English is a spoof with a seam.
        //
        // Reaches every renderer without a patch: the browser turns --lang into
        // the kApplicationLocale pref (chrome_command_line_pref_store.cc:50),
        // hands it back down to each child (render_process_host_impl.cc:3734),
        // and each child calls SetICUDefaultLocale with it
        // (chrome_main_delegate.cc:1449 via l10n_util.cc:591). V8 reads the ICU
        // default for Intl (isolate.cc:8025). Nothing overrides it on either
        // platform we ship: OverrideLocaleWithCocoaLocale is never called in
        // Chrome, and the Windows path tries --lang before the UI language list.
        format!("--lang={}", spec.ui_locale),
    ];

    // fd 4, for the same reason fd 3 carries the config: a key on the command
    // line is a key any other process on the machine can read out of `ps`.
    if spec.os_crypt_key.is_some() {
        args.push("--fury-oscrypt-fd=4".to_string());
    }

    if spec.restrictions.lock_devtools {
        args.push("--fury-lock-devtools".to_string());
    }
    if spec.restrictions.lock_data_export {
        args.push("--fury-lock-data-export".to_string());
    }

    // CDP is full cookie access, so it is gated by export_cookies upstream of here.
    match spec.debug_port {
        Some(p) if !spec.restrictions.deny_cdp => {
            args.push(format!("--remote-debugging-port={p}"));
            args.push("--remote-allow-origins=*".to_string());
        }
        _ => {}
    }

    args.extend(spec.start_urls.iter().cloned());
    args
}

/// Validate, then spawn.
///
/// Validation is fail-closed on purpose: launching an inconsistent profile is
/// worse than not launching it, because the inconsistency is itself a signal.
/// The deep consistency checks live on the persona this config was derived from
/// — a valid persona cannot produce a contradictory config — so what is checked
/// here is the other failure: a config the core will not understand.
pub fn spawn(spec: &LaunchSpec) -> Result<std::process::Child, LaunchError> {
    if let Err(missing) = fury_shared::fingerprint::check_core_config(spec.config) {
        let msg = missing
            .iter()
            .map(|e| format!("  - {e}"))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(LaunchError::Inconsistent(msg));
    }

    if !spec.core_binary.exists() {
        return Err(LaunchError::CoreMissing(spec.core_binary.to_path_buf()));
    }

    let args = build_args(spec);
    let mut cmd = std::process::Command::new(spec.core_binary);
    cmd.args(&args).stdin(Stdio::null());

    // The config has to arrive on descriptor 3, where the core reads it to EOF
    // (components/fury/fury_config.cc). Keep `carrier` alive until spawn
    // returns: the child inherits a copy of the descriptor across fork, but
    // only while this one is still open.
    let carrier = config_carrier(spec.config)?;
    attach_config_fd(&mut cmd, &carrier);

    // The key rides the same way and for the same reason. Kept alive until
    // after spawn, like the config: the child inherits a copy across fork, but
    // only while this one is still open.
    let key_carrier = match spec.os_crypt_key {
        Some(key) => {
            let file = anonymous_file(&key)?;
            attach_fd(&mut cmd, &file, 4);
            Some(file)
        }
        None => None,
    };

    let child = cmd.spawn()?;
    drop(carrier);
    drop(key_carrier);
    Ok(child)
}

/// An open, already-unlinked file holding the config JSON, rewound to the start.
///
/// A file rather than a pipe, because the core reads to EOF and a pipe would
/// need the parent to stay around writing — the agent would have to babysit a
/// descriptor for the life of a browser it otherwise just supervises. Unlinked
/// the moment it exists, so the persona never has a name on disk that another
/// process could open; what survives is a descriptor, which is what we wanted
/// to pass anyway.
#[cfg(unix)]
fn config_carrier(config: &serde_json::Value) -> Result<std::fs::File, LaunchError> {
    let json = serde_json::to_vec(config).map_err(|e| {
        LaunchError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    })?;
    anonymous_file(&json)
}

/// A file with `bytes` in it that exists only as a descriptor.
///
/// Created 0600 and unlinked immediately, so it is never nameable by another
/// process even for the moment it takes to spawn — the same reason the config
/// does not travel in argv.
#[cfg(unix)]
fn anonymous_file(bytes: &[u8]) -> Result<std::fs::File, LaunchError> {
    use std::os::unix::fs::OpenOptionsExt;

    // Named uniquely only long enough to be opened; 0600 so that even in that
    // window nobody else can read it.
    let path = std::env::temp_dir().join(format!(
        "fury-fp-{}-{}",
        std::process::id(),
        // Monotonic within a process; the pid keeps it unique between them.
        NEXT_CARRIER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    std::fs::remove_file(&path)?;

    file.write_all(bytes)?;
    file.rewind()?;
    Ok(file)
}

#[cfg(unix)]
static NEXT_CARRIER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(unix)]
fn attach_config_fd(cmd: &mut std::process::Command, carrier: &std::fs::File) {
    attach_fd(cmd, carrier, 3)
}

#[cfg(unix)]
fn attach_fd(cmd: &mut std::process::Command, carrier: &std::fs::File, target: i32) {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    let src = carrier.as_raw_fd();
    // SAFETY: runs in the forked child before exec, so it must only call
    // async-signal-safe functions. dup2 is one. It also clears FD_CLOEXEC on
    // the new descriptor, which is precisely why 3 survives the exec.
    unsafe {
        cmd.pre_exec(move || {
            if libc::dup2(src, target) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

// Windows inherits handles by an entirely different mechanism
// (PROC_THREAD_ATTRIBUTE_HANDLE_LIST on an inheritable handle, plus a switch
// carrying the handle value rather than a descriptor number). Left unwritten
// rather than guessed: nothing in this repo has been built for Windows yet, so
// an implementation here could not be tested and would only look finished.
#[cfg(not(unix))]
fn config_carrier(_config: &serde_json::Value) -> Result<std::fs::File, LaunchError> {
    Err(LaunchError::Io(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "passing the fingerprint config is not implemented on this platform yet",
    )))
}

#[cfg(not(unix))]
fn attach_config_fd(_cmd: &mut std::process::Command, _carrier: &std::fs::File) {}

#[cfg(test)]
mod tests {
    use super::*;
    use fury_shared::rbac::{effective, OrgRole, PermSet};

    /// A real derived config, not a hand-written one — these tests exist to
    /// check the thing that actually gets launched.
    fn sample_config() -> serde_json::Value {
        let persona: fury_shared::persona::Persona = serde_json::from_str(include_str!(
            "../../shared/personas/macos-15-m-series-1728x1117.json"
        ))
        .expect("shipped persona parses");
        persona.derive_core_config(
            1,
            &fury_shared::persona::ProfileContext {
                timezone: "Europe/Berlin".into(),
                languages: vec!["de-DE".into(), "de".into()],
                ui_locale: "de".into(),
                geolocation: Some((52.52, 13.405)),
                chrome_major: 150,
                chrome_full_version: "150.0.7871.187".into(),
            },
        )
    }

    fn spec_for(restrictions: LaunchRestrictions, cfg: &serde_json::Value) -> LaunchSpec<'_> {
        LaunchSpec {
            core_binary: Path::new("/nonexistent/Fury"),
            user_data_dir: Path::new("/tmp/p"),
            config: cfg,
            relay_port: 41000,
            restrictions,
            start_urls: &[],
            debug_port: Some(9222),
            os_crypt_key: None,
            ui_locale: "de",
        }
    }

    #[test]
    fn operator_launch_is_hardened_and_gets_no_cdp() {
        let perms = effective(OrgRole::Member, Some(PermSet::operator()));
        let r = LaunchRestrictions::for_perms(perms);
        let cfg = sample_config();
        let args = build_args(&spec_for(r, &cfg));

        assert!(!args.iter().any(|a| a.starts_with("--remote-debugging-port")));
        assert!(args.iter().any(|a| a == "--fury-lock-devtools"));
        assert!(args.iter().any(|a| a == "--fury-lock-data-export"));
    }

    #[test]
    fn the_ui_locale_reaches_the_core_and_matches_the_config() {
        // The two halves of one fact. `--lang` is what sets the ICU default and
        // therefore every Intl answer; `locale.locale` is what the config says
        // the profile claims. They are computed from one value on purpose, and
        // a launch where they disagree is a browser announcing German and
        // formatting numbers in Russian — which is what the capture in
        // baselines/final.json actually shows, and what this guards.
        let cfg = sample_config();
        let r = LaunchRestrictions::for_perms(PermSet::full_profile_work());
        let spec = spec_for(r, &cfg);
        let args = build_args(&spec);

        assert!(
            args.iter().any(|a| a == "--lang=de"),
            "no --lang in {args:?}"
        );
        assert_eq!(
            cfg["locale"]["locale"], "de",
            "the config must claim the locale the core is launched with"
        );

        // And it must be a locale Chrome ships strings for. Anything else and
        // ResourceBundle silently falls back to en-US, leaving the browser
        // formatting in a language the profile never claimed.
        assert_eq!(
            fury_shared::locale::ui_locale_for(Some(spec.ui_locale)),
            spec.ui_locale
        );
    }

    #[test]
    fn everything_routes_through_the_relay() {
        let cfg = sample_config();
        let r = LaunchRestrictions::for_perms(PermSet::full_profile_work());
        let args = build_args(&spec_for(r, &cfg));

        assert!(args.iter().any(|a| a == "--proxy-server=http://127.0.0.1:41000"));
        assert!(args.iter().any(|a| a == "--disable-quic"));
        // Not networking, but the same class of problem: a build setting that
        // silently makes this browser behave unlike the one it claims to be.
        assert!(args.iter().any(|a| a == "--disable-field-trial-config"));
    }

    #[test]
    fn the_cookie_key_is_offered_only_when_there_is_one() {
        // A profile with no key of its own must not claim a descriptor that
        // carries nothing: the core would read zero bytes, log, and fall back —
        // but the switch would be there in `ps` saying otherwise.
        let cfg = sample_config();
        let r = LaunchRestrictions::for_perms(PermSet::full_profile_work());
        let mut spec = spec_for(r.clone(), &cfg);
        assert!(!build_args(&spec).iter().any(|a| a.starts_with("--fury-oscrypt-fd")));

        spec.os_crypt_key = Some([7u8; 32]);
        let args = build_args(&spec);
        assert!(args.iter().any(|a| a == "--fury-oscrypt-fd=4"), "{args:?}");
        // And never in argv itself. The whole reason it travels on a descriptor
        // is that `ps` is readable by every process on the machine.
        assert!(
            !args.iter().any(|a| a.contains("07070707")),
            "the key must not appear on the command line"
        );
        // The other UDP path out is NOT handled here any more, and the absence
        // is asserted so nobody puts it back. This switch was passed on every
        // launch for weeks and read by nobody — see the note in build_args. The
        // profile's WebRTC routing is patch 0070's job now, through the
        // preference Chrome actually consults.
        assert!(!args.iter().any(|a| a.contains("webrtc-ip-handling")));
        // Widevine dies with this switch present — see the note in build_args.
        // Asserted rather than remembered: it reads like an obvious hardening
        // flag, and putting it back costs every profile its DRM playback and
        // fails the release gate.
        assert!(!args.iter().any(|a| a.contains("disable-component-update")));
        assert!(!args.iter().any(|a| a.contains("--no-proxy-server")));
    }

    #[test]
    fn config_never_appears_in_argv() {
        let cfg = sample_config();
        let r = LaunchRestrictions::for_perms(PermSet::full_profile_work());
        let args = build_args(&spec_for(r, &cfg));
        let joined = args.join(" ");
        assert!(!joined.contains("webgl"), "fingerprint leaked into argv");
        assert!(!joined.contains(cfg["navigator"]["userAgent"].as_str().unwrap()));
        assert!(args.iter().any(|a| a == "--fury-fp-fd=3"));
    }

    #[test]
    fn a_config_the_core_cannot_read_refuses_to_launch() {
        // The exact mistake this guards: handing over FingerprintConfig, whose
        // keys are snake_case, so every lookup in the core misses and the
        // profile silently reports the host machine.
        let wrong = serde_json::to_value(fury_shared::fingerprint::samples::macos_arm64()).unwrap();
        let r = LaunchRestrictions::for_perms(PermSet::full_profile_work());
        let err = spawn(&spec_for(r, &wrong)).unwrap_err();
        assert!(matches!(err, LaunchError::Inconsistent(_)), "got {err:?}");
    }

    #[test]
    fn a_config_missing_one_key_refuses_to_launch() {
        let mut cfg = sample_config();
        cfg["noise"].as_object_mut().unwrap().remove("canvasSeed");
        let r = LaunchRestrictions::for_perms(PermSet::full_profile_work());
        let err = spawn(&spec_for(r, &cfg)).unwrap_err();
        assert!(matches!(err, LaunchError::Inconsistent(_)), "got {err:?}");
    }
}
