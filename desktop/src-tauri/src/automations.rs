// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov, devrahmanbd and the Fury authors

//! Managing and running community automations from the desktop shell.
//!
//! Discovers automation packages (automation.json manifests), provides
//! execution controls, and spawns automation scripts with live log streaming.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::commands::{ApiErr, AppState};
use crate::settings::AutomationSettings;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationManifest {
    #[serde(default)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub drives: Vec<String>,
    #[serde(default)]
    pub requires: Vec<String>,
    pub entry: String,
    #[serde(default)]
    pub licence: String,
    #[serde(default)]
    pub fury: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub is_builtin: bool,
    #[serde(default)]
    pub inputs: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RunRequest {
    pub automation_id: String,
    pub profile_ids: Vec<String>,
    #[serde(default)]
    pub proxy_mode: Option<String>,
    #[serde(default)]
    pub bulk_proxies: Option<String>,
    #[serde(default)]
    pub concurrency: Option<u32>,
    #[serde(default)]
    pub run_count: Option<u32>,
    #[serde(default)]
    #[allow(dead_code)]
    pub custom_user_agent: Option<String>,
    #[serde(default)]
    pub dwell_seconds: Option<(u64, u64)>,
    #[serde(default)]
    pub custom_urls: Option<Vec<String>>,
    #[serde(default)]
    pub background: Option<bool>,
    #[serde(default)]
    pub headless: Option<bool>,
    #[serde(default)]
    pub custom_inputs: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActiveJobInfo {
    pub id: String,
    pub automation_id: String,
    pub automation_name: String,
    pub profile_ids: Vec<String>,
    pub started_at_ms: u64,
    pub status: String, // "running", "finished", "failed", "stopped"
    pub logs: Vec<String>,
    pub error: Option<String>,
}

struct ManagedChild {
    child: Arc<Mutex<Option<Child>>>,
    job: ActiveJobInfo,
}

static JOBS: std::sync::LazyLock<Mutex<HashMap<String, ManagedChild>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

const DEFAULT_WARMUP_JSON: &str =
    include_str!("../../../automations/interactive-warmup/automation.json");
const DEFAULT_WARMUP_PY: &str =
    include_str!("../../../automations/interactive-warmup/warm_up.py");
const DEFAULT_CDP_PY: &str =
    include_str!("../../../automations/interactive-warmup/cdp.py");
const DEFAULT_HUMAN_PY: &str =
    include_str!("../../../automations/interactive-warmup/human.py");
const DEFAULT_FURY_PY: &str =
    include_str!("../../../automations/interactive-warmup/fury.py");
const DEFAULT_WARMUP_README: &str =
    include_str!("../../../automations/interactive-warmup/README.md");

fn ensure_default_automations(user_dir: &Path) {
    let warmup_dir = user_dir.join("interactive-warmup");
    if !warmup_dir.exists() {
        let _ = std::fs::create_dir_all(&warmup_dir);
    }
    let warmup_manifest = warmup_dir.join("automation.json");
    // Always keep built-in scripts synchronized with latest release
    let _ = std::fs::write(&warmup_manifest, DEFAULT_WARMUP_JSON);
    let _ = std::fs::write(warmup_dir.join("warm_up.py"), DEFAULT_WARMUP_PY);
    let _ = std::fs::write(warmup_dir.join("cdp.py"), DEFAULT_CDP_PY);
    let _ = std::fs::write(warmup_dir.join("human.py"), DEFAULT_HUMAN_PY);
    let _ = std::fs::write(warmup_dir.join("fury.py"), DEFAULT_FURY_PY);
    let _ = std::fs::write(warmup_dir.join("README.md"), DEFAULT_WARMUP_README);

    // Clean up deprecated puppeteer-crawler directory if present
    let puppeteer_dir = user_dir.join("puppeteer-crawler");
    if puppeteer_dir.exists() {
        let _ = std::fs::remove_dir_all(&puppeteer_dir);
    }
}

/// Discovers automations from automations directory and custom user data directory.
#[tauri::command]
pub async fn automations_list() -> Result<Vec<AutomationManifest>, ApiErr> {
    let mut out = Vec::new();

    // 1. Built-in repo automations
    let candidates = [
        PathBuf::from("automations"),
        PathBuf::from("../automations"),
        PathBuf::from("../../automations"),
        PathBuf::from("examples/automations"),
        PathBuf::from("../examples/automations"),
        PathBuf::from("../../examples/automations"),
    ];

    for base in candidates {
        if base.is_dir() {
            scan_automations_dir(&base, true, &mut out);
            break;
        }
    }

    // 2. User custom automations directory in Fury application data
    let user_dir = fury_platform::dirs::data_dir().join("automations");
    ensure_default_automations(&user_dir);
    if user_dir.is_dir() {
        scan_automations_dir(&user_dir, false, &mut out);
    }

    // 3. Infallible guarantee: ensure interactive-warmup is always present
    if !out.iter().any(|a| a.id == "interactive-warmup") {
        if let Ok(mut manifest) = serde_json::from_str::<AutomationManifest>(DEFAULT_WARMUP_JSON) {
            manifest.id = "interactive-warmup".into();
            manifest.path = user_dir.join("interactive-warmup").display().to_string();
            manifest.is_builtin = true;
            if manifest.summary.is_empty() {
                manifest.summary = manifest.description.clone().unwrap_or_default();
            }
            out.push(manifest);
        }
    }

    for item in &mut out {
        if item.id == "interactive-warmup" {
            item.is_builtin = true;
        }
    }

    Ok(out)
}

fn scan_automations_dir(dir: &Path, is_builtin: bool, out: &mut Vec<AutomationManifest>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let folder_name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        if folder_name.starts_with('_') || folder_name.starts_with('.') {
            continue;
        }

        let manifest_file = p.join("automation.json");
        if !manifest_file.is_file() {
            continue;
        }

        if let Ok(content) = std::fs::read_to_string(&manifest_file) {
            if let Ok(mut manifest) = serde_json::from_str::<AutomationManifest>(&content) {
                if manifest.id.is_empty() {
                    manifest.id = folder_name;
                }
                if out.iter().any(|existing| existing.id == manifest.id) {
                    continue;
                }
                if manifest.summary.is_empty() {
                    manifest.summary = manifest.description.clone().unwrap_or_default();
                }
                manifest.path = p.display().to_string();
                manifest.is_builtin = is_builtin || manifest.id == "interactive-warmup";
                out.push(manifest);
            }
        }
    }
}

/// Reads saved automation settings.
#[tauri::command]
pub async fn automations_get_settings(state: State<'_, AppState>) -> Result<AutomationSettings, ApiErr> {
    let settings = state.settings.lock().map_err(|_| ApiErr::local("settings_lock"))?;
    Ok(settings.automation.clone())
}

/// Saves automation settings.
#[tauri::command]
pub async fn automations_save_settings(
    state: State<'_, AppState>,
    automation: AutomationSettings,
) -> Result<(), ApiErr> {
    let mut settings = state.settings.lock().map_err(|_| ApiErr::local("settings_lock"))?;
    settings.automation = automation;
    settings.save(&state.config_dir).map_err(|e| ApiErr::local(e.to_string()))?;
    Ok(())
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn find_python_binary(auto_dir: &Path) -> PathBuf {
    // 1. Check for local virtual environment inside the automation directory
    let venv_candidates = [
        auto_dir.join(".venv/bin/python3"),
        auto_dir.join(".venv/bin/python"),
        auto_dir.join("venv/bin/python3"),
        auto_dir.join("venv/bin/python"),
        auto_dir.join(".venv/Scripts/python.exe"),
        auto_dir.join("venv/Scripts/python.exe"),
    ];
    for p in venv_candidates {
        if p.is_file() {
            return p;
        }
    }

    if let Ok(p) = std::env::var("PYTHON3").or_else(|_| std::env::var("PYTHON")) {
        let pb = PathBuf::from(&p);
        if pb.is_file() {
            return pb;
        }
    }

    #[cfg(windows)]
    {
        let sys_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let mut win_candidates = Vec::new();
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let p = PathBuf::from(local_app_data).join("Programs").join("Python");
            if let Ok(entries) = std::fs::read_dir(&p) {
                for entry in entries.flatten() {
                    let exe = entry.path().join("python.exe");
                    if exe.is_file() {
                        win_candidates.push(exe);
                    }
                }
            }
        }
        for ver in &["313", "312", "311", "310", "39"] {
            win_candidates.push(PathBuf::from(format!(r"{}\Python{}\python.exe", sys_drive, ver)));
            win_candidates.push(PathBuf::from(format!(r"{}\Program Files\Python{}\python.exe", sys_drive, ver)));
        }
        for p in win_candidates {
            if p.is_file() {
                return p;
            }
        }
        PathBuf::from("python.exe")
    }

    #[cfg(not(windows))]
    {
        let candidates = [
            "/opt/homebrew/bin/python3",
            "/usr/local/bin/python3",
            "/usr/bin/python3",
            "/opt/local/bin/python3",
            "/Library/Frameworks/Python.framework/Versions/Current/bin/python3",
        ];

        for c in candidates {
            let p = Path::new(c);
            if p.is_file() {
                return p.to_path_buf();
            }
        }

        if let Some(home) = home_dir() {
            let user_candidates = [
                home.join(".pyenv/shims/python3"),
                home.join(".asdf/shims/python3"),
                home.join("miniconda3/bin/python3"),
                home.join("anaconda3/bin/python3"),
                home.join(".local/bin/python3"),
            ];
            for p in user_candidates {
                if p.is_file() {
                    return p;
                }
            }
        }

        PathBuf::from("python3")
    }
}

fn build_sanitized_path() -> String {
    // Only well-known system-wide directories. User developer paths (.cargo/bin, .pyenv/shims,
    // .local/bin) and the ambient $PATH are intentionally excluded: they encode the host
    // operator's username and installed toolchain layout, exposing host filesystem structure
    // to the automation child process and negating the isolation that env_clear() provides.
    //
    // The Python interpreter is resolved to an absolute path by find_python_binary() before
    // the child is spawned, so user-local pythons (pyenv, conda, asdf, ~/.local) are still
    // used — they just are not advertised inside the sandbox's PATH.
    #[cfg(windows)]
    {
        let sys_root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let parts = [
            format!(r"{}\System32", sys_root),
            sys_root.clone(),
            format!(r"{}\System32\Wbem", sys_root),
            format!(r"{}\System32\WindowsPowerShell\v1.0", sys_root),
        ];
        parts.join(";")
    }

    #[cfg(not(windows))]
    {
        let parts = [
            "/opt/homebrew/bin",
            "/opt/homebrew/sbin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ];
        parts.join(":")
    }
}


/// Launches an automation subprocess.
#[tauri::command]
pub async fn automations_run(
    _state: State<'_, AppState>,
    req: RunRequest,
) -> Result<ActiveJobInfo, ApiErr> {
    // Ensure fury-agent is running with FURY_API_PORT active
    let _ = crate::agent::ensure_running().await;

    let list = automations_list().await?;
    let auto = list
        .into_iter()
        .find(|a| a.id == req.automation_id || a.path == req.automation_id)
        .ok_or_else(|| ApiErr::local("automation_not_found"))?;

    let auto_dir = PathBuf::from(&auto.path);
    let entry_file = auto_dir.join(&auto.entry);
    if !entry_file.is_file() {
        return Err(ApiErr::local(format!("entry script not found: {}", entry_file.display())));
    }

    let job_id = uuid::Uuid::now_v7().to_string();
    let is_node = auto.entry.ends_with(".mjs") || auto.entry.ends_with(".js") || auto.runtime.as_deref() == Some("node");
    let mut cmd = if is_node {
        #[cfg(windows)]
        { Command::new("node.exe") }
        #[cfg(not(windows))]
        { Command::new("node") }
    } else {
        Command::new(find_python_binary(&auto_dir))
    };
    // Principle 4: Isolate execution environment. Strip all ambient host variables (USER,
    // LOGNAME, SHELL, LANG, HOME, host proxies, developer tool paths, etc.) to prevent host
    // environment leaks into automation runtimes.
    cmd.env_clear();
    cmd.env("PATH", build_sanitized_path());

    let fury_data = fury_platform::dirs::data_dir();
    cmd.env("FURY_HOME", fury_data.display().to_string());
    // HOME is intentionally NOT injected: fury.py reads the api-token from FURY_HOME, not HOME.
    // Injecting the host user's home directory would expose the operator's username and
    // filesystem layout to the automation child process, defeating env_clear() isolation.
    #[cfg(windows)]
    {
        if let Ok(sys_root) = std::env::var("SystemRoot") {
            cmd.env("SystemRoot", sys_root);
        }
        if let Ok(tmp) = std::env::var("TEMP").or_else(|_| std::env::var("TMP")) {
            cmd.env("TMPDIR", &tmp);
            cmd.env("TEMP", &tmp);
            cmd.env("TMP", &tmp);
        } else {
            cmd.env("TMPDIR", r"C:\Windows\Temp");
            cmd.env("TEMP", r"C:\Windows\Temp");
            cmd.env("TMP", r"C:\Windows\Temp");
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(tmp) = std::env::var("TMPDIR") {
            cmd.env("TMPDIR", tmp);
        } else {
            cmd.env("TMPDIR", "/tmp");
        }
    }

    let venv_dir = auto_dir.join(".venv");
    if venv_dir.is_dir() {
        cmd.env("VIRTUAL_ENV", venv_dir.display().to_string());
    }
    cmd.current_dir(&auto_dir);

    if !is_node {
        cmd.arg("-u");
        cmd.env("PYTHONUNBUFFERED", "1");
        cmd.env("PYTHONIOENCODING", "utf-8");
    }

    cmd.arg(&entry_file);

    // Profile argument if supplied
    for pid in &req.profile_ids {
        cmd.arg(pid);
    }

    // Dynamic arguments
    if let Some((min_d, max_d)) = req.dwell_seconds {
        cmd.arg("--min-dwell").arg(min_d.to_string());
        cmd.arg("--max-dwell").arg(max_d.to_string());
    }
    if let Some(count) = req.run_count {
        cmd.arg("--sites").arg(count.to_string());
    }
    if let Some(ref urls) = req.custom_urls {
        if !urls.is_empty() {
            cmd.arg("--custom-urls");
            for u in urls {
                cmd.arg(u);
            }
        }
    }

    // Environment variables
    cmd.env("FURY_API_PORT", "35000");
    cmd.env("FURY_PROFILE_IDS", req.profile_ids.join(","));
    if let Some(ref proxies) = req.bulk_proxies {
        cmd.env("FURY_BULK_PROXIES", proxies);
    }
    if let Some(ref pm) = req.proxy_mode {
        cmd.env("FURY_PROXY_MODE", pm);
    }
    if let Some(c) = req.concurrency {
        cmd.env("FURY_CONCURRENCY", c.to_string());
    }
    // Note: custom_user_agent is intentionally not injected to preserve Fury's C++ persona spoofing
    if let Some(ref inputs) = req.custom_inputs {
        if let Ok(json_str) = serde_json::to_string(inputs) {
            cmd.env("FURY_INPUTS_JSON", json_str);
        }
        for (k, v) in inputs {
            let env_key = format!("FURY_INPUT_{}", k.to_uppercase());
            let val_str = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Bool(b) => b.to_string(),
                serde_json::Value::Number(n) => n.to_string(),
                other => other.to_string(),
            };
            cmd.env(&env_key, &val_str);

            // Skip fields already handled as top-level CLI flags to prevent duplicates
            if matches!(
                k.as_str(),
                "sites" | "run_count" | "min_dwell" | "max_dwell" | "dwell_seconds" | "background" | "headless" | "custom_urls" | "profiles"
            ) {
                continue;
            }

            if let serde_json::Value::Bool(true) = v {
                cmd.arg(format!("--{}", k.replace('_', "-")));
            } else if let serde_json::Value::String(s) = v {
                if !s.is_empty() {
                    cmd.arg(format!("--{}", k.replace('_', "-"))).arg(s);
                }
            } else if let serde_json::Value::Number(n) = v {
                cmd.arg(format!("--{}", k.replace('_', "-"))).arg(n.to_string());
            }
        }
    }

    // Set PYTHONPATH to include vendor directory, examples directory, and automation directory
    let examples_dir = auto_dir.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf());
    let vendor_dir = auto_dir.join("vendor");
    let current_pypath = std::env::var("PYTHONPATH").unwrap_or_default();
    let mut pypath_parts = Vec::new();
    if vendor_dir.is_dir() {
        pypath_parts.push(vendor_dir.display().to_string());
    }
    if let Some(ref ex_dir) = examples_dir {
        pypath_parts.push(ex_dir.display().to_string());
    }
    pypath_parts.push(auto_dir.display().to_string());
    if !current_pypath.is_empty() {
        pypath_parts.push(current_pypath);
    }
    cmd.env("PYTHONPATH", pypath_parts.join(":"));

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut spawned = cmd.spawn().map_err(|e| ApiErr::local(format!("failed to start process: {e}")))?;

    let stdout = spawned.stdout.take();
    let stderr = spawned.stderr.take();

    let initial_log = format!("Started {} (job: {})", auto.name, &job_id[..8]);
    let child_handle = Arc::new(Mutex::new(Some(spawned)));
    let logs_arc = Arc::new(Mutex::new(vec![initial_log.clone()]));

    let job_info = ActiveJobInfo {
        id: job_id.clone(),
        automation_id: auto.id.clone(),
        automation_name: auto.name.clone(),
        profile_ids: req.profile_ids.clone(),
        started_at_ms: now_ms(),
        status: "running".into(),
        logs: vec![initial_log],
        error: None,
    };

    // Store in global registry
    {
        let mut jobs = JOBS.lock().unwrap();
        jobs.insert(
            job_id.clone(),
            ManagedChild {
                child: child_handle.clone(),
                job: job_info.clone(),
            },
        );
    }

    // Reader thread for stdout
    let jid_clone1 = job_id.clone();
    let logs_clone1 = logs_arc.clone();
    if let Some(out) = stdout {
        std::thread::spawn(move || {
            let reader = BufReader::new(out);
            for line in reader.lines().map_while(Result::ok) {
                let mut logs = logs_clone1.lock().unwrap();
                if logs.len() > 500 {
                    logs.remove(0);
                }
                logs.push(line.clone());
                // Update job log
                let mut jobs = JOBS.lock().unwrap();
                if let Some(m) = jobs.get_mut(&jid_clone1) {
                    m.job.logs = logs.clone();
                }
            }
        });
    }

    // Reader thread for stderr + exit wait
    let jid_clone2 = job_id.clone();
    let logs_clone2 = logs_arc.clone();
    let child_for_wait = child_handle.clone();
    if let Some(err) = stderr {
        std::thread::spawn(move || {
            let reader = BufReader::new(err);
            for line in reader.lines().map_while(Result::ok) {
                let mut logs = logs_clone2.lock().unwrap();
                if logs.len() > 500 {
                    logs.remove(0);
                }
                logs.push(format!("[err] {}", line));
                let mut jobs = JOBS.lock().unwrap();
                if let Some(m) = jobs.get_mut(&jid_clone2) {
                    m.job.logs = logs.clone();
                }
            }

            // Wait for exit
            let mut opt_child = child_for_wait.lock().unwrap();
            if let Some(mut child) = opt_child.take() {
                let exit_status = child.wait();
                let mut jobs = JOBS.lock().unwrap();
                if let Some(m) = jobs.get_mut(&jid_clone2) {
                    match exit_status {
                        Ok(st) if st.success() => {
                            m.job.status = "finished".into();
                            m.job.logs.push("Automation completed successfully.".into());
                        }
                        Ok(st) => {
                            m.job.status = "failed".into();
                            let err_msg = format!("Process exited with status {}", st);
                            m.job.error = Some(err_msg.clone());
                            m.job.logs.push(format!("[err] {}", err_msg));
                        }
                        Err(e) => {
                            m.job.status = "failed".into();
                            let err_msg = e.to_string();
                            m.job.error = Some(err_msg.clone());
                            m.job.logs.push(format!("[err] Failed to wait for process: {}", err_msg));
                        }
                    }
                }
            }
        });
    }

    Ok(job_info)
}

/// Stops an active automation run.
#[tauri::command]
pub async fn automations_stop(job_id: String) -> Result<(), ApiErr> {
    let mut jobs = JOBS.lock().unwrap();
    if let Some(managed) = jobs.get_mut(&job_id) {
        let mut opt = managed.child.lock().unwrap();
        if let Some(mut child) = opt.take() {
            let _ = child.kill();
        }
        managed.job.status = "stopped".into();
        managed.job.logs.push("Stopped by operator".into());
        return Ok(());
    }
    Err(ApiErr::local("job_not_found"))
}

/// Polls active automation runs and their logs.
#[tauri::command]
pub async fn automations_status() -> Result<Vec<ActiveJobInfo>, ApiErr> {
    let jobs = JOBS.lock().unwrap();
    let mut out: Vec<ActiveJobInfo> = jobs.values().map(|m| m.job.clone()).collect();
    out.sort_by_key(|j| j.started_at_ms);
    Ok(out)
}

/// Imports a .fury package archive safely into the user automations directory.
#[tauri::command]
pub async fn automations_import(archive_path: String) -> Result<AutomationManifest, ApiErr> {
    let p = PathBuf::from(&archive_path);
    if !p.is_file() {
        return Err(ApiErr::local("file_not_found"));
    }

    let user_dir = fury_platform::dirs::data_dir().join("automations");
    std::fs::create_dir_all(&user_dir).map_err(|e| ApiErr::local(format!("failed to create dir: {e}")))?;

    let py_script = r#"
import zipfile, os, sys, pathlib, json

archive = sys.argv[1]
dest_root = pathlib.Path(sys.argv[2])

with zipfile.ZipFile(archive, 'r') as zf:
    manifest_data = None
    manifest_name = None
    for n in zf.namelist():
        if n == 'automation.json' or n.endswith('/automation.json'):
            manifest_data = json.loads(zf.read(n))
            manifest_name = n
            break
    if not manifest_data:
        sys.exit("invalid .fury: automation.json missing")

    slug = manifest_data.get('id') or pathlib.Path(archive).stem
    target_dir = dest_root / slug
    target_dir.mkdir(parents=True, exist_ok=True)

    prefix = manifest_name[:-len('automation.json')] if manifest_name != 'automation.json' else ''

    for m in zf.infolist():
        rel = m.filename[len(prefix):] if prefix and m.filename.startswith(prefix) else m.filename
        if not rel or rel.startswith('/'):
            continue
        dest = (target_dir / rel).resolve()
        if not str(dest).startswith(str(target_dir.resolve())):
            sys.exit("zip slip attempt detected")
        if m.is_dir():
            dest.mkdir(parents=True, exist_ok=True)
        else:
            dest.parent.mkdir(parents=True, exist_ok=True)
            with zf.open(m) as src, open(dest, 'wb') as dst:
                dst.write(src.read())

    print(json.dumps({"id": slug, "path": str(target_dir)}))
"#;

    let output = Command::new("python3")
        .arg("-c")
        .arg(py_script)
        .arg(&p)
        .arg(&user_dir)
        .output()
        .map_err(|e| ApiErr::local(format!("failed to run python import: {e}")))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(ApiErr::local(format!("import failed: {err}")));
    }

    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(stdout_str.trim())
        .map_err(|e| ApiErr::local(format!("invalid import output: {e}")))?;

    let folder_path = PathBuf::from(parsed["path"].as_str().unwrap_or_default());
    let manifest_file = folder_path.join("automation.json");
    let content = std::fs::read_to_string(&manifest_file)
        .map_err(|e| ApiErr::local(format!("failed to read imported manifest: {e}")))?;

    let mut manifest: AutomationManifest = serde_json::from_str(&content)
        .map_err(|e| ApiErr::local(format!("invalid manifest JSON: {e}")))?;

    manifest.id = parsed["id"].as_str().unwrap_or_default().to_string();
    manifest.path = folder_path.display().to_string();
    manifest.is_builtin = false;
    if manifest.summary.is_empty() {
        manifest.summary = manifest.description.clone().unwrap_or_default();
    }

    Ok(manifest)
}

