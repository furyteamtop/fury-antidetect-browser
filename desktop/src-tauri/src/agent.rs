// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Talking to the local agent.
//!
//! The agent is a separate process because it outlives this window: it holds
//! profile locks, keeps relays up and serves the automation API while the UI is
//! closed (docs/01). The shell is one of its clients, not its owner.
//!
//! One connection per request. The protocol is newline-delimited JSON and the
//! calls are short, so a pool would buy nothing and would add the one failure
//! mode this must not have — a stale connection to an agent that has restarted,
//! reported to the operator as "the agent is not running".

use std::path::PathBuf;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    /// Nothing is listening. Distinct from every other failure because it is
    /// the only one with an action attached: start the agent.
    #[error("the local agent is not running")]
    NotRunning,

    #[error("{0}")]
    Refused(String),

    #[error("agent connection failed: {0}")]
    Io(#[from] std::io::Error),

    #[error("agent spoke a shape this version does not understand: {0}")]
    Protocol(String),
}

// What the agent answers with. Mirrors `agent/src/store.rs`; deserialised into
// named types rather than passed through as JSON so a field renamed on one side
// stops the build instead of quietly emptying a column.

#[derive(Debug, serde::Deserialize)]
pub struct LocalProject {
    pub id: String,
    pub name: String,
    pub profile_count: i64,
}

#[derive(Debug, serde::Deserialize)]
pub struct LocalProxy {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub host: String,
    pub port: u16,
    pub last_country: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct LocalProfile {
    pub id: String,
    /// Absent when the profile is in no project — which is an ordinary state,
    /// not an error: the Profiles list is every profile, and a project is a
    /// grouping one can be moved out of.
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_name: Option<String>,
    pub name: String,
    pub tags: Vec<String>,
    pub persona_id: String,
    pub fp_seed: i64,
    pub proxy: Option<LocalProxy>,
    /// Absent from the trash listing, where nothing is running by definition.
    /// Declared as a plain `bool` at first, and a missing plain field is a hard
    /// serde error — so every trash response failed to parse and the view
    /// reported an empty trash instead of a broken one.
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub languages: Option<Vec<String>>,
    #[serde(default)]
    pub running: bool,
    pub last_opened_at: Option<String>,
}

/// Where the agent's socket is.
///
/// Must match `agent/src/paths.rs` exactly, including the tag derived from the
/// data directory — the shell and the agent find each other by computing the
/// same path from the same inputs, and there is no discovery step to fall back
/// on if they disagree.
pub fn socket_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("FURY_SOCKET") {
        return PathBuf::from(explicit);
    }

    let tag = short_tag(&data_dir());

    #[cfg(target_os = "macos")]
    let base = std::env::temp_dir();

    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());

    base.join(format!("fury-{tag}.sock"))
}

fn data_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("FURY_HOME") {
        return PathBuf::from(explicit);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());

    #[cfg(target_os = "macos")]
    let dir = PathBuf::from(home).join("Library/Application Support/Fury");

    #[cfg(all(unix, not(target_os = "macos")))]
    let dir = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&home).join(".local/share"))
        .join("fury");

    dir
}

fn short_tag(of: &std::path::Path) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(of.as_os_str().as_encoded_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// One request, one response.
pub async fn call<T: DeserializeOwned>(method: &str, params: Value) -> Result<T, AgentError> {
    let path = socket_path();
    let stream = match UnixStream::connect(&path).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
            || e.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            return Err(AgentError::NotRunning)
        }
        Err(e) => return Err(AgentError::Io(e)),
    };

    let (read, mut write) = stream.into_split();
    let request = serde_json::json!({ "id": 1, "method": method, "params": params });
    let mut bytes = serde_json::to_vec(&request).map_err(|e| AgentError::Protocol(e.to_string()))?;
    bytes.push(b'\n');
    write.write_all(&bytes).await?;
    write.flush().await?;

    let mut line = String::new();
    // A bounded wait: a wedged agent must surface as an error the operator can
    // act on, not as a window that never finishes loading.
    tokio::time::timeout(
        Duration::from_secs(30),
        BufReader::new(read).read_line(&mut line),
    )
    .await
    .map_err(|_| AgentError::Protocol("the agent did not answer".into()))??;

    let response: Value =
        serde_json::from_str(&line).map_err(|e| AgentError::Protocol(e.to_string()))?;

    if let Some(err) = response.get("err").and_then(Value::as_str) {
        return Err(AgentError::Refused(err.to_string()));
    }
    let ok = response
        .get("ok")
        .ok_or_else(|| AgentError::Protocol("answer had neither ok nor err".into()))?;

    serde_json::from_value(ok.clone()).map_err(|e| AgentError::Protocol(e.to_string()))
}

/// Starts the agent if nothing answers.
///
/// The alternative — telling the operator to open a terminal — would make the
/// desktop app useless to exactly the people it is for. Spawned detached, so it
/// keeps holding locks and relays after this window closes, which is the whole
/// reason it is a separate process.
pub async fn ensure_running() -> Result<(), AgentError> {
    if UnixStream::connect(socket_path()).await.is_ok() {
        return Ok(());
    }

    let binary = agent_binary().ok_or(AgentError::NotRunning)?;
    std::process::Command::new(&binary)
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;

    // The socket appears a moment after the process does. Poll rather than
    // sleep once: on a cold start the database has to be created first.
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if UnixStream::connect(socket_path()).await.is_ok() {
            return Ok(());
        }
    }
    Err(AgentError::NotRunning)
}

fn agent_binary() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var("FURY_AGENT") {
        let path = PathBuf::from(explicit);
        return path.exists().then_some(path);
    }
    // Beside this binary in a packaged app; beside the shell's own build output
    // in a development tree, which is the same directory either way.
    let beside = std::env::current_exe().ok()?.parent()?.join("fury-agent");
    beside.exists().then_some(beside)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_path_matches_the_agents() {
        // Both sides compute it independently; if this drifts, the shell looks
        // for a socket nobody is listening on and reports "not running" forever.
        let path = socket_path();
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("fury-"), "{name}");
        assert!(name.ends_with(".sock"), "{name}");
        assert_eq!(name.len(), "fury-".len() + 8 + ".sock".len());
        assert!(path.as_os_str().len() < 100);
    }

    #[test]
    fn the_tag_follows_the_data_directory() {
        assert_ne!(
            short_tag(std::path::Path::new("/home/a/Fury")),
            short_tag(std::path::Path::new("/home/b/Fury"))
        );
    }
}
