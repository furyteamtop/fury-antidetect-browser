// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! `fury-agent mcp`: Fury as an MCP server, for Claude Desktop, Cursor, Claude
//! Code and any other client that speaks the Model Context Protocol.
//!
//! It is this binary rather than a script because a script needs an
//! interpreter, and the Python one that came before it (tools/mcp/fury-mcp.py,
//! removed in 0.2.18) also needed the local HTTP API switched on with an
//! environment variable no ordinary install sets. So it worked for whoever
//! wrote it. A tester looking for MCP "like AdsPower has" found the paragraph
//! in the README and nothing in the application (08.10.2026).
//!
//! Every installed copy has this binary, so the desktop can point a client at
//! it with one button. It talks to the agent over the same socket the desktop
//! uses -- no port, no token -- and starts the agent if nothing answers.
//!
//! What it may do is a fixed list (TOOLS), and that list is the whole surface:
//! there is no "call any method" tool. It can read the profile list, open and
//! close profiles, warm them, and drive the page of one it opened. It cannot
//! delete, edit, export, or read cookies and passwords; proxy passwords are cut
//! out of everything it returns, because what it returns goes to a model
//! provider's servers.
//!
//! Protocol: newline-delimited JSON-RPC 2.0 on stdin/stdout. Nothing else may
//! ever be written to stdout -- logging goes to stderr and the log file.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The skill, delivered by the server itself: clients put `instructions` from
/// the handshake in front of the model, so it knows how Fury works without the
/// person installing anything else. The same text ships as SKILL.md from the
/// desktop for clients that read skills from disk.
pub const SKILL: &str = include_str!("../../shared/mcp/SKILL.md");

/// The skill without its front matter, which is for skill loaders, not for
/// the model.
pub fn instructions() -> &'static str {
    SKILL
        .strip_prefix("---")
        .and_then(|rest| rest.split_once("\n---"))
        .map(|(_, body)| body.trim_start())
        .unwrap_or(SKILL)
}

type Reply = Pin<Box<dyn Future<Output = anyhow::Result<Value>> + Send>>;
/// One request to the agent. A function rather than the socket itself, so the
/// tests can answer for the agent.
pub type Backend = dyn Fn(&'static str, Value) -> Reply + Send + Sync;

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
}

fn profile_arg() -> Value {
    json!({ "type": "string", "description": "Profile id, or its exact name as shown in Fury" })
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "fury_status",
        description: "Fury's version, whether the browser is installed, and which profiles are open.",
        schema: || json!({ "type": "object", "properties": {} }),
    },
    Tool {
        name: "fury_list_profiles",
        description: "The profiles on this machine: id, name, tags, stage, project, proxy and whether each is open. \
                      Filter by text, tag or project; with none, everything.",
        schema: || json!({ "type": "object", "properties": {
            "query": { "type": "string", "description": "Part of the name, a tag or a proxy host" },
            "tag": { "type": "string", "description": "Only profiles with this tag" },
            "project": { "type": "string", "description": "Only profiles in this project (name)" },
            "open_only": { "type": "boolean", "description": "Only profiles that are open now" },
        }}),
    },
    Tool {
        name: "fury_list_proxies",
        description: "Saved proxies: name, type, host, port, last seen country and IP. Passwords are never returned.",
        schema: || json!({ "type": "object", "properties": {} }),
    },
    Tool {
        name: "fury_start_profile",
        description: "Open a profile's browser window, through its proxy, ready to be driven with the page tools. \
                      A visible window opens on the person's screen.",
        schema: || json!({ "type": "object", "properties": { "profile": profile_arg() }, "required": ["profile"] }),
    },
    Tool {
        name: "fury_stop_profile",
        description: "Close a profile's browser. Cookies and history are kept in the profile.",
        schema: || json!({ "type": "object", "properties": { "profile": profile_arg() }, "required": ["profile"] }),
    },
    Tool {
        name: "fury_open_url",
        description: "Go to an address in an open profile's current tab and wait for it to load.",
        schema: || json!({ "type": "object", "properties": {
            "profile": profile_arg(),
            "url": { "type": "string", "description": "http(s) address; a bare domain is taken as https" },
        }, "required": ["profile", "url"] }),
    },
    Tool {
        name: "fury_read_page",
        description: "The current page of an open profile: address, title, visible text, and a numbered list of \
                      links, buttons and fields. Use the numbers with fury_click and fury_type.",
        schema: || json!({ "type": "object", "properties": {
            "profile": profile_arg(),
            "max_chars": { "type": "integer", "description": "Text to return, default 8000, at most 100000" },
        }, "required": ["profile"] }),
    },
    Tool {
        name: "fury_click",
        description: "Click element N from the last fury_read_page, as a mouse click at its position.",
        schema: || json!({ "type": "object", "properties": {
            "profile": profile_arg(),
            "element": { "type": "integer", "description": "The number from fury_read_page" },
        }, "required": ["profile", "element"] }),
    },
    Tool {
        name: "fury_type",
        description: "Type text into element N (it is clicked first), or into whatever has focus. \
                      submit presses Enter afterwards.",
        schema: || json!({ "type": "object", "properties": {
            "profile": profile_arg(),
            "text": { "type": "string" },
            "element": { "type": "integer", "description": "The number from fury_read_page" },
            "submit": { "type": "boolean" },
        }, "required": ["profile", "text"] }),
    },
    Tool {
        name: "fury_screenshot",
        description: "A picture of what an open profile's tab shows now.",
        schema: || json!({ "type": "object", "properties": { "profile": profile_arg() }, "required": ["profile"] }),
    },
    Tool {
        name: "fury_warm_up",
        description: "Warm profiles: open each, visit sites with human-like pauses and scrolling so it collects \
                      ordinary cookies, optionally close it after. Runs in the background; see fury_warm_status.",
        schema: || json!({ "type": "object", "properties": {
            "profiles": { "type": "array", "items": { "type": "string" }, "description": "Ids or exact names" },
            "urls": { "type": "array", "items": { "type": "string" }, "description": "Sites to visit; default is Fury's list" },
            "close_after": { "type": "boolean", "description": "Close each profile when done (default true)" },
        }, "required": ["profiles"] }),
    },
    Tool {
        name: "fury_warm_status",
        description: "Progress of warm-ups: which site each profile is on, cookies collected, finished or failed.",
        schema: || json!({ "type": "object", "properties": {} }),
    },
];