/// Exports an automation directory to a .fury archive file.
#[tauri::command]
pub async fn automations_export(automation_id: String, target_path: String) -> Result<String, ApiErr> {
    let list = automations_list().await?;
    let auto = list
        .into_iter()
        .find(|a| a.id == automation_id || a.path == automation_id)
        .ok_or_else(|| ApiErr::local("automation_not_found"))?;

    let auto_dir = PathBuf::from(&auto.path);
    let target = PathBuf::from(&target_path);

    let py_script = r#"
import zipfile, os, sys, pathlib

src_dir = pathlib.Path(sys.argv[1])
out_file = pathlib.Path(sys.argv[2])
out_file.parent.mkdir(parents=True, exist_ok=True)

with zipfile.ZipFile(out_file, 'w', zipfile.ZIP_DEFLATED) as zf:
    for root, _, files in os.walk(src_dir):
        for f in files:
            if f.endswith(('.pyc', '.fury')) or f.startswith('.'):
                continue
            full_p = pathlib.Path(root) / f
            arcname = full_p.relative_to(src_dir)
            zf.write(full_p, arcname)

print(str(out_file.resolve()))
"#;

    let output = Command::new("python3")
        .arg("-c")
        .arg(py_script)
        .arg(&auto_dir)
        .arg(&target)
        .output()
        .map_err(|e| ApiErr::local(format!("export script error: {e}")))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(ApiErr::local(format!("export failed: {err}")));
    }

    Ok(target.display().to_string())
}

