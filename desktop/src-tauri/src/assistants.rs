// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Connecting AI assistants to Fury: the Settings screen behind "AI assistants".
//!
//! The MCP server is `fury-agent mcp` (agent/src/mcp.rs), and every installed
//! copy already has the binary. What was missing was the step between: a person
//! had to find the paragraph in the README, find the binary inside the app, and
//! edit a JSON file by hand. A tester went looking for MCP "like AdsPower has",
//! did not find it, and said that is where people get lost and leave
//! (08.10.2026). So this writes the entry itself.
//!
//! What it touches: the `mcpServers.fury` entry of each client's config, and
//! nothing else in that file. A file that does not parse is left alone and the
//! person is told, rather than "repaired" into something they did not write.
//! The first time a file is changed, the original is kept beside it.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value};

use crate::commands::ApiErr;

/// The skill, the same text the MCP server sends in its handshake.
const SKILL: &str = include_str!("../../../shared/mcp/SKILL.md");
const ENTRY: &str = "fury";

#[derive(Serialize)]
pub struct Client {
    /// "claude_desktop" | "cursor"
    id: &'static str,
    /// The application looks installed: its configuration directory exists.
    installed: bool,
    /// Our entry is there and points at this copy's agent.
    connected: bool,
    /// Our entry is there but points somewhere else: a moved or older install.
    stale: bool,
    config: String,
}

#[derive(Serialize)]
pub struct State {
    agent: Option<String>,
    /// Why the agent path cannot be handed to anything that outlives this
    /// session, when it cannot: run from the disk image, or from a quarantine
    /// copy macOS made.
    agent_problem: Option<&'static str>,
    clients: Vec<Client>,
    claude_code: Option<String>,
    snippet: Option<String>,
    skill_installed: bool,
    skill_path: String,
}

fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    PathBuf::from(std::env::var(var).unwrap_or_else(|_| ".".into()))
}

/// Where each client keeps its MCP servers. Claude Desktop from the Microsoft
/// Store keeps its files in a package directory instead of %APPDATA%; both are
/// returned and every one that exists is written.
fn configs(id: &str) -> Vec<PathBuf> {
    match id {
        "claude_desktop" => {
            #[cfg(target_os = "macos")]
            {
                vec![home().join("Library/Application Support/Claude/claude_desktop_config.json")]
            }
            #[cfg(windows)]
            {
                let mut v = Vec::new();
                if let Ok(appdata) = std::env::var("APPDATA") {
                    v.push(PathBuf::from(appdata).join("Claude").join("claude_desktop_config.json"));
                }
                if let Ok(local) = std::env::var("LOCALAPPDATA") {
                    if let Ok(entries) = std::fs::read_dir(PathBuf::from(local).join("Packages")) {
                        for e in entries.flatten() {
                            if e.file_name().to_string_lossy().starts_with("Claude_") {
                                v.push(e.path().join("LocalCache/Roaming/Claude/claude_desktop_config.json"));
                            }
                        }
                    }
                }
                v
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            {
                vec![home().join(".config/Claude/claude_desktop_config.json")]
            }
        }
        "cursor" => vec![home().join(".cursor").join("mcp.json")],
        _ => Vec::new(),
    }
}

fn skill_path() -> PathBuf {
    home().join(".claude").join("skills").join("fury").join("SKILL.md")
}

fn agent() -> (Option<PathBuf>, Option<&'static str>) {
    let Some(path) = crate::agent::agent_binary() else {
        return (None, Some("missing"));
    };
    let s = path.to_string_lossy();
    // A path inside the disk image disappears when it is ejected, and a
    // translocated one when the app quits: an assistant pointed there would
    // stop working tomorrow with nothing to say why.
    if s.starts_with("/Volumes/") || s.contains("/AppTranslocation/") {
        return (Some(path), Some("not_installed"));
    }
    (Some(path), None)
}

fn entry(agent: &Path) -> Value {
    json!({ "command": agent.to_string_lossy(), "args": ["mcp"] })
}

fn read(path: &Path) -> Result<Value, ApiErr> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => serde_json::from_str(&text).map_err(|e| {
            ApiErr::local(format!(
                "{} is not valid JSON ({e}). Fix it or remove it, then try again; it was left as it is.",
                path.display()
            ))
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(ApiErr::local(format!("could not read {}: {e}", path.display()))),
    }
}

fn write(path: &Path, value: &Value) -> Result<(), ApiErr> {
    let fail = |e: std::io::Error| ApiErr::local(format!("could not write {}: {e}", path.display()));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(fail)?;
    }
    let backup = path.with_extension("json.before-fury");
    if path.exists() && !backup.exists() {
        std::fs::copy(path, &backup).map_err(fail)?;
    }
    let mut text = serde_json::to_string_pretty(value).unwrap_or_default();
    text.push('\n');
    // Beside it, then over it: a crash half way leaves the old file, not half
    // of a new one.
    let tmp = path.with_extension("json.fury-tmp");
    std::fs::write(&tmp, text).map_err(fail)?;
    std::fs::rename(&tmp, path).map_err(fail)
}

/// Add or replace our entry, keeping everything else in the file.
fn with_entry(mut config: Value, agent: &Path) -> Value {
    if !config.is_object() {
        config = json!({});
    }
    let servers = config
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers.as_object_mut().unwrap().insert(ENTRY.into(), entry(agent));
    config
}

fn without_entry(mut config: Value) -> Value {
    if let Some(servers) = config.get_mut("mcpServers").and_then(Value::as_object_mut) {
        servers.remove(ENTRY);
    }
    config
}

