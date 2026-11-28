// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Driving the page of an open profile: go to an address, read what is there,
//! click, type, press keys, scroll, attach files, take a picture.
//!
//! This is what an assistant connected over MCP needs to do the thing people
//! actually ask it for -- "open the profiles tagged warm, check the inbox,
//! close them" -- and it is the part competitors sell as an "AI agent" running
//! on their servers. Here it runs on the operator's machine, against a browser
//! started with the debugging port on loopback (`profile.launch` with `cdp`).
//!
//! Same discipline as warm.rs: `Runtime.enable` is never called, and scripts
//! run in an isolated world rather than the page's own, so a page cannot see
//! our functions, our globals or our stack frames. The DOM is shared, which is
//! all reading and clicking need. Clicks are real input events at the
//! element's position, not `element.click()`, which a page can tell apart.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cookies::Cdp;

/// How much page text one read returns unless asked for more. A long article
/// is hundreds of kilobytes, and the model on the other end pays for each one.
const DEFAULT_TEXT: usize = 8000;
const MAX_TEXT: usize = 100_000;
/// Interactive elements listed per read. A results page can have thousands of
/// links; the first ones in document order are the ones a person would see.
const MAX_ELEMENTS: usize = 200;

/// The elements a person could act on, in document order. One expression for
/// both reading and clicking, so index N means the same element in both.
///
/// It walks into open shadow roots. A web app built from components (YouTube
/// Studio's settings, 28.11.2026) keeps its radio buttons and drop-downs
/// inside them, where `document.querySelectorAll` does not look: the
/// checkboxes of the same tab were listed, the three radio buttons beside them
/// were not, and the only way left to answer was a click nobody could aim.
/// A shadow root's contents are listed right after its host, which is where a
/// person sees them. Closed roots stay closed: nothing here can reach them.
///
/// Roles beyond buttons and links are the ones a form is made of: radio,
/// switch, combobox, listbox, option, slider and the like. Each comes back
/// with its state (`checked`, `selected`, `expanded`), so an assistant can
/// tell whether a click did what it meant without a screenshot.
const ELEMENTS: &str = r#"(() => {
  const sel = 'a[href], button, input:not([type=hidden]), textarea, select, summary, [role=button], [role=link], [role=tab], [role=menuitem], [role=menuitemradio], [role=menuitemcheckbox], [role=checkbox], [role=radio], [role=switch], [role=combobox], [role=listbox], [role=option], [role=slider], [role=spinbutton], [role=textbox], [role=searchbox], [role=treeitem], [contenteditable=""], [contenteditable=true]';
  const found = [];
  const visit = root => {
    for (const e of root.querySelectorAll('*')) {
      if (e.matches(sel)) found.push(e);
      if (e.shadowRoot) visit(e.shadowRoot);
    }
  };
  visit(document);
  return found.filter(e => {
    const r = e.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return false;
    const s = getComputedStyle(e);
    return s.visibility !== 'hidden' && s.display !== 'none';
  });
})()"#;

/// File inputs, hidden ones included, in the same order across calls. They
/// get a list of their own rather than numbers among ELEMENTS: an upload
/// button is usually a styled element over an `<input type=file>` that is
/// `display: none`, so the input cannot be clicked, only given files.
const FILE_INPUTS: &str = r#"(() => {
  const found = [];
  const visit = root => {
    for (const e of root.querySelectorAll('*')) {
      if (e.tagName === 'INPUT' && e.type === 'file') found.push(e);
      if (e.shadowRoot) visit(e.shadowRoot);
    }
  };
  visit(document);
  return found;
})()"#;

/// The element that has the keyboard, through shadow roots: inside a
/// component `document.activeElement` is the component, not its field.
const FOCUSED: &str = r#"(() => {
  let a = document.activeElement;
  while (a && a.shadowRoot && a.shadowRoot.activeElement) a = a.shadowRoot.activeElement;
  return a;
})()"#;

/// `background`: the profile was started with MCP's background start, so its
/// window is not the active one and the page knows it — document.hasFocus() is
/// false and focus moved away. Clicks and keystrokes arriving at a page that
/// has no focus are not something a person can produce, so for as long as this
/// session is attached Chrome is told to treat the page as focused
/// (Emulation.setFocusEmulationEnabled, upstream's own answer for driving
/// windows that are not in front). It ends with the detach below, and the page
/// sees a focus/blur pair around the action, as it would around somebody
/// switching to the window, doing one thing and switching back.
pub async fn dispatch(method: &str, ws: &str, params: &Value, background: bool) -> anyhow::Result<Value> {
    let mut page = Page::attach(ws).await?;
    if background {
        // Not fatal: without it the action still happens, only from a page
        // that thinks it is unfocused.
        if let Err(e) = page.call("Emulation.setFocusEmulationEnabled", json!({ "enabled": true })).await {
            tracing::warn!(error = format!("{e:#}"), "could not give the background page focus");
        }
    }
    let out = match method {
        "page.navigate" => {
            let url = params.get("url").and_then(Value::as_str).unwrap_or_default();
            page.navigate(url).await
        }
        "page.read" => {
            let max = params
                .get("max_chars")
                .and_then(Value::as_u64)
                .map(|n| (n as usize).min(MAX_TEXT))
                .unwrap_or(DEFAULT_TEXT);
            let within = params.get("within").and_then(Value::as_u64).map(|n| n as usize);
            let filter = params.get("filter").and_then(Value::as_str).map(str::trim).filter(|f| !f.is_empty());
            page.read(max, within, filter).await
        }
        "page.click" => match point(params)? {
            Some((x, y)) => page.click_point(x, y).await,
            None => page.click(element(params)?).await,
        },
        "page.type" => {
            let text = params.get("text").and_then(Value::as_str).unwrap_or_default();
            let target = params.get("element").and_then(Value::as_u64).map(|n| n as usize);
            let submit = params.get("submit").and_then(Value::as_bool).unwrap_or(false);
            let replace = params.get("replace").and_then(Value::as_bool).unwrap_or(false);
            page.type_text(target, text, submit, replace).await
        }
        "page.press" => {
            let combo = params.get("key").and_then(Value::as_str).unwrap_or_default();
            let chord = Chord::parse(combo)?;
            let times = params.get("times").and_then(Value::as_u64).unwrap_or(1).clamp(1, 50) as usize;
            page.press(&chord, times).await
        }
        "page.scroll" => {
            let dx = params.get("dx").and_then(Value::as_f64).unwrap_or(0.0);
            let dy = params.get("dy").and_then(Value::as_f64);
            let target = params.get("element").and_then(Value::as_u64).map(|n| n as usize);
            page.scroll(target, point(params)?, dx, dy).await
        }
        "page.upload" => {
            let files = upload_paths(params)?;
            let input = params.get("input").and_then(Value::as_u64).map(|n| n as usize);
            page.upload(input, &files).await
        }
        "page.screenshot" => page.screenshot().await,
        other => anyhow::bail!("no page method {other:?}"),
    };
    page.detach().await;
    out
}

fn element(params: &Value) -> anyhow::Result<usize> {
    params
        .get("element")
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .ok_or_else(|| anyhow::anyhow!("element: the number from read_page is required"))
}

