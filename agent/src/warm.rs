// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Warming a profile: visiting sites so that it has been somewhere.
//!
//! A profile with a perfect fingerprint and an empty cookie jar is a machine
//! that was unboxed this morning and went straight to the login page. Real
//! browsers carry weeks of consent cookies, analytics ids and CDN tokens from
//! the sites their owners read, and antibot vendors score their absence. That
//! makes this a fingerprint vector rather than a convenience (docs/08 §2,
//! docs/16 5.2): Octo sells it as Cookie Robot, Linken Sphere with a search
//! engine and a depth, Dolphin as "realistic sessions".
//!
//! What this does, per profile: opens it, visits each URL in turn, dwells a
//! human-ish time, scrolls in uneven steps, and may follow one link on the same
//! site before moving on. Between pages it pauses. Everything the sites set
//! stays in the profile's own jar, which is the point.
//!
//! What it deliberately does not do: run headless (a headless run would leave
//! cookies set for a browser shaped differently from the one that will log in
//! later), click adverts, fill anything in, or search — a search engine query
//! is content, and content is the operator's. The URL list is theirs too; a
//! default list is offered, not imposed.
//!
//! Costs: the profile is launched with the debugging port, like a synchronised
//! group, and it stays open with the port until closed. `Runtime.enable` is
//! NOT called — link discovery uses a single `Runtime.evaluate`, which does not
//! need it, so the one measurable side-channel (docs/16, 3.5) is not opened.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;

use crate::cookies::Cdp;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub urls: Vec<String>,
    /// Seconds on each page, as a range; the actual dwell is drawn from it.
    #[serde(default = "default_dwell")]
    pub dwell_seconds: (u64, u64),
    /// Follow one same-site link on each page before moving on.
    #[serde(default)]
    pub follow_link: bool,
    /// Close the browser when the list is done.
    #[serde(default)]
    pub close_after: bool,
    /// The farming plan this run is a session of, if any. The run reports
    /// back to it when it ends. See farm.rs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub farm_id: Option<String>,
}

fn default_dwell() -> (u64, u64) {
    (8, 25)
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub profile_id: String,
    pub name: String,
    pub total: usize,
    pub done: usize,
    pub current: Option<String>,
    /// Cookies in the jar before the run and now.
    pub cookies_before: usize,
    pub cookies_now: usize,
    pub finished: bool,
    pub stopped: bool,
    pub error: Option<String>,
    pub started_at_ms: u64,
}

struct Job {
    progress: Progress,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
pub struct Warmer {
    jobs: Mutex<HashMap<String, Job>>,
}

impl Warmer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn status(&self) -> Vec<Progress> {
        let jobs = self.jobs.lock().await;
        let mut v: Vec<Progress> = jobs.values().map(|j| j.progress.clone()).collect();
        v.sort_by_key(|p| p.started_at_ms);
        v
    }

