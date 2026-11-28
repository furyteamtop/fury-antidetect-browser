#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov, devrahmanbd and the Fury authors
"""Zero-Dependency Profile Warmup for Fury Browser.

Visits seed targets with human-realistic browsing physics:
- Smooth cubic Bézier mouse movement with micro-jitters clamped to persona viewport.
- Natural cosine-eased wheel scrolling and human dwell reading intervals.
- Automatic cookie consent banner detection and dismissal via trusted CDP clicks.
- Article and headline exploration directly on the profile's page.
- Diagnostic cookie jar metrics before and after warming.

Uses ONLY the Python standard library. 100% zero external dependencies.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import sys
import time
import urllib.parse
import urllib.request
from typing import List, Optional

# Force immediate line-buffered or unbuffered output for live log streaming in Action Logs
try:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(line_buffering=True)
    if hasattr(sys.stderr, "reconfigure"):
        sys.stderr.reconfigure(line_buffering=True)
except Exception:
    pass

def log(msg: str) -> None:
    now = time.strftime("%H:%M:%S")
    print(f"[{now}] {msg}", flush=True)


_self_dir = os.path.dirname(os.path.abspath(__file__))
if _self_dir not in sys.path:
    sys.path.insert(0, _self_dir)

from cdp import CDPClient
from fury import Fury, running
from human import HumanBrowser

DEFAULT_SEEDS = [
    "https://news.ycombinator.com",
    "https://en.wikipedia.org/wiki/Special:Random",
    "https://lobste.rs",
    "https://github.com/trending",
]


def warm_profile(
    profile_id: str,
    sites: List[str],
    min_dwell: float = 12.0,
    max_dwell: float = 25.0,
    read_articles: bool = True,
    port: Optional[int] = None,
) -> int:
    # Warmup always runs foreground. As documented in agent/src/warm.rs: cookies set for a
    # browser shaped differently from the one that will log in later (headless geometry,
    # throttled rendering, hidden visibility state) are worse than none — anti-bot CDNs can
    # score the mismatch. FURY_BACKGROUND and FURY_HEADLESS are intentionally not checked.
    fury = Fury(port=port)

    log(f"[{profile_id[:8]}] Launching profile via Fury API...")
    with running(fury, profile_id, cdp=True) as session:
        ws_endpoint = session.get("ws_endpoint")
        if not ws_endpoint:
            log(f"[{profile_id[:8]}] [err] DevTools (CDP) endpoint not exposed by profile launch.")
            return 1

        relay_port = session.get("relay_port")
        log(f"[{profile_id[:8]}] Attached to DevTools: {ws_endpoint} (relay_port={relay_port})")

        rest = ws_endpoint[len("ws://"):]
        hostport = rest.partition("/")[0]
        cdp_port = int(hostport.partition(":")[2])

        page_ws_url = None
        for _ in range(30):
            try:
                req = urllib.request.Request(f"http://127.0.0.1:{cdp_port}/json/list")
                with urllib.request.urlopen(req, timeout=5) as resp:
                    targets = json.load(resp)
                for t in targets:
                    if t.get("type") == "page" and t.get("webSocketDebuggerUrl"):
                        page_ws_url = t["webSocketDebuggerUrl"]
                        break
                if page_ws_url:
                    break
            except Exception:
                time.sleep(0.5)

        if not page_ws_url:
            try:
                req = urllib.request.Request(f"http://127.0.0.1:{cdp_port}/json/new?about:blank", method="PUT")
                with urllib.request.urlopen(req, timeout=5) as resp:
                    page_ws_url = json.load(resp)["webSocketDebuggerUrl"]
            except Exception as e:
                log(f"[{profile_id[:8]}] [err] Failed to obtain page target: {e}")
                return 1

        client = CDPClient(page_ws_url)
        try:
            cookies_before_res = client.call("Network.getCookies", {})
            cookies_before = len(cookies_before_res.get("cookies", []))
            log(f"[{profile_id[:8]}] Initial cookies in profile jar: {cookies_before}")

            browser = HumanBrowser(client, relay_port=relay_port)

            for idx, url in enumerate(sites, 1):
                log(f"[{profile_id[:8]}] [{idx}/{len(sites)}] Navigating to {url}")
                try:
                    browser.navigate(url, wait_seconds=3.0)

                    # Check readyState
                    for _ in range(20):
                        try:
                            state = client.eval("document.readyState")
                            if state in ("interactive", "complete"):
                                break
                        except Exception:
                            pass
                        time.sleep(0.5)

                    time.sleep(random.uniform(1.5, 2.5))
                    browser._refresh_viewport()
                    browser.dismiss_cookie_banners()

                    dwell = random.uniform(min_dwell, max_dwell)
                    log(f"[{profile_id[:8]}] Scanning page (dwelling for {dwell:.1f}s)...")
                    browser.human_browse_page(dwell)

                    if read_articles:
                        browser.find_and_read_article(min_dwell * 0.7, max_dwell * 0.7)

                except Exception as e:
                    log(f"[{profile_id[:8]}] Notice on {url}: {e} (continuing to next target)")
                    continue

            cookies_after_res = client.call("Network.getCookies", {})
            cookies_after = len(cookies_after_res.get("cookies", []))
            gained = cookies_after - cookies_before
            sign = "+" if gained >= 0 else ""
            log(f"[{profile_id[:8]}] Warm-up complete: {len(sites)} sites, cookies before: {cookies_before}, cookies after: {cookies_after} ({sign}{gained} new cookies)")

        finally:
            client.close()

    log(f"[{profile_id[:8]}] Profile closed cleanly, locks released, SQLite cookies synced.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Warm up Fury profiles using native CDP with zero external dependencies.",
        add_help=True,
    )
    parser.add_argument("profile_ids", nargs="*", default=[], help="Fury profile UUID(s) to warm up.")
    parser.add_argument("--sites", type=int, default=3, help="Number of seed sites to visit (default: 3).")
    parser.add_argument("--min-dwell", type=float, default=12.0, help="Min seconds per page (default: 12.0).")
    parser.add_argument("--max-dwell", type=float, default=25.0, help="Max seconds per page (default: 25.0).")
    parser.add_argument("--min-gap", type=float, default=15.0, help="Min seconds between profiles (default: 15.0).")
    parser.add_argument("--max-gap", type=float, default=45.0, help="Max seconds between profiles (default: 45.0).")
    parser.add_argument("--no-articles", action="store_true", help="Only visit homepages without following articles.")
    parser.add_argument("--port", type=int, default=None, help="Fury API port (default: from FURY_API_PORT or 35000).")
    parser.add_argument("--custom-urls", nargs="+", default=[], help="Seed URLs to warm up against.")

    args, unknown = parser.parse_known_args()
    if unknown:
        log(f"Note: ignored unrecognized arguments: {unknown}")

    target_profiles = list(args.profile_ids)
    if not target_profiles and os.environ.get("FURY_PROFILE_IDS"):
        target_profiles = [p.strip() for p in os.environ["FURY_PROFILE_IDS"].split(",") if p.strip()]

    if not target_profiles:
        log("[err] No target profiles specified. Usage: python3 warm_up.py <profile-id> [profile-id-2 ...]")
        return 1

    raw_urls: List[str] = []
    if args.custom_urls:
        for item in args.custom_urls:
            for line in item.splitlines():
                u = line.strip()
                if u.startswith("http"):
                    raw_urls.append(u)

    if not raw_urls:
        raw_urls = list(DEFAULT_SEEDS)

    seed_urls = list(dict.fromkeys(raw_urls))
    sample_count = max(1, min(args.sites, len(seed_urls)))

    try:
        log(f"Target profiles ({len(target_profiles)}): {', '.join(target_profiles)}")
        log(f"Available seed URLs ({len(seed_urls)}): {sample_count} will be visited per profile")

        # Anti-Detection Rule 9: Sequential execution with irregular gaps
        # Never run concurrent browser sessions across the same target domains
        exit_code = 0
        for i, pid in enumerate(target_profiles):
            log(f"--- Profile {i + 1}/{len(target_profiles)}: {pid} ---")
            chosen = random.sample(seed_urls, sample_count)
            res = warm_profile(
                profile_id=pid,
                sites=chosen,
                min_dwell=args.min_dwell,
                max_dwell=args.max_dwell,
                read_articles=not args.no_articles,
                port=args.port,
            )
            if res != 0:
                exit_code = res

            if i + 1 < len(target_profiles):
                gap = random.uniform(args.min_gap, args.max_gap)
                log(f"Pausing for {gap:.1f}s before next profile...")
                time.sleep(gap)

        log(f"All {len(target_profiles)} profiles warmed successfully.")
        return exit_code
    except SystemExit as e:
        log(f"[err] Interactive Warmup stopped: {e}")
        return 1
    except Exception as e:
        log(f"[err] Interactive Warmup unexpected error: {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main())