/// `x` and `y` together, or neither. They are pixels of the picture
/// `screenshot` returns, which is taken at the page's own (CSS) size so that
/// a point in it is a point on the page with no arithmetic in between.
fn point(params: &Value) -> anyhow::Result<Option<(f64, f64)>> {
    match (params.get("x").and_then(Value::as_f64), params.get("y").and_then(Value::as_f64)) {
        (Some(x), Some(y)) if x >= 0.0 && y >= 0.0 => Ok(Some((x, y))),
        (Some(_), Some(_)) => anyhow::bail!("x and y cannot be negative"),
        (None, None) => Ok(None),
        _ => anyhow::bail!("x and y go together: both, or an element number instead"),
    }
}

/// One key, with the modifiers held while it is pressed: "Enter", "Tab",
/// "Shift+Tab", "ArrowDown", "Cmd+A".
#[derive(Debug, PartialEq)]
struct Chord {
    key: String,
    code: String,
    vk: u32,
    /// What the key types with these modifiers, if anything.
    text: String,
    /// CDP's bit mask: Alt 1, Ctrl 2, Meta 4, Shift 8.
    modifiers: u32,
    /// Editing commands the key stands for on macOS; see `press`.
    commands: Vec<&'static str>,
}

impl Chord {
    fn parse(combo: &str) -> anyhow::Result<Self> {
        let parts: Vec<&str> = combo.split('+').map(str::trim).filter(|p| !p.is_empty()).collect();
        let Some((name, mods)) = parts.split_last() else {
            anyhow::bail!("key is required, e.g. Enter, Tab, Escape, ArrowDown, Shift+Tab, Cmd+A")
        };
        let mut modifiers = 0;
        for m in mods {
            modifiers |= match m.to_ascii_lowercase().as_str() {
                "alt" | "option" | "opt" => 1,
                "ctrl" | "control" => 2,
                "cmd" | "command" | "meta" | "win" => 4,
                "shift" => 8,
                other => anyhow::bail!("{other:?} is not a modifier: use Shift, Ctrl, Alt or Cmd"),
            };
        }
        let shift = modifiers & 8 != 0;
        let typing = modifiers & (2 | 4) == 0;
        let named: Option<(&str, &str, u32, &str)> = match name.to_ascii_lowercase().as_str() {
            "enter" | "return" => Some(("Enter", "Enter", 13, "\r")),
            "tab" => Some(("Tab", "Tab", 9, "")),
            "escape" | "esc" => Some(("Escape", "Escape", 27, "")),
            "backspace" => Some(("Backspace", "Backspace", 8, "")),
            "delete" | "del" => Some(("Delete", "Delete", 46, "")),
            "space" => Some((" ", "Space", 32, " ")),
            "arrowup" | "up" => Some(("ArrowUp", "ArrowUp", 38, "")),
            "arrowdown" | "down" => Some(("ArrowDown", "ArrowDown", 40, "")),
            "arrowleft" | "left" => Some(("ArrowLeft", "ArrowLeft", 37, "")),
            "arrowright" | "right" => Some(("ArrowRight", "ArrowRight", 39, "")),
            "home" => Some(("Home", "Home", 36, "")),
            "end" => Some(("End", "End", 35, "")),
            "pageup" => Some(("PageUp", "PageUp", 33, "")),
            "pagedown" => Some(("PageDown", "PageDown", 34, "")),
            _ => None,
        };
        let (key, code, vk, text) = match named {
            Some((k, c, v, t)) => (k.to_string(), c.to_string(), v, if typing { t.to_string() } else { String::new() }),
            None => {
                let mut chars = name.chars();
                let (Some(c), None) = (chars.next(), chars.next()) else {
                    anyhow::bail!(
                        "{name:?} is not a key this knows: a letter, a digit, Enter, Tab, Escape, Backspace, \
                         Delete, Space, an arrow, Home, End, PageUp or PageDown"
                    )
                };
                if !c.is_ascii_alphanumeric() {
                    anyhow::bail!("{name:?}: only letters and digits are keys here; type other characters with type");
                }
                let upper = c.to_ascii_uppercase();
                let code = if c.is_ascii_digit() { format!("Digit{c}") } else { format!("Key{upper}") };
                let key = if shift { upper.to_string() } else { c.to_ascii_lowercase().to_string() };
                let text = if typing { key.clone() } else { String::new() };
                (key, code, upper as u32, text)
            }
        };
        // Chrome on macOS turns these key presses into editing only when the
        // event names the command: a bare Cmd+A reaches the page as a keydown
        // and selects nothing (measured on core 158, 28.11.2026: selection
        // 0..0 without, 0..11 of 11 with). A real keyboard goes through
        // AppKit, which adds them; CDP does not.
        let commands = if cfg!(target_os = "macos") && modifiers & 4 != 0 && modifiers & (1 | 2) == 0 {
            match (key.to_ascii_lowercase().as_str(), shift) {
                ("a", false) => vec!["selectAll"],
                ("c", false) => vec!["copy"],
                ("x", false) => vec!["cut"],
                ("v", false) => vec!["paste"],
                ("z", false) => vec!["undo"],
                ("z", true) => vec!["redo"],
                _ => vec![],
            }
        } else {
            vec![]
        };
        Ok(Self { key, code, vk, text, modifiers, commands })
    }
}

/// Files a page may be given, checked before the browser sees the path.
///
/// What an assistant asks to upload comes from a model, and a model can be
/// talked into things by the page it is reading. So the tool reaches only
/// what a person keeps as their own documents: files under the home folder,
/// never inside a hidden folder (.ssh, .aws, .config, ...), never under
/// ~/Library on macOS or AppData on Windows (other applications' data,
/// browsers' cookies among it), and never Fury's own directory. The path is
/// resolved first, so a link cannot point out of those bounds.
fn upload_paths(params: &Value) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut raw: Vec<&str> = Vec::new();
    if let Some(p) = params.get("path").and_then(Value::as_str) {
        raw.push(p);
    }
    if let Some(list) = params.get("paths").and_then(Value::as_array) {
        raw.extend(list.iter().filter_map(Value::as_str));
    }
    if raw.is_empty() {
        anyhow::bail!("path is required: the full path of the file to upload");
    }
    let home = home_dir().ok_or_else(|| anyhow::anyhow!("cannot tell where the home folder is"))?;
    let home = home.canonicalize().unwrap_or(home);
    let fury = crate::paths::data_dir();
    let fury = fury.canonicalize().unwrap_or(fury);
    raw.iter().map(|p| allowed_upload(p, &home, &fury)).collect()
}

fn home_dir() -> Option<std::path::PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).filter(|v| !v.is_empty()).map(std::path::PathBuf::from)
}

