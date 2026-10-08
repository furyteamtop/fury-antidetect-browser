// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Driving the page of an open profile: go to an address, read what is there,
//! click, type, take a picture.
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
const ELEMENTS: &str = r#"(() => {
  const sel = 'a[href], button, input:not([type=hidden]), textarea, select, summary, [role=button], [role=link], [role=tab], [role=menuitem], [role=checkbox], [contenteditable=""], [contenteditable=true]';
  return Array.from(document.querySelectorAll(sel)).filter(e => {
    const r = e.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return false;
    const s = getComputedStyle(e);
    return s.visibility !== 'hidden' && s.display !== 'none';
  });
})()"#;

pub async fn dispatch(method: &str, ws: &str, params: &Value) -> anyhow::Result<Value> {
    let mut page = Page::attach(ws).await?;
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
            page.read(max).await
        }
        "page.click" => page.click(element(params)?).await,
        "page.type" => {
            let text = params.get("text").and_then(Value::as_str).unwrap_or_default();
            let target = params.get("element").and_then(Value::as_u64).map(|n| n as usize);
            let submit = params.get("submit").and_then(Value::as_bool).unwrap_or(false);
            page.type_text(target, text, submit).await
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

    /// A script's value, run in a world of its own. A fresh world per call:
    /// one made earlier dies with the document it was made for.
    async fn eval(&mut self, expression: &str) -> anyhow::Result<Value> {
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
        let context = world
            .get("executionContextId")
            .and_then(Value::as_i64)
            .ok_or_else(|| anyhow::anyhow!("the page refused a script context"))?;
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

    async fn read(&mut self, max_chars: usize) -> anyhow::Result<Value> {
        let script = format!(
            r#"(() => {{
  const els = {ELEMENTS};
  const label = e => (e.getAttribute('aria-label') || e.innerText || e.value || e.getAttribute('placeholder') || e.getAttribute('title') || e.getAttribute('name') || '').replace(/\s+/g, ' ').trim().slice(0, 100);
  const text = (document.body ? document.body.innerText : '').replace(/\n{{3,}}/g, '\n\n');
  return {{
    url: location.href,
    title: document.title,
    text: text.slice(0, {max_chars}),
    text_truncated: text.length > {max_chars},
    elements: els.slice(0, {MAX_ELEMENTS}).map((e, i) => {{
      const o = {{ n: i, tag: e.tagName.toLowerCase(), label: label(e) }};
      if (e.type && e.tagName !== 'BUTTON') o.type = e.type;
      if (e.getAttribute('role')) o.role = e.getAttribute('role');
      if (e.href) o.href = e.href;
      if (e.disabled) o.disabled = true;
      return o;
    }}),
    elements_total: els.length,
  }};
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
  e.scrollIntoView({{ block: 'center', inline: 'center' }});
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

    async fn click(&mut self, n: usize) -> anyhow::Result<Value> {
        let (x, y) = self.locate(n).await?;
        self.click_at(x, y).await?;
        // A click that navigates needs the new page to exist before the next
        // read; one that does not costs a short wait.
        self.settle(Duration::from_secs(10)).await;
        Ok(self.where_am_i().await)
    }

    async fn type_text(&mut self, target: Option<usize>, text: &str, submit: bool) -> anyhow::Result<Value> {
        if let Some(n) = target {
            let (x, y) = self.locate(n).await?;
            self.click_at(x, y).await?;
        }
        if !text.is_empty() {
            self.call("Input.insertText", json!({ "text": text })).await?;
        }
        if submit {
            for kind in ["keyDown", "keyUp"] {
                self.call(
                    "Input.dispatchKeyEvent",
                    json!({
                        "type": kind, "key": "Enter", "code": "Enter",
                        "windowsVirtualKeyCode": 13, "nativeVirtualKeyCode": 13,
                        "text": if kind == "keyDown" { "\r" } else { "" },
                    }),
                )
                .await?;
            }
            self.settle(Duration::from_secs(15)).await;
        }
        Ok(self.where_am_i().await)
    }

    async fn screenshot(&mut self) -> anyhow::Result<Value> {
        // Bringing the tab forward first: a background tab paints nothing.
        let _ = timed(&mut self.cdp, None, "Target.activateTarget", json!({ "targetId": self.target })).await;
        let r = self
            .call("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 70 }))
            .await?;
        let data = r
            .get("data")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("the browser returned no picture"))?;
        let mut out = self.where_am_i().await;
        out["image"] = json!({ "mime": "image/jpeg", "data": data });
        Ok(out)
    }
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
    use super::normalise;

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
}
