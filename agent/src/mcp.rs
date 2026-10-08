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
//! there is no "call any method" tool. It can read the profile list, create
//! profiles and save proxies, rename, tag and re-proxy a profile, move one to
//! the Trash, open and close profiles, warm them, and drive the page of one it
//! opened. Creating was added on a tester's request (08.10.2026): "AdsPower's
//! AI does everything". It cannot erase a profile, change a fingerprint,
//! export cookies or read passwords; proxy passwords and notes are cut out of
//! everything it returns, because what it returns goes to a model provider.
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
        name: "fury_list_personas",
        description: "The machines a profile can be: id, OS, GPU, screen, and how common each is among real users. \
                      Prefer common ones; the OS should usually match the person's own computer.",
        schema: || json!({ "type": "object", "properties": {
            "os": { "type": "string", "description": "Only this OS: windows, macos" },
        }}),
    },
    Tool {
        name: "fury_create_profiles",
        description: "Create one profile or a batch on this machine. Each gets its own fingerprint seed. \
                      Without a persona, machines are spread by how common they are (or restricted to `os`). \
                      Give each account its own proxy: profiles sharing one exit are linked by it.",
        schema: || json!({ "type": "object", "properties": {
            "name": { "type": "string", "description": "Name, or a pattern with {n} for a batch, e.g. \"Shop {n}\"" },
            "count": { "type": "integer", "description": "How many, 1-50 (default 1)" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "proxy": { "type": "string", "description": "A saved proxy's id or name (fury_list_proxies)" },
            "proxy_line": { "type": "string", "description": "Or a proxy as a line, e.g. socks5://user:pass@host:port; it is saved first" },
            "persona": { "type": "string", "description": "Persona id from fury_list_personas" },
            "os": { "type": "string", "description": "Or just the OS: windows, macos" },
            "start_urls": { "type": "array", "items": { "type": "string" } },
            "stage": { "type": "string", "description": "The account's stage, e.g. warming" },
        }, "required": ["name"] }),
    },
    Tool {
        name: "fury_update_profile",
        description: "Change a profile on this machine: name, tags, stage, proxy, start pages. \
                      Only the fields given change. The fingerprint is never changed here.",
        schema: || json!({ "type": "object", "properties": {
            "profile": profile_arg(),
            "name": { "type": "string" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "stage": { "type": "string" },
            "proxy": { "type": "string", "description": "A saved proxy's id or name; empty string removes the proxy" },
            "start_urls": { "type": "array", "items": { "type": "string" } },
        }, "required": ["profile"] }),
    },
    Tool {
        name: "fury_add_proxies",
        description: "Save proxies from text, one per line, in any common format (host:port:user:pass, \
                      user:pass@host:port, scheme://...). Returns what was saved and which lines were not understood.",
        schema: || json!({ "type": "object", "properties": {
            "lines": { "type": "string" },
            "name_prefix": { "type": "string", "description": "Optional prefix for their names" },
        }, "required": ["lines"] }),
    },
    Tool {
        name: "fury_move_to_trash",
        description: "Move a profile to Fury's Trash. It can be restored from there; nothing is erased. \
                      Ask the person before doing this.",
        schema: || json!({ "type": "object", "properties": { "profile": profile_arg() }, "required": ["profile"] }),
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
        "fury_list_personas" => {
            let all = backend("personas.list", json!({})).await?;
            let want = s(args, "os").map(os_word);
            let rows: Vec<Value> = all
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter(|p| want.as_deref().is_none_or(|w| os_word(p.get("os").and_then(Value::as_str).unwrap_or_default()) == w))
                        .map(|p| json!({ "id": p.get("id"), "os": p.get("os"), "gpu": p.get("gpu"), "screen": p.get("screen"), "share": p.get("weight") }))
                        .collect()
                })
                .unwrap_or_default();
            Ok(text(&json!({ "count": rows.len(), "personas": rows })))
        }
        "fury_create_profiles" => {
            let count = args.get("count").and_then(Value::as_u64).unwrap_or(1);
            // Fifty, not the five hundred the window allows: a model that
            // misread "5" as "500" should not get to fill the list.
            if !(1..=50).contains(&count) {
                anyhow::bail!("count must be 1-50 here; for more, create them in the Fury window");
            }
            let name = s(args, "name").ok_or_else(|| anyhow::anyhow!("name is required"))?;
            let proxy_id: Option<String> = match (s(args, "proxy"), s(args, "proxy_line")) {
                (Some(p), _) => Some(proxy_ref(backend, p).await?),
                (None, Some(line)) => Some(import_one_proxy(backend, line).await?),
                (None, None) => None,
            };
            let strings = |k: &str| -> Vec<String> {
                args.get(k)
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default()
            };
            let (tags, start_urls) = (strings("tags"), strings("start_urls"));
            let mut created = Vec::new();
            let mut failed = Vec::new();
            for n in 1..=count {
                let this = if name.contains("{n}") {
                    name.replace("{n}", &n.to_string())
                } else if count == 1 {
                    name.to_string()
                } else {
                    format!("{name} {n}")
                };
                // A persona per profile, not one for the batch: a batch on one
                // persona is a crowd of identical machines, which is the one
                // thing a batch of accounts must not look like.
                let persona = match s(args, "persona") {
                    Some(p) => p.to_string(),
                    None => pick_persona(backend, s(args, "os")).await?,
                };
                // Through profiles.upsert, the window's own path: id and seed
                // left empty are generated by the store, a fresh seed each.
                let profile = json!({
                    "id": "", "project_id": null, "name": this, "notes": "",
                    "tags": tags, "status": s(args, "stage").unwrap_or_default(),
                    "persona_id": persona, "fp_seed": 0, "proxy": null, "proxy_id": proxy_id,
                    "timezone": null, "languages": null, "overrides": {},
                    "start_urls": start_urls, "allow_no_proxy": false, "last_opened_at": null,
                });
                match backend("profiles.upsert", profile).await {
                    Ok(r) => created.push(json!({ "name": this, "id": r.get("id"), "persona": persona })),
                    Err(e) => failed.push(json!({ "name": this, "error": format!("{e:#}") })),
                }
            }
            let mut out = json!({ "created": created, "failed": failed });
            if proxy_id.is_none() {
                out["note"] = json!("No proxy: these profiles will not open until one is set (fury_update_profile).");
            } else if count > 1 {
                out["note"] = json!("All of them share one proxy. Give each account its own exit before using them.");
            }
            Ok(text(&out))
        }
        "fury_update_profile" => {
            let (id, _) = resolve(backend, args).await?;
            let all = backend("profiles.list", json!({})).await?;
            let mut p = all
                .as_array()
                .and_then(|a| a.iter().find(|p| p.get("id").and_then(Value::as_str) == Some(&id)).cloned())
                .ok_or_else(|| anyhow::anyhow!("that profile is not on this machine"))?;
            if p.get("running").and_then(Value::as_bool) == Some(true) {
                anyhow::bail!("the profile is open; close it first (fury_stop_profile)");
            }
            if let Some(n) = s(args, "name") { p["name"] = json!(n); }
            if let Some(t) = args.get("tags").filter(|v| v.is_array()) { p["tags"] = t.clone(); }
            if let Some(st) = args.get("stage").and_then(Value::as_str) { p["status"] = json!(st.trim()); }
            if let Some(u) = args.get("start_urls").filter(|v| v.is_array()) { p["start_urls"] = u.clone(); }
            if let Some(px) = args.get("proxy").and_then(Value::as_str) {
                p["proxy_id"] = if px.trim().is_empty() { Value::Null } else { json!(proxy_ref(backend, px.trim()).await?) };
                p["proxy"] = Value::Null;
            }
            backend("profiles.upsert", p).await?;
            Ok(text(&json!({ "updated": id })))
        }
        "fury_add_proxies" => {
            let lines = s(args, "lines").ok_or_else(|| anyhow::anyhow!("lines is required"))?;
            let r = backend("proxies.importMany", json!({ "text": lines, "name_prefix": s(args, "name_prefix").unwrap_or_default() })).await?;
            Ok(text(&r))
        }
        "fury_move_to_trash" => {
            let (id, name) = resolve(backend, args).await?;
            backend("profiles.delete", json!({ "id": id })).await?;
            Ok(text(&json!({ "moved_to_trash": name, "id": id, "restore": "Fury → Trash → Restore" })))
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

/// "Windows 11", "windows", "macOS 15", "mac" -> "windows" | "macos".
fn os_word(os: &str) -> String {
    let o = os.to_lowercase();
    if o.contains("win") { "windows".into() } else if o.contains("mac") || o.contains("os x") { "macos".into() } else { o }
}

/// A saved proxy by id or by name.
async fn proxy_ref(backend: &Backend, want: &str) -> anyhow::Result<String> {
    let all = backend("proxies.list", json!({})).await?;
    let all = all.as_array().cloned().unwrap_or_default();
    let field = |p: &Value, k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    if let Some(p) = all.iter().find(|p| field(p, "id") == want) {
        return Ok(field(p, "id"));
    }
    let by_name: Vec<&Value> = all.iter().filter(|p| field(p, "name").eq_ignore_ascii_case(want)).collect();
    match by_name.as_slice() {
        [one] => Ok(field(one, "id")),
        [] => anyhow::bail!("no saved proxy {want:?}; fury_list_proxies shows them, fury_add_proxies saves new ones"),
        _ => anyhow::bail!("{} proxies are called {want:?}; use the id", by_name.len()),
    }
}

/// Save one proxy line and return its id.
async fn import_one_proxy(backend: &Backend, line: &str) -> anyhow::Result<String> {
    let r = backend("proxies.importMany", json!({ "text": line })).await?;
    if let Some(id) = r.pointer("/saved/0/id").and_then(Value::as_str) {
        return Ok(id.to_string());
    }
    let why = r.pointer("/rejected/0/error").and_then(Value::as_str).unwrap_or("not understood");
    anyhow::bail!("the proxy line was not saved: {why}")
}

/// A persona of this OS, the common ones more likely, as the window does.
async fn pick_persona(backend: &Backend, os: Option<&str>) -> anyhow::Result<String> {
    let want = os.map(os_word);
    let all = backend("personas.list", json!({})).await?;
    let pool: Vec<(String, f64)> = all
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|p| want.as_deref().is_none_or(|w| os_word(p.get("os").and_then(Value::as_str).unwrap_or_default()) == w))
                .map(|p| {
                    (
                        p.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                        p.get("weight").and_then(Value::as_f64).unwrap_or(1.0).max(0.0001),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    if pool.is_empty() {
        anyhow::bail!("no persona for {:?}; fury_list_personas shows what there is", os.unwrap_or_default());
    }
    let total: f64 = pool.iter().map(|(_, w)| w).sum();
    let mut x = rand::random::<f64>() * total;
    for (id, w) in &pool {
        if x < *w {
            return Ok(id.clone());
        }
        x -= w;
    }
    Ok(pool[pool.len() - 1].0.clone())
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