pub async fn serve() -> anyhow::Result<()> {
    let backend: Box<Backend> = Box::new(|method, params| Box::pin(agent_call(method, params)));
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(msg) => handle(&*backend, msg).await,
            Err(e) => Some(json!({
                "jsonrpc": "2.0", "id": null,
                "error": { "code": -32700, "message": format!("not JSON: {e}") },
            })),
        };
        if let Some(reply) = reply {
            let mut bytes = serde_json::to_vec(&reply)?;
            bytes.push(b'\n');
            out.write_all(&bytes).await?;
            out.flush().await?;
        }
    }
    Ok(())
}

/// One message in, at most one out. Notifications get nothing back.
pub async fn handle(backend: &Backend, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned()?;
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let ok = |result: Value| Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    let err = |code: i64, message: String| {
        Some(json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }))
    };
    match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
            let version = PROTOCOLS.iter().find(|v| **v == asked).copied().unwrap_or(PROTOCOLS[0]);
            ok(json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "fury", "title": "Fury", "version": env!("CARGO_PKG_VERSION") },
                "instructions": instructions(),
            }))
        }
        "ping" => ok(json!({})),
        "tools/list" => ok(json!({ "tools": TOOLS.iter().map(|t| json!({
            "name": t.name, "description": t.description, "inputSchema": (t.schema)(),
        })).collect::<Vec<_>>() })),
        // Asked for by some clients whatever the capabilities say; an empty
        // answer is better than an error in their log.
        "resources/list" => ok(json!({ "resources": [] })),
        "prompts/list" => ok(json!({ "prompts": [] })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            if !TOOLS.iter().any(|t| t.name == name) {
                return err(-32602, format!("no tool {name:?}"));
            }
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            // A failure is the tool's answer, not a protocol error: the model
            // reads it and can do something about it.
            match call_tool(backend, name, &args).await {
                Ok(content) => ok(json!({ "content": content, "isError": false })),
                Err(e) => ok(json!({ "content": [{ "type": "text", "text": format!("{e:#}") }], "isError": true })),
            }
        }
        _ => err(-32601, format!("no method {method:?}")),
    }
}

fn text(v: &Value) -> Vec<Value> {
    vec![json!({ "type": "text", "text": serde_json::to_string_pretty(v).unwrap_or_default() })]
}

fn s<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|v| !v.is_empty())
}