    pub async fn stop(&self, profile_id: &str) -> bool {
        let jobs = self.jobs.lock().await;
        match jobs.get(profile_id) {
            Some(j) => {
                j.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Forget finished jobs so the status list does not grow forever.
    pub async fn clear_finished(&self) {
        self.jobs.lock().await.retain(|_, j| !j.progress.finished);
    }

    /// Start warming one profile. `ws_endpoint` is the browser's debugging
    /// endpoint from the launch; `on_done` is called with the final progress
    /// when the run ends, so the caller can close the browser if the plan says
    /// so and a farming plan can count the session.
    pub async fn start(
        self: &Arc<Self>,
        profile_id: &str,
        name: &str,
        ws_endpoint: &str,
        plan: Plan,
        on_done: impl FnOnce(Progress) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + 'static,
    ) -> anyhow::Result<()> {
        if plan.urls.is_empty() {
            anyhow::bail!("nothing to visit: the list is empty");
        }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let mut jobs = self.jobs.lock().await;
            if jobs.get(profile_id).map(|j| !j.progress.finished).unwrap_or(false) {
                anyhow::bail!("{name} is already being warmed");
            }
            jobs.insert(
                profile_id.to_string(),
                Job {
                    progress: Progress {
                        profile_id: profile_id.to_string(),
                        name: name.to_string(),
                        total: plan.urls.len(),
                        done: 0,
                        current: None,
                        cookies_before: 0,
                        cookies_now: 0,
                        finished: false,
                        stopped: false,
                        error: None,
                        started_at_ms: now_ms(),
                    },
                    stop: Arc::clone(&stop),
                },
            );
        }

        let me = Arc::clone(self);
        let pid = profile_id.to_string();
        let ws = ws_endpoint.to_string();
        tokio::spawn(async move {
            let result = me.run(&pid, &ws, &plan, &stop).await;
            let last = {
                let mut jobs = me.jobs.lock().await;
                jobs.get_mut(&pid).map(|j| {
                    j.progress.finished = true;
                    j.progress.stopped = stop.load(std::sync::atomic::Ordering::Relaxed);
                    j.progress.current = None;
                    if let Err(e) = &result {
                        j.progress.error = Some(e.to_string());
                    }
                    j.progress.clone()
                })
            };
            if let Some(last) = last {
                on_done(last).await;
            }
        });
        Ok(())
    }

    async fn set<F: FnOnce(&mut Progress)>(&self, pid: &str, f: F) {
        if let Some(j) = self.jobs.lock().await.get_mut(pid) {
            f(&mut j.progress);
        }
    }

    async fn run(&self, pid: &str, ws: &str, plan: &Plan, stop: &std::sync::atomic::AtomicBool) -> anyhow::Result<()> {
        let mut cdp = Cdp::connect(ws).await?;

        let before = count_cookies(&mut cdp).await;
        self.set(pid, |p| {
            p.cookies_before = before;
            p.cookies_now = before;
        })
        .await;

        // The first page target: the one the launch opened. A new tab per site
        // would leave the operator forty tabs to close.
        let targets = timed(&mut cdp, None, "Target.getTargets", json!({})).await?;
        let target_id = targets
            .get("targetInfos")
            .and_then(|t| t.as_array())
            .and_then(|a| a.iter().find(|t| t.get("type").and_then(|v| v.as_str()) == Some("page")))
            .and_then(|t| t.get("targetId").and_then(|v| v.as_str()))
            .ok_or_else(|| anyhow::anyhow!("the browser has no page to drive"))?
            .to_string();
        let attached = timed(&mut cdp, None, "Target.attachToTarget", json!({ "targetId": target_id, "flatten": true })).await?;
        let sid = attached
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("attach gave no session"))?
            .to_string();

        for (i, url) in plan.urls.iter().enumerate() {
            if stop.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let url = normalise(url);
            self.set(pid, |p| p.current = Some(url.clone())).await;

            timed(&mut cdp, Some(&sid), "Page.navigate", json!({ "url": url })).await?;
            // Let it paint. Load events would be more exact and need
            // Page.enable; a pause that varies is closer to a person anyway.
            sleep(jitter(1800, 4200), stop).await;

            self.browse(&mut cdp, &sid, plan, stop).await;

            if plan.follow_link && !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if let Some(next) = same_site_link(&mut cdp, &sid).await {
                    self.set(pid, |p| p.current = Some(next.clone())).await;
                    let _ = timed(&mut cdp, Some(&sid), "Page.navigate", json!({ "url": next })).await;
                    sleep(jitter(1800, 4200), stop).await;
                    self.browse(&mut cdp, &sid, plan, stop).await;
                }
            }

            let now = count_cookies(&mut cdp).await;
            self.set(pid, |p| {
                p.done = i + 1;
                p.cookies_now = now;
            })
            .await;
            // Between sites: a person switches context, not tabs.
            sleep(jitter(1500, 6000), stop).await;
        }
        Ok(())
    }

    /// Read the page the way a person does: scroll down in uneven steps,
    /// pause, sometimes scroll back a little, for about the dwell time.
    async fn browse(&self, cdp: &mut Cdp, sid: &str, plan: &Plan, stop: &std::sync::atomic::AtomicBool) {
        let (lo, hi) = plan.dwell_seconds;
        let total = Duration::from_millis(jitter(lo.max(1) * 1000, hi.max(lo.max(1)) * 1000));
        let started = Instant::now();
        while started.elapsed() < total && !stop.load(std::sync::atomic::Ordering::Relaxed) {
            let dy: f64 = if jitter(0, 9) < 2 { -(jitter(80, 240) as f64) } else { jitter(180, 620) as f64 };
            if timed(cdp, Some(sid), "Input.dispatchMouseEvent", json!({
                    "type": "mouseWheel",
                    "x": jitter(200, 700), "y": jitter(200, 500),
                    "deltaX": 0, "deltaY": dy
                }))
                .await
                .is_err()
            {
                // The page is not taking input; reading it further is pointless.
                return;
            }
            sleep(jitter(700, 2600), stop).await;
        }
    }
}