fn allowed_upload(
    path: &str,
    home: &std::path::Path,
    fury: &std::path::Path,
) -> anyhow::Result<std::path::PathBuf> {
    let given = std::path::Path::new(path.trim());
    if !given.is_absolute() {
        anyhow::bail!("{path:?}: give the full path of the file");
    }
    let real = given
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("{path:?}: {e}"))?;
    if !real.is_file() {
        anyhow::bail!("{path:?} is not a file");
    }
    let Ok(rest) = real.strip_prefix(home) else {
        anyhow::bail!("{path:?}: only files in the home folder can be uploaded");
    };
    if real.starts_with(fury) {
        anyhow::bail!("{path:?}: Fury's own files cannot be uploaded");
    }
    let mut parts = rest.components().map(|c| c.as_os_str().to_string_lossy().to_string());
    let first = parts.next().unwrap_or_default();
    let refused_root = if cfg!(target_os = "macos") {
        first == "Library"
    } else if cfg!(windows) {
        first.eq_ignore_ascii_case("AppData")
    } else {
        false
    };
    let dirs: Vec<String> = rest
        .parent()
        .map(|p| p.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect())
        .unwrap_or_default();
    if refused_root || dirs.iter().any(|d| d.starts_with('.')) {
        anyhow::bail!("{path:?}: files in hidden or application folders cannot be uploaded");
    }
    Ok(real)
}

/// Accepts what a person would type in an address bar.
pub fn normalise(url: &str) -> anyhow::Result<String> {
    let url = url.trim();
    if url.is_empty() {
        anyhow::bail!("url is empty");
    }
    // A scheme is letters then a colon not followed by a port number:
    // "javascript:x" has one, "localhost:3000" does not.
    let has_scheme = url.contains("://")
        || url.split_once(':').is_some_and(|(head, rest)| {
            head.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && head.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c))
                && !rest.starts_with(|c: char| c.is_ascii_digit())
        });
    let full = if has_scheme { url.to_string() } else { format!("https://{url}") };
    // chrome:// and file:// reach the browser and the disk rather than a site,
    // and nothing an assistant was asked to do on the web needs either.
    let scheme = full.split(':').next().unwrap_or_default().to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "about") {
        anyhow::bail!("only http and https addresses can be opened, not {scheme}:");
    }
    Ok(full)
}

struct Page {
    cdp: Cdp,
    session: String,
    target: String,
}