async fn call_tool(backend: &Backend, name: &str, args: &Value) -> anyhow::Result<Vec<Value>> {
    match name {
        "fury_status" => {
            let st = backend("status", json!({})).await?;
            Ok(text(&json!({
                "version": st.get("version"),
                "browser_installed": st.get("core").is_some_and(|c| !c.is_null()),
                "browser_problem": st.get("core_problem"),
                "open_profiles": st.get("running"),
            })))
        }
        "fury_list_profiles" => {
            let all = backend("profiles.list", json!({})).await?;
            let rows: Vec<Value> = all
                .as_array()
                .map(|a| a.iter().filter(|p| keep(p, args)).map(summary).collect())
                .unwrap_or_default();
            Ok(text(&json!({ "count": rows.len(), "profiles": rows })))
        }
        "fury_list_proxies" => {
            let all = backend("proxies.list", json!({})).await?;
            let rows: Vec<Value> = all.as_array().map(|a| a.iter().map(proxy_summary).collect()).unwrap_or_default();
            Ok(text(&json!({ "count": rows.len(), "proxies": rows })))
        }
        "fury_start_profile" => {
            let (id, name) = resolve(backend, args).await?;
            let r = backend("profile.launch", json!({ "id": id, "cdp": true })).await?;
            if r.get("ws_endpoint").is_none() {
                // The launch worked but the core refused the debugging port: a
                // restriction on this profile, and the page tools will say so.
                return Ok(text(&json!({ "opened": name, "id": id, "page_tools": false,
                    "note": "the window is open, but this profile does not allow automation" })));
            }
            Ok(text(&json!({ "opened": name, "id": id, "page_tools": true })))
        }
        "fury_stop_profile" => {
            let (id, name) = resolve(backend, args).await?;
            backend("profile.stop", json!({ "id": id })).await?;
            Ok(text(&json!({ "closed": name, "id": id })))
        }
        "fury_open_url" => {
            let (id, _) = resolve(backend, args).await?;
            let url = s(args, "url").ok_or_else(|| anyhow::anyhow!("url is required"))?;
            Ok(text(&backend("page.navigate", json!({ "id": id, "url": url })).await?))
        }
        "fury_read_page" => {
            let (id, _) = resolve(backend, args).await?;
            let mut p = json!({ "id": id });
            if let Some(n) = args.get("max_chars").and_then(Value::as_u64) {
                p["max_chars"] = json!(n);
            }
            Ok(text(&backend("page.read", p).await?))
        }
        "fury_click" => {
            let (id, _) = resolve(backend, args).await?;
            let n = args.get("element").and_then(Value::as_u64).ok_or_else(|| anyhow::anyhow!("element is required"))?;
            Ok(text(&backend("page.click", json!({ "id": id, "element": n })).await?))
        }
        "fury_type" => {
            let (id, _) = resolve(backend, args).await?;
            let mut p = json!({
                "id": id,
                "text": args.get("text").and_then(Value::as_str).unwrap_or_default(),
                "submit": args.get("submit").and_then(Value::as_bool).unwrap_or(false),
            });
            if let Some(n) = args.get("element").and_then(Value::as_u64) {
                p["element"] = json!(n);
            }
            Ok(text(&backend("page.type", p).await?))
        }
        "fury_screenshot" => {
            let (id, _) = resolve(backend, args).await?;
            let mut r = backend("page.screenshot", json!({ "id": id })).await?;
            let image = r.as_object_mut().and_then(|o| o.remove("image")).unwrap_or(Value::Null);
            let mut content = text(&r);
            if let (Some(data), Some(mime)) = (image.get("data"), image.get("mime")) {
                content.push(json!({ "type": "image", "data": data, "mimeType": mime }));
            }
            Ok(content)
        }
        "fury_warm_up" => {
            let names: Vec<String> = args
                .get("profiles")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                .unwrap_or_default();
            if names.is_empty() {
                anyhow::bail!("profiles: name at least one");
            }
            let mut ids = Vec::new();
            for n in &names {
                ids.push(resolve(backend, &json!({ "profile": n })).await?.0);
            }
            let urls: Vec<String> = match args.get("urls").and_then(Value::as_array) {
                Some(a) if !a.is_empty() => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
                _ => backend("warm.defaults", json!({}))
                    .await?
                    .get("urls")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default(),
            };
            let plan = json!({
                "urls": urls,
                "close_after": args.get("close_after").and_then(Value::as_bool).unwrap_or(true),
            });
            Ok(text(&backend("warm.start", json!({ "profile_ids": ids, "plan": plan })).await?))
        }
        "fury_warm_status" => Ok(text(&backend("warm.status", json!({})).await?)),
        other => anyhow::bail!("no tool {other:?}"),
    }
}