/// Imports a .fury package from base64 data.
#[tauri::command]
pub async fn automations_import_b64(name: String, data_b64: String) -> Result<AutomationManifest, ApiErr> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data_b64)
        .map_err(|e| ApiErr::local(format!("invalid base64: {e}")))?;
    let temp_file = std::env::temp_dir().join(format!("import_{}_{}", uuid::Uuid::now_v7(), name));
    std::fs::write(&temp_file, &bytes).map_err(|e| ApiErr::local(format!("failed to write temp file: {e}")))?;
    let res = automations_import(temp_file.display().to_string()).await;
    let _ = std::fs::remove_file(&temp_file);
    res
}

/// Exports an automation directory to a base64 encoded .fury archive.
#[tauri::command]
pub async fn automations_export_b64(automation_id: String) -> Result<String, ApiErr> {
    use base64::Engine;
    let temp_file = std::env::temp_dir().join(format!("export_{}_{}.fury", uuid::Uuid::now_v7(), &automation_id));
    let _ = automations_export(automation_id, temp_file.display().to_string()).await?;
    let bytes = std::fs::read(&temp_file).map_err(|e| ApiErr::local(format!("failed to read exported file: {e}")))?;
    let _ = std::fs::remove_file(&temp_file);
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_automations_list_contains_interactive_warmup() {
        let list = automations_list().await.expect("automations_list failed");
        assert!(!list.is_empty(), "automations list should not be empty");
        let warmup = list.iter().find(|a| a.id == "interactive-warmup");
        assert!(warmup.is_some(), "interactive-warmup must be present in automations_list");
        let w = warmup.unwrap();
        assert_eq!(w.name, "Interactive Warmup");
        assert!(w.is_builtin);
    }
}