impl Page {
    /// The tab the person is looking at: the first page target, which is the
    /// one Chrome lists as most recently active.
    async fn attach(ws: &str) -> anyhow::Result<Self> {
        let mut cdp = Cdp::connect(ws).await?;
        let targets = timed(&mut cdp, None, "Target.getTargets", json!({})).await?;
        let target = targets
            .get("targetInfos")
            .and_then(Value::as_array)
            .and_then(|a| {
                a.iter().find(|t| {
                    t.get("type").and_then(Value::as_str) == Some("page")
                        && !t
                            .get("url")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .starts_with("devtools://")
                })
            })
            .and_then(|t| t.get("targetId").and_then(Value::as_str))
            .ok_or_else(|| anyhow::anyhow!("the browser has no tab open"))?
            .to_string();
        let attached = timed(
            &mut cdp,
            None,
            "Target.attachToTarget",
            json!({ "targetId": target, "flatten": true }),
        )
        .await?;
        let session = attached
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("attaching to the tab gave no session"))?
            .to_string();
        Ok(Self { cdp, session, target })
    }

    async fn detach(mut self) {
        let _ = timed(
            &mut self.cdp,
            None,
            "Target.detachFromTarget",
            json!({ "sessionId": self.session }),
        )
        .await;
    }

    async fn call(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        timed(&mut self.cdp, Some(&self.session), method, params).await
    }

    /// A world of its own on the main frame, for one script. A fresh world per
    /// call: one made earlier dies with the document it was made for.
    async fn world(&mut self) -> anyhow::Result<i64> {
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let frame = tree
            .pointer("/frameTree/frame/id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("the tab has no main frame"))?
            .to_string();
        let world = self
            .call(
                "Page.createIsolatedWorld",
                json!({ "frameId": frame, "worldName": "fury", "grantUniveralAccess": false }),
            )
            .await?;
        world
            .get("executionContextId")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("the page refused a script context"))
    }

    /// A script's value, run in a world of its own.
    async fn eval(&mut self, expression: &str) -> anyhow::Result<Value> {
        let context = self.world().await?;
        let r = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "contextId": context,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        if let Some(e) = r.get("exceptionDetails") {
            let text = e
                .pointer("/exception/description")
                .or_else(|| e.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("the script failed");
            anyhow::bail!("{text}");
        }
        Ok(r.pointer("/result/value").cloned().unwrap_or(Value::Null))
    }

    async fn where_am_i(&mut self) -> Value {
        self.eval("({ url: location.href, title: document.title })")
            .await
            .unwrap_or_else(|_| json!({}))
    }

    async fn navigate(&mut self, url: &str) -> anyhow::Result<Value> {
        let url = normalise(url)?;
        let r = self.call("Page.navigate", json!({ "url": url })).await?;
        if let Some(e) = r.get("errorText").and_then(Value::as_str) {
            // net::ERR_PROXY_CONNECTION_FAILED and friends: the proxy, not the
            // site, and the assistant should say so rather than retry.
            anyhow::bail!("could not open {url}: {e}");
        }
        self.settle(Duration::from_secs(20)).await;
        Ok(self.where_am_i().await)
    }

    /// Until the document says it has loaded, or the time is up. A page that
    /// keeps a connection open never reaches "complete", and is still usable.
    async fn settle(&mut self, within: Duration) {
        let start = Instant::now();
        // The old document can still answer "complete" for a moment.
        tokio::time::sleep(Duration::from_millis(300)).await;
        while start.elapsed() < within {
            if let Ok(Value::String(s)) = self.eval("document.readyState").await {
                if s == "complete" {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// `within`: only what is inside element N (a dialog, a panel), its text
    /// and its elements. `filter`: only elements whose label holds this text.
    /// Either way the numbers stay the page's own, so they work with click and
    /// type as they are. A web app's settings page lists fifty to seventy
    /// elements, and an assistant reads it again after every click.
    async fn read(&mut self, max_chars: usize, within: Option<usize>, filter: Option<&str>) -> anyhow::Result<Value> {
        let within = within.map_or("null".to_string(), |n| n.to_string());
        let filter = serde_json::to_string(&filter.map(str::to_lowercase))?;
        let script = format!(
            r#"(() => {{
  const els = {ELEMENTS};
  const within = {within};
  const filter = {filter};
  const scope = within === null ? null : els[within];
  if (within !== null && !scope) throw new Error('there is no element ' + within + ' on the page now; read_page again, the page may have changed');
  const inside = (e, c) => {{ for (let n = e.parentNode || e.host; n; n = n.parentNode || n.host) if (n === c) return true; return false; }};
  const label = e => (e.getAttribute('aria-label') || (e.labels && e.labels[0] && e.labels[0].innerText) || (e.tagName === 'SELECT' ? '' : e.innerText) || e.getAttribute('placeholder') || e.getAttribute('title') || e.getAttribute('name') || '').replace(/\s+/g, ' ').trim().slice(0, 100);
  const text = ((scope || document.body) ? (scope || document.body).innerText : '').replace(/\n{{3,}}/g, '\n\n');
  const picked = els.map((e, i) => [e, i]).filter(([e]) => !scope || inside(e, scope)).filter(([e]) => !filter || label(e).toLowerCase().includes(filter));
  const files = {FILE_INPUTS};
  const out = {{
    url: location.href,
    title: document.title,
    text: text.slice(0, {max_chars}),
    text_truncated: text.length > {max_chars},
    elements: picked.slice(0, {MAX_ELEMENTS}).map(([e, i]) => {{
      const o = {{ n: i, tag: e.tagName.toLowerCase(), label: label(e) }};
      if (e.type && e.tagName !== 'BUTTON') o.type = e.type;
      if (e.getAttribute('role')) o.role = e.getAttribute('role');
      if (e.href) o.href = e.href;
      if (e.disabled || e.getAttribute('aria-disabled') === 'true') o.disabled = true;
      const checked = e.getAttribute('aria-checked');
      if (checked !== null) o.checked = checked === 'mixed' ? 'mixed' : checked === 'true';
      else if (e.type === 'checkbox' || e.type === 'radio') o.checked = e.checked;
      const selected = e.getAttribute('aria-selected');
      if (selected !== null) o.selected = selected === 'true';
      const expanded = e.getAttribute('aria-expanded');
      if (expanded !== null) o.expanded = expanded === 'true';
      if (e.tagName === 'SELECT') o.value = (e.selectedOptions[0] && e.selectedOptions[0].text || '').slice(0, 100);
      else if (e.tagName === 'TEXTAREA' || (e.tagName === 'INPUT' && !['checkbox', 'radio', 'button', 'submit', 'reset', 'file', 'image', 'range', 'color'].includes(e.type))) {{
        // A password's characters never leave: what is returned goes to a
        // model provider. Whether it is filled is enough to act on.
        if (e.type === 'password') o.filled = e.value.length > 0;
        else if (e.value) o.value = e.value.slice(0, 100);
      }}
      return o;
    }}),
    elements_total: picked.length,
  }};
  if (files.length) out.file_inputs = files.map((e, i) => {{
    const o = {{ input: i, label: (e.getAttribute('aria-label') || (e.labels && e.labels[0] && e.labels[0].innerText) || e.getAttribute('name') || e.id || (e.getRootNode().host ? e.getRootNode().host.tagName.toLowerCase() : '')).trim().slice(0, 100) }};
    if (e.accept) o.accept = e.accept;
    if (e.multiple) o.multiple = true;
    return o;
  }});
  return out;
}})()"#
        );
        self.eval(&script).await
    }

    /// Where element N is, after bringing it into view.
    async fn locate(&mut self, n: usize) -> anyhow::Result<(f64, f64)> {
        let script = format!(
            r#"(() => {{
  const e = {ELEMENTS}[{n}];
  if (!e) return null;
  // Instant even where the page asks for smooth scrolling (YouTube Studio's
  // menu does): a smooth scroll has not moved yet when this returns, so the
  // point measured below would be where the element was, and the click lands
  // on whatever slides under it -- in the Studio, the menu item below.
  e.scrollIntoView({{ block: 'center', inline: 'center', behavior: 'instant' }});
  const r = e.getBoundingClientRect();
  return {{ x: r.left + r.width / 2, y: r.top + r.height / 2 }};
}})()"#
        );
        let at = self.eval(&script).await?;
        match (at.get("x").and_then(Value::as_f64), at.get("y").and_then(Value::as_f64)) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => anyhow::bail!(
                "there is no element {n} on the page now; read_page again, the page may have changed"
            ),
        }
    }

    async fn click_at(&mut self, x: f64, y: f64) -> anyhow::Result<()> {
        self.call("Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": x, "y": y }))
            .await?;
        for kind in ["mousePressed", "mouseReleased"] {
            self.call(
                "Input.dispatchMouseEvent",
                json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 }),
            )
            .await?;
        }
        Ok(())
    }

    /// A click at a point of the screenshot, for what no read lists: a
    /// control drawn on a canvas, a component with no role, a spot on a map.
    async fn click_point(&mut self, x: f64, y: f64) -> anyhow::Result<Value> {
        let (w, h) = self.viewport().await?;
        if x >= w || y >= h {
            anyhow::bail!("({x}, {y}) is outside the page, which is {w}x{h}: take a screenshot again");
        }
        self.click_at(x, y).await?;
        self.settle(Duration::from_secs(10)).await;
        Ok(self.where_am_i().await)
    }

    /// The visible part of the page, in CSS pixels, and where it sits on the
    /// document. From the browser rather than from the page's `innerWidth`,
    /// which is the page's to report.
    async fn viewport_at(&mut self) -> anyhow::Result<(f64, f64, f64, f64)> {
        let m = self.call("Page.getLayoutMetrics", json!({})).await?;
        let v = m
            .get("cssVisualViewport")
            .ok_or_else(|| anyhow::anyhow!("the browser did not say how large the page is"))?;
        let f = |k: &str| v.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        Ok((f("pageX"), f("pageY"), f("clientWidth"), f("clientHeight")))
    }

    async fn viewport(&mut self) -> anyhow::Result<(f64, f64)> {
        let (_, _, w, h) = self.viewport_at().await?;
        Ok((w, h))
    }

    async fn click(&mut self, n: usize) -> anyhow::Result<Value> {
        let (x, y) = self.locate(n).await?;
        self.click_at(x, y).await?;
        // A click that navigates needs the new page to exist before the next
        // read; one that does not costs a short wait.
        self.settle(Duration::from_secs(10)).await;
        Ok(self.where_am_i().await)
    }

    /// Text goes in at the caret, after what the field already holds, as a
    /// person typing would put it. `replace` selects what is there first, so
    /// the text takes its place, and with no text it empties the field. The
    /// selection is made by script on the field itself rather than with
    /// Cmd+A or Ctrl+A, which differ by platform and, on a page that does
    /// not hold the focus where expected, select the whole page instead.
    async fn type_text(&mut self, target: Option<usize>, text: &str, submit: bool, replace: bool) -> anyhow::Result<Value> {
        if let Some(n) = target {
            let (x, y) = self.locate(n).await?;
            self.click_at(x, y).await?;
            if !replace {
                // The click leaves the caret where it landed, in the middle of
                // a longer text, so "add to the field" would land wherever the
                // field's centre fell (the end-to-end test below caught it).
                // To the end, where a person adding to a field puts it.
                let to_end = format!(
                    r#"(() => {{
  const e = {FOCUSED};
  if (!e) return;
  if ((e.tagName === 'INPUT' || e.tagName === 'TEXTAREA') && typeof e.setSelectionRange === 'function') {{
    try {{ e.setSelectionRange(e.value.length, e.value.length); }} catch (_) {{}}
  }} else if (e.isContentEditable) {{
    const r = document.createRange(); r.selectNodeContents(e); r.collapse(false);
    const s = getSelection(); s.removeAllRanges(); s.addRange(r);
  }}
}})()"#
                );
                let _ = self.eval(&to_end).await;
            }
        }
        if replace {
            let script = format!(
                r#"(() => {{
  const e = {FOCUSED};
  if (!e || e === document.body) return 'nothing has the keyboard';
  if (typeof e.select === 'function' && (e.tagName === 'INPUT' || e.tagName === 'TEXTAREA')) {{ e.select(); return ''; }}
  if (e.isContentEditable) {{
    const r = document.createRange(); r.selectNodeContents(e);
    const s = getSelection(); s.removeAllRanges(); s.addRange(r); return '';
  }}
  return 'the focused element is not a text field';
}})()"#
            );
            if let Value::String(why) = self.eval(&script).await? {
                if !why.is_empty() {
                    anyhow::bail!("cannot replace: {why}. Pass the element number of the field");
                }
            }
            if text.is_empty() {
                self.key(&Chord::parse("Delete")?).await?;
            }
        }
        if !text.is_empty() {
            self.call("Input.insertText", json!({ "text": text })).await?;
        }
        if submit {
            self.key(&Chord::parse("Enter")?).await?;
            self.settle(Duration::from_secs(15)).await;
        }
        Ok(self.where_am_i().await)
    }

    /// One press and release, as the keyboard sends it.
    async fn key(&mut self, chord: &Chord) -> anyhow::Result<()> {
        let mut down = json!({
            "type": if chord.text.is_empty() { "rawKeyDown" } else { "keyDown" },
            "key": chord.key, "code": chord.code, "modifiers": chord.modifiers,
            "windowsVirtualKeyCode": chord.vk, "nativeVirtualKeyCode": chord.vk,
        });
        if !chord.text.is_empty() {
            down["text"] = json!(chord.text);
            down["unmodifiedText"] = json!(chord.text);
        }
        if !chord.commands.is_empty() {
            down["commands"] = json!(chord.commands);
        }
        self.call("Input.dispatchKeyEvent", down).await?;
        self.call(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp", "key": chord.key, "code": chord.code, "modifiers": chord.modifiers,
                "windowsVirtualKeyCode": chord.vk, "nativeVirtualKeyCode": chord.vk,
            }),
        )
        .await?;
        Ok(())
    }

    /// Keys go to whatever has the focus, the way a keyboard's do. A pause
    /// between repeats, because ten ArrowDowns in one frame is not a hand.
    async fn press(&mut self, chord: &Chord, times: usize) -> anyhow::Result<Value> {
        for i in 0..times {
            if i > 0 {
                tokio::time::sleep(Duration::from_millis(60)).await;
            }
            self.key(chord).await?;
        }
        if chord.key == "Enter" || chord.key == "Escape" || chord.key == "Tab" {
            self.settle(Duration::from_secs(10)).await;
        }
        let mut out = self.where_am_i().await;
        let focus = format!(
            r#"(() => {{ const e = {FOCUSED}; return e && e !== document.body ? (e.getAttribute('aria-label') || e.innerText || e.getAttribute('name') || e.tagName.toLowerCase()).replace(/\s+/g, ' ').trim().slice(0, 100) : null; }})()"#
        );
        out["focused"] = self.eval(&focus).await.unwrap_or(Value::Null);
        Ok(out)
    }

    /// A turn of the mouse wheel over element N, over a point of the
    /// screenshot, or over the middle of the page. The wheel scrolls whatever
    /// is under it, so a panel with its own scroll bar scrolls without the
    /// page moving (measured: 400 px down inside the panel, page at 0).
    /// `moved` says whether anything did, so the end of a list is visible.
    async fn scroll(&mut self, target: Option<usize>, at: Option<(f64, f64)>, dx: f64, dy: Option<f64>) -> anyhow::Result<Value> {
        let (w, h) = self.viewport().await?;
        let (x, y) = match (target, at) {
            (Some(n), _) => self.locate(n).await?,
            (None, Some(p)) => p,
            (None, None) => (w / 2.0, h / 2.0),
        };
        // Most of a screen, so the last lines of the old view are still in
        // the new one, as a person scrolls to keep their place.
        let dy = dy.unwrap_or(h * 0.8);
        let state = format!(
            r#"(() => {{
  let e = document.elementFromPoint({x}, {y});
  while (e && e.shadowRoot && e.shadowRoot.elementFromPoint({x}, {y}) && e.shadowRoot.elementFromPoint({x}, {y}) !== e) e = e.shadowRoot.elementFromPoint({x}, {y});
  let sum = scrollX * 7 + scrollY;
  for (let n = e; n; n = n.parentNode || n.host) if (n.nodeType === 1) sum += n.scrollLeft * 7 + n.scrollTop;
  return sum;
}})()"#
        );
        let before = self.eval(&state).await.unwrap_or(Value::Null);
        self.call("Input.dispatchMouseEvent", json!({ "type": "mouseMoved", "x": x, "y": y })).await?;
        self.call(
            "Input.dispatchMouseEvent",
            json!({ "type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy }),
        )
        .await?;
        // Smooth scrolling takes a few frames to arrive.
        tokio::time::sleep(Duration::from_millis(500)).await;
        let after = self.eval(&state).await.unwrap_or(Value::Null);
        let mut out = self.where_am_i().await;
        out["moved"] = json!(before != after);
        Ok(out)
    }

    /// Files into file input `input` (from read_page's `file_inputs`), as a
    /// person choosing them in the system's dialog would: the page gets the
    /// files and its change event, no dialog opens, and the size is not
    /// limited by anything here (150 MB in 4 ms, measured 28.11.2026). Only
    /// paths `upload_paths` allowed reach this.
    async fn upload(&mut self, input: Option<usize>, files: &[std::path::PathBuf]) -> anyhow::Result<Value> {
        let count = self.eval(&format!("{FILE_INPUTS}.length")).await?.as_u64().unwrap_or(0) as usize;
        let n = match (input, count) {
            (_, 0) => anyhow::bail!("this page has no file input. Open the upload dialog first, then try again"),
            (Some(n), c) if n >= c => anyhow::bail!("there is no file input {n}; read_page lists {c} under file_inputs"),
            (Some(n), _) => n,
            (None, 1) => 0,
            (None, c) => anyhow::bail!("this page has {c} file inputs; say which with input (read_page lists them under file_inputs)"),
        };
        let multiple = self.eval(&format!("{FILE_INPUTS}[{n}].multiple")).await?.as_bool().unwrap_or(false);
        if files.len() > 1 && !multiple {
            anyhow::bail!("file input {n} takes one file, not {}", files.len());
        }
        let object = self.eval_object(&format!("{FILE_INPUTS}[{n}]")).await?;
        let paths: Vec<String> = files.iter().map(|p| p.to_string_lossy().to_string()).collect();
        let r = self
            .call("DOM.setFileInputFiles", json!({ "files": paths, "objectId": object }))
            .await;
        let _ = self.call("Runtime.releaseObject", json!({ "objectId": object })).await;
        r?;
        let mut out = self.where_am_i().await;
        out["uploaded"] = json!(paths);
        Ok(out)
    }

    /// The picture is taken at the page's CSS size, whatever the screen's
    /// pixel density: on a Retina display Chrome would otherwise return twice
    /// the width and height, and a point read off the picture would land at
    /// twice the distance when clicked. `clip.scale` multiplies the device
    /// scale (measured on core 158: at scale factor 2 a 1000x613 page comes
    /// back 2000x1226 unclipped and 1000x613 with scale 0.5), and the clip is
    /// in document coordinates, so it starts where the page is scrolled to.
    /// A quarter of the pixels is also a quarter of what the model pays for.
    async fn screenshot(&mut self) -> anyhow::Result<Value> {
        // Bringing the tab forward first: a background tab paints nothing.
        // In a background start this switches the tab and nothing more: patch
        // 0305 keeps the window itself where it is (BrowserView::Activate).
        let _ = timed(&mut self.cdp, None, "Target.activateTarget", json!({ "targetId": self.target })).await;
        let (px, py, w, h) = self.viewport_at().await?;
        // The device scale, from the browser: a one-pixel clip comes back as
        // many pixels wide as the screen has per CSS pixel.
        let probe = self
            .call(
                "Page.captureScreenshot",
                json!({ "format": "png", "clip": { "x": px, "y": py, "width": 1, "height": 1, "scale": 1 } }),
            )
            .await
            .ok()
            .and_then(|r| r.get("data").and_then(Value::as_str).and_then(png_width));
        let scale = probe.map_or(1.0, |dpr| 1.0 / f64::from(dpr.max(1)));
        let r = self
            .call(
                "Page.captureScreenshot",
                json!({ "format": "jpeg", "quality": 70,
                        "clip": { "x": px, "y": py, "width": w, "height": h, "scale": scale } }),
            )
            .await?;
        let data = r
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("the browser returned no picture"))?;
        let mut out = self.where_am_i().await;
        out["width"] = json!(w.round());
        out["height"] = json!(h.round());
        out["image"] = json!({ "mime": "image/jpeg", "data": data });
        Ok(out)
    }

    /// A script's result as a handle rather than a value: an element, for a
    /// command that takes one.
    async fn eval_object(&mut self, expression: &str) -> anyhow::Result<String> {
        let context = self.world().await?;
        let r = self
            .call("Runtime.evaluate", json!({ "expression": expression, "contextId": context }))
            .await?;
        if r.get("exceptionDetails").is_some() {
            anyhow::bail!("the element could not be reached");
        }
        r.pointer("/result/objectId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("the element is gone"))
    }
}