fn keep(p: &Value, args: &Value) -> bool {
    let field = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_lowercase();
    let tags: Vec<String> = p
        .get("tags")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_lowercase).collect())
        .unwrap_or_default();
    if let Some(t) = s(args, "tag") {
        if !tags.contains(&t.to_lowercase()) {
            return false;
        }
    }
    if let Some(pr) = s(args, "project") {
        if field("project_name") != pr.to_lowercase() {
            return false;
        }
    }
    if args.get("open_only").and_then(Value::as_bool) == Some(true)
        && p.get("running").and_then(Value::as_bool) != Some(true)
    {
        return false;
    }
    if let Some(q) = s(args, "query") {
        let q = q.to_lowercase();
        let host = p.pointer("/proxy/host").and_then(Value::as_str).unwrap_or_default().to_lowercase();
        if !(field("name").contains(&q) || tags.iter().any(|t| t.contains(&q)) || host.contains(&q)) {
            return false;
        }
    }
    true
}

/// What the model needs to choose a profile, and nothing it should not carry
/// off: no proxy password, no notes (operators keep logins in them).
fn summary(p: &Value) -> Value {
    let proxy = p.get("proxy").filter(|v| !v.is_null()).map(|x| {
        json!({
            "name": x.get("name"), "type": x.get("kind"), "host": x.get("host"), "port": x.get("port"),
            "country": x.get("last_country"),
        })
    });
    json!({
        "id": p.get("id"),
        "name": p.get("name"),
        "tags": p.get("tags"),
        "stage": p.get("status"),
        "project": p.get("project_name"),
        "proxy": proxy,
        "open": p.get("running"),
        "last_opened": p.get("last_opened_at"),
    })
}

fn proxy_summary(x: &Value) -> Value {
    json!({
        "id": x.get("id"), "name": x.get("name"), "type": x.get("kind"),
        "host": x.get("host"), "port": x.get("port"), "username": x.get("username"),
        "country": x.get("last_country"), "ip": x.get("last_ip"),
    })
}

/// An id or an exact name to (id, name). Names are what a person says; ids
/// are what the agent takes.
async fn resolve(backend: &Backend, args: &Value) -> anyhow::Result<(String, String)> {
    let want = s(args, "profile")
        .or_else(|| s(args, "id"))
        .ok_or_else(|| anyhow::anyhow!("profile is required: an id or a name from fury_list_profiles"))?;
    let all = backend("profiles.list", json!({})).await?;
    let all = all.as_array().cloned().unwrap_or_default();
    let name_of = |p: &Value| p.get("name").and_then(Value::as_str).unwrap_or_default().to_string();
    if let Some(p) = all.iter().find(|p| p.get("id").and_then(Value::as_str) == Some(want)) {
        return Ok((want.to_string(), name_of(p)));
    }
    let matches: Vec<&Value> = all.iter().filter(|p| name_of(p).eq_ignore_ascii_case(want)).collect();
    match matches.as_slice() {
        [one] => Ok((one.get("id").and_then(Value::as_str).unwrap_or_default().to_string(), name_of(one))),
        [] => anyhow::bail!("no profile {want:?} on this machine; fury_list_profiles shows the ones there are"),
        _ => anyhow::bail!("{} profiles are called {want:?}; use the id from fury_list_profiles", matches.len()),
    }
}

/// One request to the running agent over its socket, starting it first if
/// nothing answers -- Fury does not have to be open for the assistant to work.
async fn agent_call(method: &'static str, params: Value) -> anyhow::Result<Value> {
    let endpoint = crate::paths::ipc_endpoint();
    let stream = match fury_platform::Stream::connect(&endpoint).await {
        Ok(s) => s,
        Err(_) => {
            start_agent()?;
            let mut tries = 0;
            loop {
                tokio::time::sleep(Duration::from_millis(150)).await;
                match fury_platform::Stream::connect(&endpoint).await {
                    Ok(s) => break s,
                    Err(_) if tries < 60 => tries += 1,
                    Err(e) => anyhow::bail!("Fury's agent did not start: {e}. Open the Fury app once and try again"),
                }
            }
        }
    };
    let (read, mut write) = stream.into_split();
    let mut bytes = serde_json::to_vec(&json!({ "id": 1, "method": method, "params": params }))?;
    bytes.push(b'\n');
    write.write_all(&bytes).await?;
    write.flush().await?;
    let mut line = String::new();
    // Opening a profile pulls nothing over MCP (local profiles only), but a
    // first launch can still take a while; a page load is bounded inside.
    let within = if method == "profile.launch" || method.starts_with("page.") { 120 } else { 30 };
    tokio::time::timeout(Duration::from_secs(within), BufReader::new(read).read_line(&mut line))
        .await
        .map_err(|_| anyhow::anyhow!("Fury's agent did not answer {method}"))??;
    let r: Value = serde_json::from_str(&line)?;
    if let Some(e) = r.get("err").and_then(Value::as_str) {
        anyhow::bail!("{e}");
    }
    Ok(r.get("ok").cloned().unwrap_or(Value::Null))
}