fn ours(config: &Value) -> Option<&Value> {
    config.get("mcpServers")?.get(ENTRY)
}

fn client(id: &'static str, agent: Option<&Path>) -> Client {
    let paths = configs(id);
    let installed = paths.iter().any(|p| p.parent().is_some_and(Path::exists));
    let found: Vec<Value> = paths
        .iter()
        .filter_map(|p| read(p).ok())
        .filter_map(|c| ours(&c).cloned())
        .collect();
    let want = agent.map(entry);
    let connected = !found.is_empty() && want.as_ref().is_some_and(|w| found.iter().all(|f| f == w));
    Client {
        id,
        installed,
        connected,
        stale: !found.is_empty() && !connected,
        config: paths
            .iter()
            .find(|p| p.parent().is_some_and(Path::exists))
            .or(paths.first())
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    }
}

/// Quoted for the shell the person will paste it into.
fn claude_code_command(agent: &Path) -> String {
    let p = agent.to_string_lossy();
    format!("claude mcp add --scope user fury -- \"{p}\" mcp")
}

fn state() -> State {
    let (agent, agent_problem) = agent();
    let usable = agent.as_deref().filter(|_| agent_problem.is_none());
    State {
        agent: agent.as_ref().map(|p| p.display().to_string()),
        agent_problem,
        clients: vec![client("claude_desktop", usable), client("cursor", usable)],
        claude_code: usable.map(claude_code_command),
        snippet: usable.map(|a| {
            serde_json::to_string_pretty(&json!({ "mcpServers": { ENTRY: entry(a) } })).unwrap_or_default()
        }),
        skill_installed: std::fs::read_to_string(skill_path()).is_ok_and(|s| s == SKILL),
        skill_path: skill_path().display().to_string(),
    }
}

#[tauri::command]
pub async fn assistants_state() -> Result<State, ApiErr> {
    Ok(state())
}

#[tauri::command]
pub async fn assistants_connect(client: String) -> Result<State, ApiErr> {
    let (agent, problem) = agent();
    let agent = match (agent, problem) {
        (Some(a), None) => a,
        (_, Some("not_installed")) => {
            return Err(ApiErr::coded(
                "err.aiNotInstalled",
                "Fury is running from the disk image or Downloads. Move it to Applications, open it from there, and connect again.",
            ))
        }
        _ => return Err(ApiErr::local("the fury-agent binary was not found beside the application")),
    };
    let paths = configs(&client);
    if paths.is_empty() {
        return Err(ApiErr::local(format!("unknown assistant {client:?}")));
    }
    // Every location that exists, or the first if none does: Claude Desktop
    // creates its directory on first run, and writing ahead of it is harmless.
    let existing: Vec<&PathBuf> = paths.iter().filter(|p| p.parent().is_some_and(Path::exists)).collect();
    let targets: Vec<&PathBuf> = if existing.is_empty() { vec![&paths[0]] } else { existing };
    for path in targets {
        let config = read(path)?;
        write(path, &with_entry(config, &agent))?;
    }
    Ok(state())
}

#[tauri::command]
pub async fn assistants_disconnect(client: String) -> Result<State, ApiErr> {
    for path in configs(&client) {
        if !path.exists() {
            continue;
        }
        let config = read(&path)?;
        if ours(&config).is_some() {
            write(&path, &without_entry(config))?;
        }
    }
    Ok(state())
}

/// For Claude Code, which reads skills from ~/.claude/skills. Claude Desktop
/// and Cursor need no file: they get the same text from the server itself.
#[tauri::command]
pub async fn assistants_install_skill() -> Result<State, ApiErr> {
    let path = skill_path();
    let fail = |e: std::io::Error| ApiErr::local(format!("could not write {}: {e}", path.display()));
    std::fs::create_dir_all(path.parent().unwrap()).map_err(fail)?;
    std::fs::write(&path, SKILL).map_err(fail)?;
    Ok(state())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_entry_goes_in_and_everything_else_stays() {
        let before = json!({
            "preferences": { "x": 1 },
            "mcpServers": { "other": { "command": "node", "args": ["a.js"] } },
        });
        let after = with_entry(before.clone(), Path::new("/Applications/Fury.app/Contents/MacOS/fury-agent"));
        assert_eq!(after["preferences"], before["preferences"]);
        assert_eq!(after["mcpServers"]["other"], before["mcpServers"]["other"]);
        assert_eq!(after["mcpServers"]["fury"]["args"], json!(["mcp"]));
        let removed = without_entry(after);
        assert_eq!(removed, before);
    }

    #[test]
    fn a_config_with_no_servers_gets_the_section() {
        let after = with_entry(json!({ "preferences": {} }), Path::new("/x/fury-agent"));
        assert_eq!(after["mcpServers"]["fury"]["command"], "/x/fury-agent");
    }

    #[test]
    fn a_broken_file_is_refused_and_left_alone() {
        let dir = std::env::temp_dir().join(format!("fury-assist-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("c.json");
        std::fs::write(&f, "{ not json").unwrap();
        assert!(read(&f).is_err());
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "{ not json");
        std::fs::write(&f, "{\"a\":1}").unwrap();
        write(&f, &with_entry(read(&f).unwrap(), Path::new("/x/fury-agent"))).unwrap();
        assert!(dir.join("c.json.before-fury").exists(), "the original is kept");
        assert_eq!(read(&f).unwrap()["a"], 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_skill_is_the_one_the_server_sends() {
        assert!(SKILL.starts_with("---\nname: fury"));
        assert!(SKILL.contains("fury_read_page"));
    }
}