/// Width of a base64 PNG, from its header: the IHDR chunk is always first,
/// width at byte 16.
fn png_width(b64: &str) -> Option<u32> {
    use base64::Engine;
    let head = base64::engine::general_purpose::STANDARD.decode(b64.get(..32)?).ok()?;
    if head.get(..8)? != b"\x89PNG\r\n\x1a\n" || head.get(12..16)? != b"IHDR" {
        return None;
    }
    Some(u32::from_be_bytes(head.get(16..20)?.try_into().ok()?))
}

/// A CDP call that gives up rather than hangs; see warm.rs for the call that
/// taught this.
async fn timed(cdp: &mut Cdp, session: Option<&str>, method: &str, params: Value) -> anyhow::Result<Value> {
    match tokio::time::timeout(Duration::from_secs(20), cdp.call_in(session, method, params)).await {
        Ok(r) => r,
        Err(_) => anyhow::bail!("{method} timed out"),
    }
}

#[cfg(test)]
mod tests {
    use super::{allowed_upload, normalise, png_width, point, Chord};
    use serde_json::json;

    #[test]
    fn addresses_are_taken_as_a_person_types_them() {
        assert_eq!(normalise("example.com").unwrap(), "https://example.com");
        assert_eq!(normalise(" http://a.b/c ").unwrap(), "http://a.b/c");
        assert_eq!(normalise("about:blank").unwrap(), "about:blank");
        assert_eq!(normalise("localhost:3000/x").unwrap(), "https://localhost:3000/x");
    }

