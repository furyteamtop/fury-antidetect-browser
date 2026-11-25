# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov, devrahmanbd and the Fury authors
"""Human Interaction Dynamics with Viewport & Anti-Detect Enforcement.

Controls a CDP page session using realistic human physical dynamics:
- Bézier curve mouse trajectories with micro-jitters (tremor).
- Clamped strictly within the profile persona's visible viewport and screen bounds.
- Trusted CDP Input mouse events (never synthetic JavaScript el.click()).
- Cosine-eased wheel scrolling (Input.dispatchMouseEvent with mouseWheel).
- Automatic cookie consent banner detection and dismissal via trusted clicks.
"""

from __future__ import annotations

import json
import math
import random
import time
from typing import Any, Dict, Optional

from cdp import CDPClient


class HumanBrowser:
    """Controls a CDP page session with persona-enforced human dynamics.

    Automatically detects whether the active profile persona is a mobile device
    (via navigator.maxTouchPoints > 0 and UA sniff). When is_mobile is True:
    - Mouse hover and wheel events are suppressed (desktop events on a touch-primary
      persona are an immediate anti-bot tell per core/patches/0130-mobile-mode.patch).
    - Article navigation uses Page.navigate via CDP rather than click-on-coordinate,
      which is consistent with how touch-driven navigation reaches a URL.
    """

    def __init__(self, cdp: CDPClient, relay_port: Optional[int] = None):
        self.cdp = cdp
        self.relay_port = relay_port
        self.viewport_width = 1280
        self.viewport_height = 800
        self.dpr = 1.0
        self.screen_width = 1920
        self.screen_height = 1080
        self.mouse_x: Optional[int] = None
        self.mouse_y: Optional[int] = None
        # Detected once on first _refresh_viewport(); persona type never changes mid-session.
        self.is_mobile: Optional[bool] = None
        self._refresh_viewport()

    def _refresh_viewport(self) -> None:
        """Inspects the profile's genuine persona viewport, screen metrics, and input model."""
        try:
            metrics = self.cdp.eval("""(() => ({
                innerWidth: window.innerWidth,
                innerHeight: window.innerHeight,
                screenWidth: window.screen ? window.screen.width : null,
                screenHeight: window.screen ? window.screen.height : null,
                dpr: window.devicePixelRatio || 1,
                maxTouchPoints: navigator.maxTouchPoints || 0,
                ua: navigator.userAgent || ''
            }))()""")
            if metrics and isinstance(metrics, dict):
                w = int(metrics.get("innerWidth") or 0)
                h = int(metrics.get("innerHeight") or 0)
                if w > 0:
                    self.viewport_width = w
                if h > 0:
                    self.viewport_height = h
                sw = metrics.get("screenWidth")
                sh = metrics.get("screenHeight")
                if sw:
                    self.screen_width = int(sw)
                if sh:
                    self.screen_height = int(sh)
                self.dpr = float(metrics.get("dpr") or 1.0)

                # Detect mobile persona once. Fury's 0130-mobile-mode.patch sets maxTouchPoints
                # from the persona config; a touch-primary device must never receive desktop
                # mouseMoved/mouseWheel CDP events — that combination is an immediate bot tell.
                if self.is_mobile is None:
                    touch_points = int(metrics.get("maxTouchPoints") or 0)
                    ua = str(metrics.get("ua") or "")
                    ua_mobile = "Android" in ua or "iPhone" in ua or "iPad" in ua or "Mobile" in ua
                    self.is_mobile = touch_points > 0 or ua_mobile
                    if self.is_mobile:
                        now = time.strftime("%H:%M:%S")
                        print(f"[{now}]   ℹ Mobile persona detected (maxTouchPoints={touch_points}): "
                              f"desktop mouse events suppressed.", flush=True)

                if self.mouse_x is None:
                    self.mouse_x = random.randint(int(self.viewport_width * 0.35), int(self.viewport_width * 0.65))
                    self.mouse_y = random.randint(int(self.viewport_height * 0.35), int(self.viewport_height * 0.65))
                else:
                    self.mouse_x = self.clamp_x(self.mouse_x)
                    self.mouse_y = self.clamp_y(self.mouse_y)
        except Exception:
            pass

    def clamp_x(self, x: int | float) -> int:
        """Clamps X coordinate strictly within the profile's visible viewport."""
        return max(5, min(int(x), self.viewport_width - 5))

    def clamp_y(self, y: int | float) -> int:
        """Clamps Y coordinate strictly within the profile's visible viewport."""
        return max(5, min(int(y), self.viewport_height - 5))

    def navigate(self, url: str, wait_seconds: float = 3.0) -> None:
        """Navigates to URL, refreshes viewport metrics, and waits for page load."""
        now = time.strftime("%H:%M:%S")
        print(f"[{now}]   → Navigating to {url}", flush=True)
        self.cdp.call("Page.navigate", {"url": url})
        time.sleep(wait_seconds)
        self._refresh_viewport()

    def move_mouse(self, target_x: int, target_y: int, steps: int = 24) -> None:
        """Moves cursor along a smooth cubic Bézier curve with micro-jitter, strictly clamped.

        No-op on mobile personas: dispatching mouseMoved CDP events on a touch-primary
        profile (0130-mobile-mode.patch) exposes a desktop-on-mobile contradiction that
        anti-bot engines score as a definitive automation signal.
        """
        # Guard: suppress all desktop mouse events on mobile personas.
        if self.is_mobile:
            return

        target_x = self.clamp_x(target_x)
        target_y = self.clamp_y(target_y)

        if self.mouse_x is None or self.mouse_y is None:
            self.mouse_x = self.clamp_x(self.viewport_width // 2)
            self.mouse_y = self.clamp_y(self.viewport_height // 2)

        start_x, start_y = self.mouse_x, self.mouse_y
        dist = math.hypot(target_x - start_x, target_y - start_y)
        if dist < 5:
            steps = 5

        # Random control point offset for natural human curvature
        ctrl_x = (start_x + target_x) / 2 + random.uniform(-dist * 0.15, dist * 0.15)
        ctrl_y = (start_y + target_y) / 2 + random.uniform(-dist * 0.15, dist * 0.15)

        for i in range(1, steps + 1):
            t = i / steps
            # Quadratic Bézier: B(t) = (1-t)^2*P0 + 2(1-t)t*P1 + t^2*P2
            bx = (1 - t) ** 2 * start_x + 2 * (1 - t) * t * ctrl_x + (t ** 2) * target_x
            by = (1 - t) ** 2 * start_y + 2 * (1 - t) * t * ctrl_y + (t ** 2) * target_y

            # Add human micro-jitter (tremor)
            jx = self.clamp_x(bx + random.uniform(-1.0, 1.0))
            jy = self.clamp_y(by + random.uniform(-1.0, 1.0))

            try:
                self.cdp.send_cmd(
                    "Input.dispatchMouseEvent",
                    {"type": "mouseMoved", "x": jx, "y": jy},
                )
            except Exception:
                pass
            time.sleep(random.uniform(0.008, 0.022))

        self.mouse_x = target_x
        self.mouse_y = target_y
        try:
            self.cdp.send_cmd(
                "Input.dispatchMouseEvent",
                {"type": "mouseMoved", "x": target_x, "y": target_y},
            )
        except Exception:
            pass

    def click(self, x: int, y: int) -> None:
        """Moves cursor naturally to coordinates, pauses, and dispatches trusted CDP click.

        No-op on mobile personas: mouse click CDP events on a touch-primary
        profile produce an invalid desktop-on-mobile combination. Callers that
        need navigation on mobile must use Page.navigate directly.
        """
        if self.is_mobile:
            return
        tx = self.clamp_x(x)
        ty = self.clamp_y(y)
        self.move_mouse(tx, ty)
        time.sleep(random.uniform(0.08, 0.18))
        try:
            self.cdp.call(
                "Input.dispatchMouseEvent",
                {"type": "mousePressed", "button": "left", "clickCount": 1, "x": tx, "y": ty},
                timeout=5.0,
            )
            time.sleep(random.uniform(0.06, 0.14))
            self.cdp.call(
                "Input.dispatchMouseEvent",
                {"type": "mouseReleased", "button": "left", "clickCount": 1, "x": tx, "y": ty},
                timeout=5.0,
            )
        except Exception:
            pass

    def scroll_smooth(self, delta_y: int, duration_seconds: float = 1.2) -> None:
        """Dispatches natural wheel scroll events with cosine acceleration/deceleration.

        No-op on mobile personas: mouseWheel CDP events are desktop-only.
        On mobile profiles, human_browse_page still observes the dwell time
        so session duration stays realistic even without scroll events.
        """
        if self.is_mobile:
            # Dwell time is consumed by the caller (human_browse_page) separately.
            return
        steps = max(6, int(duration_seconds * 20))
        for i in range(steps):
            t = (i + 1) / steps
            # Smooth ease-in-out cosine curve
            factor = 0.5 * (1.0 - math.cos(math.pi * t))
            step_delta = int((delta_y / steps) * (0.8 + 0.4 * factor))
            try:
                self.cdp.send_cmd(
                    "Input.dispatchMouseEvent",
                    {
                        "type": "mouseWheel",
                        "x": self.mouse_x,
                        "y": self.mouse_y,
                        "deltaX": 0,
                        "deltaY": step_delta,
                    },
                )
            except Exception:
                pass
            time.sleep(duration_seconds / steps)


    def human_browse_page(self, dwell_seconds: float) -> None:
        """Simulates realistic human reading behavior: scrolls, pauses, small mouse moves."""
        start = time.time()
        while time.time() - start < dwell_seconds:
            remaining = dwell_seconds - (time.time() - start)
            if remaining <= 0:
                break

            # Random scroll down (or occasional scroll up to re-read)
            direction = 1 if random.random() > 0.15 else -1
            delta = random.randint(180, 420) * direction
            scroll_duration = min(remaining, random.uniform(0.8, 1.4))
            self.scroll_smooth(delta, duration_seconds=scroll_duration)

            remaining = dwell_seconds - (time.time() - start)
            if remaining <= 0:
                break

            # Natural dwell pause
            pause_time = min(remaining, random.uniform(1.2, 3.0))
            time.sleep(pause_time)

            remaining = dwell_seconds - (time.time() - start)
            if remaining <= 0:
                break

            # Small gaze cursor shift
            if random.random() > 0.35:
                jx = self.clamp_x(self.mouse_x + random.randint(-50, 50))
                jy = self.clamp_y(self.mouse_y + random.randint(-40, 40))
                self.move_mouse(jx, jy, steps=random.randint(6, 12))

    def dismiss_cookie_banners(self) -> bool:
        """Finds consent buttons and clicks them with trusted CDP mouse events (zero synthetic el.click())."""
        try:
            rect = self.cdp.eval("""(() => {
                const selectors = [
                    '#onetrust-accept-btn-handler',
                    'button#accept-cookie',
                    'button[id*="cookie-accept"]',
                    'button[id*="consent-accept"]',
                    'button[data-testid*="cookie-accept"]',
                    'button[data-testid*="agree"]',
                    '.didomi-components-button--color',
                    'button.cmp-agree'
                ];
                for (const s of selectors) {
                    const el = document.querySelector(s);
                    if (el && el.offsetParent !== null) {
                        const r = el.getBoundingClientRect();
                        if (r.width > 0 && r.height > 0) {
                            return {x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2)};
                        }
                    }
                }
                const buttons = Array.from(document.querySelectorAll('button, a'));
                for (const b of buttons) {
                    const t = (b.innerText || '').trim().toLowerCase();
                    if (['accept all', 'accept all cookies', 'i agree', 'agree & continue', 'allow all', 'got it'].includes(t)) {
                        if (b.offsetParent !== null) {
                            const r = b.getBoundingClientRect();
                            if (r.width > 0 && r.height > 0) {
                                return {x: Math.round(r.x + r.width / 2), y: Math.round(r.y + r.height / 2)};
                            }
                        }
                    }
                }
                return null;
            })()""")

            if rect and isinstance(rect, dict) and "x" in rect and "y" in rect:
                self.click(int(rect["x"]), int(rect["y"]))
                now = time.strftime("%H:%M:%S")
                print(f"[{now}]   ✓ Dismissed cookie consent banner via trusted CDP click", flush=True)
                time.sleep(1.0)
                return True
        except Exception:
            pass
        return False

    def find_and_read_article(self, min_dwell: float, max_dwell: float) -> bool:
        """Discovers headlines/articles on the page and navigates naturally."""
        try:
            links = self.cdp.eval(r"""(() => {
                const here = location.hostname;
                const anchors = Array.from(document.querySelectorAll('article a, h2 a, h3 a, [class*="headline"] a, [class*="title"] a, a[href]'));
                const results = [];
                for (const a of anchors) {
                    try {
                        const u = new URL(a.href);
                        const text = (a.innerText || "").trim();
                        if (
                            (u.hostname === here || u.hostname.endsWith("." + here)) &&
                            u.pathname !== "/" &&
                            u.pathname !== location.pathname &&
                            text.length >= 15 &&
                            !a.href.includes("#") &&
                            !/\.(pdf|zip|jpg|png|gif|mp4)$/i.test(u.pathname) &&
                            !/(privacy|terms|cookie|subscribe|login|sign-in)/i.test(u.pathname)
                        ) {
                            const rect = a.getBoundingClientRect();
                            if (rect.width > 0 && rect.height > 0 && rect.top > 0 && rect.top < window.innerHeight * 2.5) {
                                results.push({
                                    url: a.href,
                                    text: text.slice(0, 60),
                                    x: Math.round(rect.x + rect.width / 2),
                                    y: Math.round(rect.y + rect.height / 2)
                                });
                            }
                        }
                    } catch (e) {}
                }
                return results;
            })()""")
            if not links or not isinstance(links, list):
                return False

            chosen = random.choice(links[:6])
            now = time.strftime("%H:%M:%S")
            print(f"[{now}]   → Exploring article: \"{chosen['text']}...\"", flush=True)

            if self.is_mobile:
                # Mobile persona: no cursor exists. Navigate directly via Page.navigate so no
                # desktop mouse event is dispatched on a touch-primary profile.
                self.cdp.call("Page.navigate", {"url": chosen["url"]})
            else:
                self.click(chosen["x"], chosen["y"])
            time.sleep(random.uniform(2.5, 4.0))
            self._refresh_viewport()

            dwell = random.uniform(min_dwell, max_dwell)
            now = time.strftime("%H:%M:%S")
            print(f"[{now}]   📖 Reading article (dwell {dwell:.1f}s)...", flush=True)
            self.human_browse_page(dwell)
            return True
        except Exception:
            return False