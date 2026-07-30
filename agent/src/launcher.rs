//! Launching the patched core.
//!
//! # Why the fingerprint config is not passed on the command line
//!
//! Process arguments are readable by any other process on the machine (`ps`,
//! Process Explorer, and any native helper an anti-fraud vendor ships). A
//! command line containing the whole spoofed persona is a gift. The config
//! travels over an inherited pipe instead; only the descriptor number appears
//! in the arguments.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use fury_shared::rbac::LaunchRestrictions;
use fury_shared::FingerprintConfig;

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
    pub config: &'a FingerprintConfig,
    pub relay_port: u16,
    pub restrictions: LaunchRestrictions,
    pub start_urls: &'a [String],
    /// Only set when the caller is allowed CDP at all; see `LaunchRestrictions`.
    pub debug_port: Option<u16>,
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
        "--disable-component-update".to_string(),
        "--disable-domain-reliability".to_string(),
        "--disable-breakpad".to_string(),
        "--safebrowsing-disable-auto-update".to_string(),
        // QUIC would carry UDP straight past an HTTP relay.
        "--disable-quic".to_string(),
    ];

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
pub fn spawn(spec: &LaunchSpec) -> Result<std::process::Child, LaunchError> {
    if let Err(errs) = spec.config.validate() {
        let msg = errs
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

    // TODO(phase-1): inherit the config pipe as fd 3.
    //   unix:    CommandExt::pre_exec + dup2 onto 3
    //   windows: PROC_THREAD_ATTRIBUTE_HANDLE_LIST with an inheritable pipe
    // Blocked on patch 0001-fp-config-plumbing landing in the core.

    Ok(cmd.spawn()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fury_shared::fingerprint::samples::macos_arm64 as sample_config;
    use fury_shared::rbac::{effective, OrgRole, PermSet};

    fn spec_for(restrictions: LaunchRestrictions, cfg: &FingerprintConfig) -> LaunchSpec<'_> {
        LaunchSpec {
            core_binary: Path::new("/nonexistent/Fury"),
            user_data_dir: Path::new("/tmp/p"),
            config: cfg,
            relay_port: 41000,
            restrictions,
            start_urls: &[],
            debug_port: Some(9222),
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
    fn everything_routes_through_the_relay() {
        let cfg = sample_config();
        let r = LaunchRestrictions::for_perms(PermSet::full_profile_work());
        let args = build_args(&spec_for(r, &cfg));

        assert!(args.iter().any(|a| a == "--proxy-server=http://127.0.0.1:41000"));
        assert!(args.iter().any(|a| a == "--disable-quic"));
        assert!(!args.iter().any(|a| a.contains("--no-proxy-server")));
    }

    #[test]
    fn config_never_appears_in_argv() {
        let cfg = sample_config();
        let r = LaunchRestrictions::for_perms(PermSet::full_profile_work());
        let args = build_args(&spec_for(r, &cfg));
        let joined = args.join(" ");
        assert!(!joined.contains("webgl"), "fingerprint leaked into argv");
        assert!(!joined.contains(&cfg.navigator.user_agent));
        assert!(args.iter().any(|a| a == "--fury-fp-fd=3"));
    }

    #[test]
    fn inconsistent_config_refuses_to_launch() {
        let mut cfg = sample_config();
        cfg.screen.scrollbar_width = 15; // Windows scrollbar on a macOS profile
        let r = LaunchRestrictions::for_perms(PermSet::full_profile_work());
        let err = spawn(&spec_for(r, &cfg)).unwrap_err();
        assert!(matches!(err, LaunchError::Inconsistent(_)));
    }
}