    #[test]
    fn the_disk_and_the_browser_itself_are_not_sites() {
        assert!(normalise("file:///etc/passwd").is_err());
        assert!(normalise("chrome://settings").is_err());
        assert!(normalise("javascript:alert(1)").is_err());
        assert!(normalise("").is_err());
    }

    #[test]
    fn keys_are_named_as_a_keyboard_names_them() {
        let enter = Chord::parse("Enter").unwrap();
        assert_eq!((enter.key.as_str(), enter.vk, enter.text.as_str(), enter.modifiers), ("Enter", 13, "\r", 0));
        let back = Chord::parse("shift + tab").unwrap();
        assert_eq!((back.key.as_str(), back.modifiers, back.text.as_str()), ("Tab", 8, ""));
        let down = Chord::parse("Down").unwrap();
        assert_eq!(down.code, "ArrowDown");
        let a = Chord::parse("a").unwrap();
        assert_eq!((a.key.as_str(), a.code.as_str(), a.vk, a.text.as_str()), ("a", "KeyA", 65, "a"));
        let big = Chord::parse("Shift+a").unwrap();
        assert_eq!((big.key.as_str(), big.text.as_str()), ("A", "A"));
        let seven = Chord::parse("7").unwrap();
        assert_eq!((seven.code.as_str(), seven.vk), ("Digit7", 55));
    }

    #[test]
    fn a_shortcut_types_nothing_and_names_its_command_on_macos() {
        let all = Chord::parse("Cmd+A").unwrap();
        assert_eq!((all.modifiers, all.text.as_str()), (4, ""));
        assert_eq!(all.commands, if cfg!(target_os = "macos") { vec!["selectAll"] } else { vec![] });
        let redo = Chord::parse("Cmd+Shift+Z").unwrap();
        assert_eq!(redo.commands, if cfg!(target_os = "macos") { vec!["redo"] } else { vec![] });
        // Ctrl+A is not an editing shortcut on macOS; nothing to name there.
        assert!(Chord::parse("Ctrl+A").unwrap().commands.is_empty());
        assert_eq!(Chord::parse("Ctrl+A").unwrap().text, "");
    }

    #[test]
    fn what_is_not_a_key_says_so() {
        assert!(Chord::parse("").is_err());
        assert!(Chord::parse("Hyper+A").is_err());
        assert!(Chord::parse("F13").is_err());
        assert!(Chord::parse("@").is_err());
    }

    #[test]
    fn a_point_is_both_coordinates_or_none() {
        assert_eq!(point(&json!({ "x": 10, "y": 20.5 })).unwrap(), Some((10.0, 20.5)));
        assert_eq!(point(&json!({ "element": 3 })).unwrap(), None);
        assert!(point(&json!({ "x": 10 })).is_err());
        assert!(point(&json!({ "x": -1, "y": 5 })).is_err());
    }

    #[test]
    fn a_png_says_how_wide_it_is() {
        // 2x1 PNG header: what a one-pixel clip returns at device scale 2.
        let png = [
            0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 2, 0, 0, 0,
            2, 8, 2, 0, 0, 0,
        ];
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        assert_eq!(png_width(&b64), Some(2));
        assert_eq!(png_width("bm90IGEgcG5nIGF0IGFsbCwgbm90IGF0IGFsbA=="), None);
    }