/// One anchor on the same site, chosen at random, or nothing. A single
/// `Runtime.evaluate` — no `Runtime.enable`.
async fn same_site_link(cdp: &mut Cdp, sid: &str) -> Option<String> {
    let r = timed(cdp, Some(sid), "Runtime.evaluate", json!({
            "expression": r#"(() => {
                const here = location.host;
                const links = [...document.querySelectorAll('a[href]')]
                  .map(a => a.href)
                  .filter(h => { try {
                    const u = new URL(h);
                    // Same site, a real page: not this one, not an anchor on it
                    // (a bare-anchor href has an empty hash and the same path), not
                    // a file to download.
                    return u.host === here && u.protocol.startsWith('http')
                      && u.pathname + u.search !== location.pathname + location.search
                      && !h.endsWith('#') && !/\.(pdf|zip|dmg|exe|jpg|png|gif|mp4)$/i.test(u.pathname);
                  } catch (e) { return false; } });
                if (!links.length) return null;
                return links[Math.floor(Math.random() * links.length)];
            })()"#,
            "returnByValue": true
        }))
        .await
        .ok()?;
    r.pointer("/result/value").and_then(|v| v.as_str()).map(str::to_string)
}

/// A CDP call that gives up rather than hangs. `Input.dispatchMouseEvent` waits
/// for the renderer to acknowledge the event, and a renderer that is busy or
/// occluded can take its time; the first live run sat inside one such call
/// for minutes with Stop pressed. Ten seconds is generous for anything here,
/// and a timed-out call is logged by name so the next diagnosis is a grep.
async fn timed(cdp: &mut Cdp, session: Option<&str>, method: &str, params: serde_json::Value) -> anyhow::Result<serde_json::Value> {
    match tokio::time::timeout(Duration::from_secs(10), cdp.call_in(session, method, params)).await {
        Ok(r) => r,
        Err(_) => {
            tracing::warn!(method, "CDP call timed out during warm-up");
            anyhow::bail!("{method} timed out")
        }
    }
}

async fn count_cookies(cdp: &mut Cdp) -> usize {
    timed(cdp, None, "Storage.getCookies", json!({}))
        .await
        .ok()
        .and_then(|v| v.get("cookies").and_then(|c| c.as_array()).map(|a| a.len()))
        .unwrap_or(0)
}

fn normalise(url: &str) -> String {
    let u = url.trim();
    if u.contains("://") { u.to_string() } else { format!("https://{u}") }
}

fn jitter(lo: u64, hi: u64) -> u64 {
    use rand::Rng;
    if hi <= lo { return lo; }
    rand::thread_rng().gen_range(lo..=hi)
}

/// A sleep that ends early when asked to stop.
async fn sleep(ms: u64, stop: &std::sync::atomic::AtomicBool) {
    let step = Duration::from_millis(250);
    let mut left = Duration::from_millis(ms);
    while !left.is_zero() && !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let d = left.min(step);
        tokio::time::sleep(d).await;
        left -= d;
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The list offered when the operator has none: widely read, cookie-heavy,
/// harmless. A default, not a recommendation — the profile's own platform is
/// what its operator adds.
pub const DEFAULT_URLS: &[&str] = &[
    "https://www.wikipedia.org/",
    "https://www.youtube.com/",
    "https://www.reddit.com/",
    "https://www.bbc.com/",
    "https://www.amazon.com/",
    "https://www.imdb.com/",
    "https://weather.com/",
    "https://www.nytimes.com/",
    "https://stackoverflow.com/",
    "https://www.ebay.com/",
    "https://www.aliexpress.com/",
    "https://www.booking.com/",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_without_a_scheme_get_https() {
        assert_eq!(normalise("example.com"), "https://example.com");
        assert_eq!(normalise(" http://a.b/c "), "http://a.b/c");
    }

    #[test]
    fn a_plan_parses_with_defaults() {
        let p: Plan = serde_json::from_str(r#"{"urls":["a.com"]}"#).unwrap();
        assert_eq!(p.dwell_seconds, (8, 25));
        assert!(!p.follow_link);
        assert!(!p.close_after);
    }

    #[tokio::test]
    async fn an_empty_list_is_refused_before_anything_opens() {
        let w = Warmer::new();
        let r = w
            .start("p", "n", "ws://127.0.0.1:1/x", Plan { urls: vec![], dwell_seconds: (1, 1), follow_link: false, close_after: false, farm_id: None }, |_| Box::pin(async {}))
            .await;
        assert!(r.is_err());
        assert!(w.status().await.is_empty());
    }
}
