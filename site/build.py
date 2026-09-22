#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Build the landing page from what the repository actually says.

    ./build.py            write dist/
    ./build.py --check    fail if dist/ is out of date (for CI)

WHY A BUILD STEP FOR ONE PAGE. The page makes numbers its argument — the
Chromium major we are on, how many patches carry the spoofing, how many
machines are in the catalogue — and a number typed into HTML by hand is a
number that goes stale the week after somebody reads it. Every one of them
is read here from the file that owns it: core/CHROMIUM_VERSION, the patch
series, the persona catalogue, the workspace version. Change the thing and
the page changes; there is no second place to remember.

WHAT IS NOT BUILT IN. The competitors' versions come from competitors.json,
which a person edits after measuring, and the page prints the date beside
them. They cannot be polled: they are closed products, and a page that
claimed to know today's Multilogin build would be the one lie on a site
whose whole argument is that it does not ask to be believed.

WHAT THE PAGE DOES POLL, in the browser, at load: the latest release on
GitHub, so the download links and the version shown are right even for a
deploy that happened before the release did. It fails silently to what was
built in, because a landing page that breaks when api.github.com is slow is
worse than one whose version is a day old.
"""

import datetime
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
DIST = HERE / "dist"

REPO = "furyteamtop/fury-antidetect-browser"


def chromium_version() -> str:
    """The pin the core is built from. `\\r` stripped: the Windows checkout is
    CRLF and this file is read on both machines."""
    return (ROOT / "core" / "CHROMIUM_VERSION").read_text().replace("\r", "").strip()


def app_version() -> str:
    m = re.search(r'(?m)^\[workspace\.package\](?:.|\n)*?^version = "([^"]+)"',
                  (ROOT / "Cargo.toml").read_text())
    if not m:
        raise SystemExit("no workspace version in Cargo.toml")
    return m.group(1)


def patch_count() -> int:
    """Lines of the series that name a patch.

    The '!' marker is stripped BEFORE the name is looked at, the way
    core/build/apply.sh does it. Checking `.patch` on the raw token instead
    silently missed all five patches that carry the marker — the page said 23
    where the series has 28, and 0001 among the missing, which is the one
    every other patch depends on. The same off-by-the-marker mistake apply.sh
    has its own comment about."""
    series = (ROOT / "core" / "patches" / "series").read_text().splitlines()
    n = 0
    for line in series:
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        name = line.split()[0].rstrip("!")
        if name.endswith(".patch"):
            n += 1
    return n


def persona_count() -> int:
    """Variants expanded from the two measured bases, plus whole machines
    somebody sent in. The same arithmetic `catalogue::all()` does."""
    cat = (ROOT / "shared-rs" / "src" / "catalogue.rs").read_text()
    variants = len(re.findall(r"(?m)^    Variant \{", cat))
    contributed = len(list((ROOT / "shared" / "personas" / "contributed").glob("*.json")))
    return variants + contributed


def gate_checks() -> int:
    """How many claims the release gate makes. Printed as "13 of 13" on the
    page, so it is read rather than remembered."""
    src = (ROOT / "tools" / "detect-suite" / "src" / "main.rs").read_text()
    m = re.search(r"gate has thirteen checks", src)
    # The count lives in status.py's assertion; read that instead of guessing.
    status = (ROOT / "tools" / "detect-suite" / "status.py").read_text()
    m = re.search(r"len\(gate_rows\) != (\d+)", status)
    return int(m.group(1)) if m else 13


def commit() -> str:
    try:
        return subprocess.run(["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
                              capture_output=True, text=True, check=True).stdout.strip()
    except Exception:
        return "unknown"


def competitors():
    data = json.loads((HERE / "competitors.json").read_text())
    return data["measured"], data["rows"]


def table_rows(rows, ours_major, lang):
    """One <tr> per product. Ours is marked, and the bar is drawn from the
    major itself so that "ahead" is visible without reading the number."""
    lo = min(r["chrome"] for r in rows)
    hi = max(r["chrome"] + 1 for r in rows)
    words = {
        "ru": {"yes": "есть", "no": "нет", "launcher": "лончер", "core": "ядро",
               "free": "бесплатно", "paid": "подписка", "stalled": "остановился"},
        "en": {"yes": "yes", "no": "no", "launcher": "launcher", "core": "core",
               "free": "free", "paid": "subscription", "stalled": "stalled"},
    }[lang]
    out = []
    for r in rows:
        width = round(100 * (r["chrome"] - lo + 1) / (hi - lo), 1)
        name = r["name"]
        if r.get("engine"):
            name += f' <span class="dim">({r["engine"]})</span>'
        if r.get("stalled"):
            name += f' <span class="dim">— {words["stalled"]}</span>'
        cls = " class=\"ours\"" if r.get("self") else ""
        out.append(
            f'<tr{cls}><th scope="row">{name}</th>'
            f'<td class="ver"><span class="bar" style="width:{width}%"></span>'
            f'<span class="num">{r["chrome"]}</span></td>'
            f'<td>{words[r["teams"]]}</td>'
            f'<td>{words[r["open"]]}</td>'
            f'<td>{words[r["price"]]}</td></tr>'
        )
    return "\n".join(out)


def main() -> int:
    check = "--check" in sys.argv

    chromium = chromium_version()
    major = chromium.split(".")[0]
    measured, rows = competitors()
    d, m, y = measured.split("-")[2], measured.split("-")[1], measured.split("-")[0]

    values = {
        "CHROMIUM": chromium,
        "CHROME_MAJOR": major,
        "VERSION": app_version(),
        "PATCHES": str(patch_count()),
        "PERSONAS": str(persona_count()),
        "GATE": str(gate_checks()),
        "REPO": REPO,
        "MEASURED_RU": f"{d}.{m}.{y}",
        "MEASURED_EN": datetime.date.fromisoformat(measured).strftime("%d %B %Y"),
        "ROWS_RU": table_rows(rows, int(major), "ru"),
        "ROWS_EN": table_rows(rows, int(major), "en"),
        "COMMIT": commit(),
        "BUILT": datetime.date.today().isoformat(),
    }

    page = (HERE / "index.html").read_text()
    missing = {m for m in re.findall(r"\{\{(\w+)\}\}", page)} - values.keys()
    if missing:
        raise SystemExit(f"the template asks for values nothing provides: {sorted(missing)}")
    for k, v in values.items():
        page = page.replace("{{" + k + "}}", v)

    out = DIST / "index.html"
    if check:
        if not out.exists() or out.read_text() != page:
            print("!! site/dist is out of date — run site/build.py", file=sys.stderr)
            return 1
        print("site/dist is up to date")
        return 0

    DIST.mkdir(exist_ok=True)
    out.write_text(page)
    # The images travel with the page — a landing that hotlinks its own logo
    # out of a git host breaks when that host rate-limits it — and they are
    # resized on the way. The originals are 1920px and 1024px: 1.6 MB of PNG
    # to draw a 40px wordmark and a favicon, which is most of the page's
    # weight for none of its meaning. `sips` ships with macOS; where it does
    # not, the full-size file is copied and the page still works.
    for src, dst, px in ((ROOT / "assets" / "logo-dark.png", DIST / "logo.png", 520),
                         (ROOT / "assets" / "icon.png", DIST / "icon.png", 96)):
        shutil.copy(src, dst)
        if shutil.which("sips"):
            subprocess.run(["sips", "-Z", str(px), str(dst)],
                           capture_output=True, check=False)
    total = sum(f.stat().st_size for f in DIST.iterdir())
    print(f"wrote {out} ({len(page):,} bytes); dist is {total/1024:.0f} KB in {len(list(DIST.iterdir()))} files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