    /// The bounds of an upload, on a scratch tree laid out like a home folder.
    #[test]
    fn uploads_reach_documents_and_nothing_else() {
        let root = std::env::temp_dir().join(format!("fury-upload-{}", std::process::id()));
        let home = root.join("home");
        let fury = home.join("FuryData");
        for d in ["Movies", ".ssh", "Projects/.git", "Library/Cookies", "AppData/Local", "FuryData"] {
            std::fs::create_dir_all(home.join(d)).unwrap();
        }
        for f in ["Movies/ep1.mp4", ".ssh/id_ed25519", "Projects/.git/config", "Library/Cookies/c.db",
                  "AppData/Local/x.db", "FuryData/fury.db", ".hidden-file.mp4"] {
            std::fs::write(home.join(f), b"x").unwrap();
        }
        std::fs::write(root.join("outside.txt"), b"x").unwrap();
        let home = home.canonicalize().unwrap();
        let fury = fury.canonicalize().unwrap();
        let ok = |p: &str| allowed_upload(&home.join(p).to_string_lossy(), &home, &fury);

        assert!(ok("Movies/ep1.mp4").is_ok());
        // A file whose own name starts with a dot is the person's, in a folder
        // they can see; only hidden folders are refused.
        assert!(ok(".hidden-file.mp4").is_ok());
        assert!(ok(".ssh/id_ed25519").is_err());
        assert!(ok("Projects/.git/config").is_err());
        assert!(ok("FuryData/fury.db").is_err());
        assert_eq!(ok("Library/Cookies/c.db").is_err(), cfg!(target_os = "macos"));
        assert_eq!(ok("AppData/Local/x.db").is_err(), cfg!(windows));
        assert!(ok("Movies").is_err(), "a folder is not a file");
        assert!(ok("Movies/missing.mp4").is_err());
        assert!(allowed_upload(&root.join("outside.txt").to_string_lossy(), &home, &fury).is_err());
        assert!(allowed_upload("Movies/ep1.mp4", &home, &fury).is_err(), "relative");
        // Through ".." and through a link: resolved before it is judged.
        assert!(ok("Movies/../.ssh/id_ed25519").is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(home.join(".ssh/id_ed25519"), home.join("Movies/key.mp4")).unwrap();
            assert!(ok("Movies/key.mp4").is_err());
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The page tools against a real browser, on a page built like a web app's
    /// settings: radio buttons and a file input inside shadow roots, a panel
    /// with its own scroll bar, a field that already holds text.
    ///
    ///     FURY_TEST_CORE=".../Fury.app/Contents/MacOS/Fury" \
    ///         cargo test -p fury-agent page_tools_drive_a_real_page -- --ignored
    ///
    /// It starts the browser at device scale 2, as on a Retina display, so the
    /// screenshot's size and the clicks at its points are measured where they
    /// used to disagree. It writes one file under the home folder for the
    /// upload (uploads reach nothing else) and removes it.
    #[tokio::test]
    #[ignore = "needs a browser: FURY_TEST_CORE"]
    async fn page_tools_drive_a_real_page() {
        use super::dispatch;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let core = std::env::var("FURY_TEST_CORE").expect("FURY_TEST_CORE: path to a Chromium binary");
        const PAGE: &str = r##"<!doctype html><meta charset=utf-8><title>settings</title>
<style>body{margin:0;font:16px sans-serif} #panel{height:120px;overflow:auto;border:1px solid} #spacer{height:600px}</style>
<input id=name aria-label="Nome" value="Ray Charlie">
<input id=pw type=password aria-label="Senha" value="hunter2">
<x-radio-group></x-radio-group>
<div id=panel><div id=spacer>top</div><button id=deep>Acessar recursos</button></div>
<div id=dialog role=dialog><button>Cancelar</button><button>Salvar</button></div>
<nav id=nav style="height:200px;overflow:auto;scroll-behavior:smooth;border:1px solid"><div style="height:150px">menu</div><div role=button style="height:40px" onclick="document.body.dataset.opened='settings'">Configurações</div><div role=button style="height:40px" onclick="document.body.dataset.opened='feedback'">Enviar feedback</div><div style="height:300px"></div></nav>
<x-uploader></x-uploader>
<script>
customElements.define('x-radio-group', class extends HTMLElement { connectedCallback() {
  const r = this.attachShadow({ mode: 'open' });
  r.innerHTML = ['Sim', 'Não', 'Revisar'].map((t, i) => `<x-r role=radio aria-checked=${i == 2} tabindex=0 style="display:block">${t}</x-r>`).join('');
  r.querySelectorAll('[role=radio]').forEach(e => e.onclick = () => { r.querySelectorAll('[role=radio]').forEach(o => o.setAttribute('aria-checked', 'false')); e.setAttribute('aria-checked', 'true'); });
}});
customElements.define('x-uploader', class extends HTMLElement { connectedCallback() {
  const r = this.attachShadow({ mode: 'open' });
  r.innerHTML = '<input type=file style="display:none" accept="video/*" aria-label="Vídeo"><span>arraste</span>';
  r.querySelector('input').onchange = e => { document.title = 'got ' + [...e.target.files].map(f => f.name + ':' + f.size).join(','); };
}});
document.addEventListener('keydown', e => { document.body.dataset.keys = (document.body.dataset.keys || '') + ' ' + e.key; });
</script>"##;

        // The page, from loopback, since only http(s) can be opened.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let body = PAGE.as_bytes();
                let head = format!("HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body).await;
            }
        });

        let dir = std::env::temp_dir().join(format!("fury-page-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut child = std::process::Command::new(&core)
            .arg(format!("--user-data-dir={}", dir.display()))
            // --use-mock-keychain: a scratch profile on macOS otherwise asks
            // the login keychain for "Fury Safe Storage", and every run of
            // this test puts a password prompt in front of whoever ran it.
            .args(["--remote-debugging-port=0", "--no-first-run", "--no-default-browser-check",
                   "--use-mock-keychain", "--password-store=basic",
                   "--force-device-scale-factor=2", "--window-size=1000,800", "about:blank"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let port_file = dir.join("DevToolsActivePort");
        let mut ws = None;
        for _ in 0..80 {
            if let Ok(t) = std::fs::read_to_string(&port_file) {
                let mut lines = t.lines();
                if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                    ws = Some(format!("ws://127.0.0.1:{port}{path}"));
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }
        let ws = ws.expect("the browser wrote no DevToolsActivePort");
        let home = super::home_dir().unwrap();
        let upload_dir = home.join(format!("fury-upload-test-{}", std::process::id()));
        std::fs::create_dir_all(&upload_dir).unwrap();
        let clip = upload_dir.join("clip.mp4");
        std::fs::write(&clip, vec![0u8; 20_000_000]).unwrap();

        let result = async {
            let go = |m: &'static str, p: serde_json::Value| {
                let ws = ws.clone();
                async move { dispatch(m, &ws, &p, false).await }
            };
            go("page.navigate", json!({ "url": url })).await?;

            let page = go("page.read", json!({})).await?;
            let els = page["elements"].as_array().unwrap().clone();
            let find = |label: &str| els.iter().find(|e| e["label"] == label).cloned().unwrap_or_else(|| panic!("{label} not listed: {els:?}"));
            // Inside a shadow root, with their state.
            assert_eq!(find("Revisar")["checked"], true);
            assert_eq!(find("Não")["checked"], false);
            assert_eq!(find("Nome")["value"], "Ray Charlie");
            // A password's characters never leave.
            assert_eq!(find("Senha")["filled"], true);
            assert!(!page.to_string().contains("hunter2"), "{page}");
            assert_eq!(page["file_inputs"][0]["accept"], "video/*");

            // within and filter keep the page's own numbers.
            let dialog = els.iter().position(|e| e["role"] == "dialog");
            assert!(dialog.is_none(), "a dialog is not something to act on");
            let salvar = find("Salvar")["n"].clone();
            let filtered = go("page.read", json!({ "filter": "salv" })).await?;
            assert_eq!(filtered["elements_total"], 1);
            assert_eq!(filtered["elements"][0]["n"], salvar);

            // The screenshot is at the page's size at device scale 2, and a
            // click at a point of it lands on what the picture shows there.
            let shot = go("page.screenshot", json!({})).await?;
            let width = shot["width"].as_f64().unwrap();
            assert!((990.0..=1000.0).contains(&width), "CSS width, not device pixels: {width}");
            let r = go("page.read", json!({ "filter": "Não" })).await?;
            let n = r["elements"][0]["n"].as_u64().unwrap();
            let at = go("page.click", json!({ "element": n })).await?; // brings it into view
            drop(at);
            let r = go("page.read", json!({ "filter": "Sim" })).await?;
            let sim = r["elements"][0]["n"].as_u64().unwrap() as usize;
            let mut pg = super::Page::attach(&ws).await?;
            let (x, y) = pg.locate(sim).await?;
            pg.detach().await;
            go("page.click", json!({ "x": x, "y": y })).await?;
            let r = go("page.read", json!({})).await?;
            let state = |l: &str| r["elements"].as_array().unwrap().iter().find(|e| e["label"] == l).unwrap()["checked"].clone();
            assert_eq!((state("Sim"), state("Não"), state("Revisar")), (json!(true), json!(false), json!(false)));
            assert!(go("page.click", json!({ "x": 5000, "y": 10 })).await.is_err(), "outside the picture");

            // replace, then clearing.
            let name = find("Nome")["n"].clone();
            go("page.type", json!({ "element": name, "text": "Not Again, Ray", "replace": true })).await?;
            let r = go("page.read", json!({ "filter": "Nome" })).await?;
            assert_eq!(r["elements"][0]["value"], "Not Again, Ray");
            go("page.type", json!({ "element": name, "text": " 2" })).await?;
            let r = go("page.read", json!({ "filter": "Nome" })).await?;
            assert!(r["elements"][0]["value"].as_str().unwrap().ends_with(" 2"), "appending stays the default");
            go("page.type", json!({ "element": name, "text": "", "replace": true })).await?;
            let r = go("page.read", json!({ "filter": "Nome" })).await?;
            assert!(r["elements"][0].get("value").is_none(), "cleared: {r}");

            // Keys: Cmd+A selects on macOS, Escape and Tab arrive.
            go("page.type", json!({ "element": name, "text": "abc" })).await?;
            go("page.press", json!({ "key": if cfg!(target_os = "macos") { "Cmd+A" } else { "Ctrl+A" } })).await?;
            let mut pg = super::Page::attach(&ws).await?;
            let sel = pg.eval("(() => { const e = document.getElementById('name'); return [e.selectionStart, e.selectionEnd]; })()").await?;
            pg.detach().await;
            assert_eq!(sel, json!([0, 3]), "select all");
            let r = go("page.press", json!({ "key": "Tab" })).await?;
            assert_eq!(r["focused"], "Senha");
            go("page.press", json!({ "key": "ArrowDown", "times": 3 })).await?;
            let mut pg = super::Page::attach(&ws).await?;
            let keys = pg.eval("document.body.dataset.keys").await?;
            pg.detach().await;
            assert!(keys.as_str().unwrap().ends_with("Tab ArrowDown ArrowDown ArrowDown"), "{keys}");

            // The wheel over the panel scrolls the panel, and says when it stops.
            let r = go("page.read", json!({ "filter": "Acessar" })).await?;
            let deep = r["elements"][0]["n"].clone();
            let mut pg = super::Page::attach(&ws).await?;
            let panel = pg.eval("(() => { const r = document.getElementById('panel').getBoundingClientRect(); return [r.left + r.width / 2, r.top + 10]; })()").await?;
            pg.detach().await;
            let (px, py) = (panel[0].as_f64().unwrap(), panel[1].as_f64().unwrap());
            go("page.scroll", json!({ "x": px, "y": py, "dy": -2000 })).await?;
            let r = go("page.scroll", json!({ "x": px, "y": py, "dy": 300 })).await?;
            assert_eq!(r["moved"], true);
            let r = go("page.scroll", json!({ "x": px, "y": py, "dy": 5000 })).await?;
            assert_eq!(r["moved"], true);
            let r = go("page.scroll", json!({ "x": px, "y": py, "dy": 5000 })).await?;
            assert_eq!(r["moved"], false, "at the end");
            let _ = deep;

            // A menu that scrolls smoothly, as YouTube Studio's does: the
            // click lands on the item that was named, not on the one that
            // slid under the pointer while the menu was still moving.
            let mut pg = super::Page::attach(&ws).await?;
            pg.eval("(() => { const n = document.getElementById('nav'); n.addEventListener('mousedown', () => { document.body.dataset.pressedAt = n.scrollTop; }, true); })()").await?;
            pg.detach().await;
            let r = go("page.read", json!({ "filter": "Configurações" })).await?;
            go("page.click", json!({ "element": r["elements"][0]["n"] })).await?;
            let mut pg = super::Page::attach(&ws).await?;
            let (opened, pressed_at, rests_at) = (
                pg.eval("document.body.dataset.opened || null").await?,
                pg.eval("Number(document.body.dataset.pressedAt)").await?,
                pg.eval("document.getElementById('nav').scrollTop").await?,
            );
            pg.detach().await;
            // Measured with the animation left on: the press arrived with the
            // menu 1.5px into a 70px move. Here that still hit the right item;
            // on a heavier page it is further along, and it was the next one.
            assert!(rests_at.as_f64().unwrap() > 0.0, "the menu had to scroll");
            assert_eq!(pressed_at, rests_at, "the press arrived while the menu was still moving");
            assert_eq!(opened, "settings");

            // A file into a hidden input inside a shadow root.
            let r = go("page.upload", json!({ "path": clip.to_string_lossy() })).await?;
            assert_eq!(r["title"], "got clip.mp4:20000000", "{r}");
            assert!(go("page.upload", json!({ "path": "/etc/hosts" })).await.is_err());
            assert!(go("page.upload", json!({ "path": clip.to_string_lossy(), "input": 3 })).await.is_err());
            anyhow::Ok(())
        }
        .await;

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&upload_dir);
        let _ = std::fs::remove_dir_all(&dir);
        result.unwrap();
    }
}
