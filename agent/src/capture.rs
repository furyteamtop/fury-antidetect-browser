// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Capturing this machine as a persona, from the application.
//!
//! The catalogue has 27 machines and two of them are measured. Adding one used
//! to mean `tools/detect-suite/capture-chrome.sh`, a collector on port 8731 and
//! `fury-detect persona` — a checkout, Python and a terminal. The machines that
//! could widen the crowd belong to people who installed the .dmg, and none of
//! them has any of that. This is the same three steps behind one button
//! (docs/16, 3.4).
//!
//! What it does, in order:
//!
//! 1. finds the Google Chrome already installed here — the persona must
//!    describe the machine, and only an unmodified browser reports the machine.
//!    Our own core would report itself with our patches in the path, which is a
//!    measurement of us, not of the host;
//! 2. serves the detect-suite probe on a loopback port under a one-shot token
//!    and launches Chrome at it with a throwaway profile, so extensions and
//!    settings the operator has installed do not shape the capture;
//! 3. receives the dump the page posts back, converts it with
//!    `fury_shared::capture::from_capture` — the same code `fury-detect persona`
//!    runs — and checks the result with `Persona::validate()`.
//!
//! What it does not do: send anything anywhere. The result is returned to the
//! interface, which shows it whole and offers to write it to a file. Getting
//! that file into the catalogue is a pull request, on purpose — README promises
//! no telemetry, and an agent that opens a connection nobody asked for is what
//! that word means.

use std::path::PathBuf;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::relay::{PROBE_HTML, PROBE_JS, PROBE_SW};

#[derive(Debug, serde::Serialize)]
pub struct Captured {
    /// The persona, as it would sit in `shared/personas/contributed/`.
    pub persona: serde_json::Value,
    /// What `validate()` objected to. Empty is the result that matters; a
    /// non-empty list on a real machine is either a bug in the validator or a
    /// machine the catalogue thought impossible — both worth more than the file.
    pub problems: Vec<String>,
    /// Which browser produced the dump, as it reported itself.
    pub browser: String,
}

/// Where a Google Chrome executable is on this machine, or why not.
fn chrome_binary() -> Result<PathBuf, String> {
    let candidates: Vec<PathBuf> = if cfg!(target_os = "macos") {
        let mut v = vec![
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            PathBuf::from("/Applications/Google Chrome Beta.app/Contents/MacOS/Google Chrome Beta"),
        ];
        if let Ok(h) = std::env::var("HOME") {
            v.push(PathBuf::from(h).join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
        }
        v
    } else if cfg!(target_os = "windows") {
        ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .map(|b| PathBuf::from(b).join(r"Google\Chrome\Application\chrome.exe"))
            .collect()
    } else {
        vec![PathBuf::from("/opt/google/chrome/chrome"), PathBuf::from("/usr/bin/google-chrome")]
    };
    candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "no Google Chrome on this machine — looked in {}. A persona has to come from an \
                 unmodified browser, so there is nothing to capture with",
                candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
            )
        })
}