/// The same start the desktop does (desktop/src-tauri/src/agent.rs), with the
/// same reason for CREATE_NO_WINDOW.
fn start_agent() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake() -> Box<Backend> {
        Box::new(|method, params| {
            Box::pin(async move {
                Ok(match method {
                    "profiles.list" => json!([
                        { "id": "p1", "name": "Shop DE", "tags": ["warm"], "status": "warming", "project_name": "EU",
                          "notes": "login: me / pass: secret", "running": false,
                          "proxy": { "name": "de1", "kind": "socks5", "host": "de.exit", "port": 1080,
                                     "username": "u", "password": "hunter2", "last_country": "DE" } },
                        { "id": "p2", "name": "Shop FR", "tags": [], "running": true, "proxy": null },
                    ]),
                    "proxies.list" => json!([{ "id": "x", "name": "de1", "kind": "socks5", "host": "de.exit",
                                              "port": 1080, "username": "u", "password": "hunter2" }]),
                    "profile.launch" => json!({ "pid": 1, "ws_endpoint": "ws://127.0.0.1:1/x", "echo": params }),
                    "page.screenshot" => json!({ "url": "https://a", "image": { "mime": "image/jpeg", "data": "QUJD" } }),
                    _ => json!({ "method": method, "params": params }),
                })
            })
        })
    }

    async fn call(b: &Backend, name: &str, args: Value) -> Value {
        handle(b, json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call",
                          "params": { "name": name, "arguments": args } }))
            .await
            .unwrap()["result"]
            .clone()
    }

    #[tokio::test]
    async fn the_handshake_carries_the_skill_and_a_version_the_client_knows() {
        let b = fake();
        let r = handle(&*b, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
                                    "params": { "protocolVersion": "2025-03-26" } })).await.unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        let skill = r["result"]["instructions"].as_str().unwrap();
        assert!(skill.contains("fury_read_page") && skill.starts_with("# Fury"), "{skill}");
        let r = handle(&*b, json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize",
                                    "params": { "protocolVersion": "1999-01-01" } })).await.unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOLS[0]);
        assert!(handle(&*b, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await.is_none());
    }

    #[tokio::test]
    async fn every_tool_is_listed_with_a_schema() {
        let b = fake();
        let r = handle(&*b, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })).await.unwrap();
        let tools = r["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), TOOLS.len());
        assert!(tools.iter().all(|t| t["inputSchema"]["type"] == "object"));
    }

    #[tokio::test]
    async fn no_password_and_no_notes_leave_through_a_list() {
        let b = fake();
        for tool in ["fury_list_profiles", "fury_list_proxies"] {
            let r = call(&*b, tool, json!({})).await.to_string();
            assert!(!r.contains("hunter2"), "{tool}: {r}");
            assert!(!r.contains("secret"), "{tool}: {r}");
        }
    }

    #[tokio::test]
    async fn profiles_filter_by_tag_and_text() {
        let b = fake();
        let r = call(&*b, "fury_list_profiles", json!({ "tag": "WARM" })).await;
        assert!(r.to_string().contains("Shop DE") && !r.to_string().contains("Shop FR"));
        let r = call(&*b, "fury_list_profiles", json!({ "query": "de.exit" })).await;
        assert!(r.to_string().contains("\\\"count\\\": 1"), "{r}");
    }

    #[tokio::test]
    async fn a_profile_is_found_by_name_and_opened_with_the_port() {
        let b = fake();
        let r = call(&*b, "fury_start_profile", json!({ "profile": "shop de" })).await;
        assert_eq!(r["isError"], false, "{r}");
        assert!(r.to_string().contains("p1"));
        let r = call(&*b, "fury_start_profile", json!({ "profile": "nobody" })).await;
        assert_eq!(r["isError"], true);
    }

    #[tokio::test]
    async fn a_screenshot_is_an_image() {
        let b = fake();
        let r = call(&*b, "fury_screenshot", json!({ "profile": "p2" })).await;
        let content = r["content"].as_array().unwrap();
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["mimeType"], "image/jpeg");
        assert!(!content[0]["text"].as_str().unwrap().contains("QUJD"), "the picture is not repeated as text");
    }

    #[tokio::test]
    async fn an_unknown_tool_is_refused() {
        let b = fake();
        let r = handle(&*b, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                                    "params": { "name": "profiles.delete", "arguments": {} } })).await.unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }
}