/// Run the capture end to end. Takes up to about a minute; the browser is
/// closed and its throwaway profile removed whatever happens.
pub async fn run() -> Result<Captured, String> {
    let chrome = chrome_binary()?;

    let token: String = (0..16).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let url = format!("http://127.0.0.1:{port}/{token}/probe.html?auto=capture");

    let (tx, rx) = tokio::sync::oneshot::channel::<serde_json::Value>();
    let serve_token = token.clone();
    let server = tokio::spawn(async move {
        let mut tx = Some(tx);
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let mut buf = Vec::with_capacity(8192);
            // Read the head, then as much body as Content-Length says.
            let mut head_end = None;
            loop {
                let mut chunk = [0u8; 8192];
                let n = match sock.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                buf.extend_from_slice(&chunk[..n]);
                if head_end.is_none() {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        head_end = Some(i + 4);
                    }
                }
                if let Some(he) = head_end {
                    let head = String::from_utf8_lossy(&buf[..he]).to_string();
                    let want: usize = head
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().ok()))
                        .flatten()
                        .unwrap_or(0);
                    if buf.len() - he >= want {
                        break;
                    }
                }
            }
            let Some(he) = head_end else { continue };
            let head = String::from_utf8_lossy(&buf[..he]).to_string();
            let line = head.lines().next().unwrap_or_default();
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default();
            let target = parts.next().unwrap_or_default();
            let path = target.split('?').next().unwrap_or_default();
            let prefix = format!("/{serve_token}/");
            let (status, ctype, body): (&str, &str, String) = match (method, path.strip_prefix(prefix.as_str())) {
                ("GET", Some("probe.html")) => ("200 OK", "text/html; charset=utf-8", PROBE_HTML.to_string()),
                ("GET", Some("probe.js")) => ("200 OK", "application/javascript; charset=utf-8", PROBE_JS.to_string()),
                ("GET", Some("sw-probe.js")) => ("200 OK", "application/javascript; charset=utf-8", PROBE_SW.to_string()),
                // The page posts to /save relative to its origin, not under the
                // token; the token was already proved by loading the page, and
                // a process that could guess a path on this port for the
                // second it exists would have had to guess the first.
                ("POST", _) if path == "/save" => {
                    let raw = &buf[he..];
                    match serde_json::from_slice::<serde_json::Value>(raw) {
                        Ok(v) => {
                            if let Some(t) = tx.take() {
                                let _ = t.send(v.get("dump").cloned().unwrap_or(v));
                            }
                            ("200 OK", "application/json", r#"{"saved":"captured"}"#.to_string())
                        }
                        Err(e) => ("400 Bad Request", "text/plain", format!("not json: {e}")),
                    }
                }
                _ => ("404 Not Found", "text/plain", String::new()),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.write_all(body.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });

    // A throwaway profile, deliberately: a reference capture must describe a
    // clean browser, not one shaped by whatever the operator has installed.
    let scratch = std::env::temp_dir().join(format!("fury-capture-{token}"));
    let mut child = std::process::Command::new(&chrome)
        .arg(format!("--user-data-dir={}", scratch.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-sync")
        .arg("--window-size=1000,760")
        .arg(&url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("could not start {}: {e}", chrome.display()))?;

    let dump = tokio::time::timeout(Duration::from_secs(90), rx).await;
    let _ = child.kill();
    let _ = child.wait();
    server.abort();
    let _ = std::fs::remove_dir_all(&scratch);

    let dump = match dump {
        Ok(Ok(v)) => v,
        Ok(Err(_)) => return Err("the browser closed before the probe finished".into()),
        Err(_) => return Err("the probe did not report back within 90 s — Chrome may have shown a dialog that needed answering".into()),
    };

    let browser = fury_shared::capture::s(&dump, "navigator.userAgent").unwrap_or_default();
    // Unique against what ships: the same model of machine is the common case
    // for a contribution, and two files with one id would be refused by the
    // catalogue's own test. A second M5 becomes macos-m5-1470x956-2.
    let taken: std::collections::HashSet<String> =
        fury_shared::catalogue::all().into_iter().map(|p| p.id).collect();
    let base = suggest_id(&dump);
    let mut id = base.clone();
    let mut n = 2;
    while taken.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    let persona = fury_shared::capture::from_capture(&dump, &id, 0.01)?;

    let problems = match serde_json::from_value::<fury_shared::persona::Persona>(persona.clone()) {
        Ok(p) => p.validate().err().map(|errs| errs.iter().map(|e| e.to_string()).collect()).unwrap_or_default(),
        Err(e) => vec![format!("the persona does not fit the schema: {e}")],
    };

    Ok(Captured { persona, problems, browser })
}

/// `macos-m1-1440x900`, `win11-rtx4060-1920x1080` — the catalogue's naming,
/// derived from what the machine says about itself.
fn suggest_id(dump: &serde_json::Value) -> String {
    use fury_shared::capture::{n, s};
    let plat = s(dump, "clientHints.platform").unwrap_or_default();
    // A phone is known by its model, which the hints carry: android-sm-a546b.
    // The GPU and the screen say much less about a handset than about a PC.
    if plat == "Android" {
        let model = s(dump, "clientHints.model").unwrap_or_default().to_ascii_lowercase();
        let slug: String = model
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        return format!("android-{}", if slug.is_empty() { "phone".into() } else { slug });
    }
    let os = if plat == "macOS" {
        "macos".to_string()
    } else if plat == "Windows" {
        // navigator.userAgentData says "Windows" for both, and only
        // platformVersion tells them apart: Chrome reports 13.0.0 and up on
        // Windows 11 and 1.0.0-10.0.0 on Windows 10. Calling every Windows
        // machine win11 named a tester's Windows 10 capture after an OS it
        // is not running.
        let major = s(dump, "clientHints.platformVersion")
            .and_then(|v| v.split('.').next().and_then(|m| m.parse::<u32>().ok()));
        match major {
            Some(m) if m >= 13 => "win11".to_string(),
            Some(_) => "win10".to_string(),
            None => "windows".to_string(),
        }
    } else {
        plat.to_ascii_lowercase()
    };
    let renderer = s(dump, "webgl.webgl2.unmasked.renderer")
        .or_else(|| s(dump, "webgl.webgl1.unmasked.renderer"))
        .unwrap_or_default();
    let gpu = gpu_slug(&renderer);
    let w = n(dump, "screen.width").unwrap_or(0.0) as u32;
    let h = n(dump, "screen.height").unwrap_or(0.0) as u32;
    format!("{os}-{gpu}-{w}x{h}")
}

/// "ANGLE (Apple, ANGLE Metal Renderer: Apple M1, Unspecified Version)" → "m1";
/// "ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Direct3D11 …)" → "rtx4060".
fn gpu_slug(renderer: &str) -> String {
    let lower = renderer.to_ascii_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    // Apple: the M-series token. NVIDIA/AMD/Intel: the model token after the family word.
    if let Some(i) = words.iter().position(|w| w.starts_with('m') && w.len() <= 3 && w[1..].chars().all(|c| c.is_ascii_digit()) && !w[1..].is_empty()) {
        let mut slug = words[i].to_string();
        if let Some(next) = words.get(i + 1) {
            if ["pro", "max", "ultra"].contains(next) {
                slug.push_str(next);
            }
        }
        return slug;
    }
    // "Intel(R) Iris(R) Xe" splits into iris, r, xe: the trademark marks are
    // words too, and not the ones wanted.
    let words: Vec<&str> = words.into_iter().filter(|w| *w != "r" && *w != "tm").collect();
    for family in ["rtx", "gtx", "rx", "arc", "iris", "uhd", "radeon"] {
        if let Some(i) = words.iter().position(|w| *w == family) {
            let model = words.get(i + 1).copied().unwrap_or("");
            return format!("{family}{model}");
        }
    }
    words.get(1).copied().unwrap_or("gpu").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_follow_the_catalogue_naming() {
        assert_eq!(gpu_slug("ANGLE (Apple, ANGLE Metal Renderer: Apple M1, Unspecified Version)"), "m1");
        assert_eq!(gpu_slug("ANGLE (Apple, ANGLE Metal Renderer: Apple M3 Pro, Unspecified Version)"), "m3pro");
        assert_eq!(gpu_slug("ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Direct3D11 vs_5_0 ps_5_0, D3D11)"), "rtx4060");
        assert_eq!(gpu_slug("ANGLE (Intel, Intel(R) Iris(R) Xe Graphics Direct3D11 vs_5_0 ps_5_0, D3D11)"), "irisxe");
        let dump = serde_json::json!({
            "clientHints": { "platform": "macOS" },
            "webgl": { "webgl2": { "unmasked": { "renderer": "ANGLE (Apple, ANGLE Metal Renderer: Apple M5, Unspecified Version)" } } },
            "screen": { "width": 1470, "height": 956 }
        });
        assert_eq!(suggest_id(&dump), "macos-m5-1470x956");
        let phone = serde_json::json!({ "clientHints": { "platform": "Android", "model": "SM-A546B" } });
        assert_eq!(suggest_id(&phone), "android-sm-a546b");
    }

    #[test]
    fn windows_10_and_11_are_told_apart_by_platform_version() {
        let win = |version: &str| {
            serde_json::json!({
                "clientHints": { "platform": "Windows", "platformVersion": version },
                "webgl": { "webgl2": { "unmasked": { "renderer": "ANGLE (Intel, Intel(R) UHD Graphics (0x00009B41) Direct3D11 vs_5_0 ps_5_0, D3D11)" } } },
                "screen": { "width": 1280, "height": 720 }
            })
        };
        // The tester's capture, 27.09.2026: Windows 10, platformVersion 7.0.0.
        assert_eq!(suggest_id(&win("7.0.0")), "win10-uhdgraphics-1280x720");
        assert_eq!(suggest_id(&win("10.0.0")), "win10-uhdgraphics-1280x720");
        assert_eq!(suggest_id(&win("13.0.0")), "win11-uhdgraphics-1280x720");
        assert_eq!(suggest_id(&win("19.0.0")), "win11-uhdgraphics-1280x720");
        assert_eq!(suggest_id(&win("")), "windows-uhdgraphics-1280x720");
    }
}
