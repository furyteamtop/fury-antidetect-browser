#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Canvas noise: stable, seed-dependent, and self-consistent.

Noise is the easy half. Any build can return different pixels every call — and
that is worse than no noise at all, because a site that reads the same canvas
twice and gets two answers has found a browser that no real one behaves like.
What 0030 claims is harder, and each part is checkable:

  1. STABLE. The same canvas read twice gives the same bytes. The noise is a
     pure function of the seed and the pixel's absolute position, not of a
     counter or a clock.

  2. SEED-DEPENDENT. Two profiles with different seeds disagree. Otherwise
     every Fury profile shares one canvas hash, which is a fingerprint of the
     product.

  3. POSITION-CONSISTENT. `getImageData(50, 50, 10, 10)` returns exactly the
     pixels a whole-canvas read has at (50,50). This is what "absolute
     coordinates" in the patch buys, and a per-call or per-buffer offset would
     fail it while passing 1 and 2.

  4. ENCODED READBACK AGREES. `toDataURL()` goes through ImageDataBuffer and
     `getImageData` through BaseRenderingContext2D — two code paths. A site that
     decodes the PNG and compares against getImageData must see no
     contradiction. The PNG is decoded here rather than in the page, because
     drawing it back into a canvas would noise it a second time.

  5. SMALL. The pixels are perturbed, not replaced. A canvas whose colours have
     visibly moved is a broken renderer, not a quiet one.

  6. WORKERS TOO. BaseRenderingContext2D backs OffscreenCanvas as well, which
     the patch says is why it was changed there.

Usage: core/verify/verify-0030.py <core binary>
"""

import base64
import hashlib
import json
import os
import struct
import sys
import zlib

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

SEED_A = 0x5EED0001
SEED_B = 0x5EED0002

# Something with gradients, text and curves — the shapes a fingerprinting canvas
# actually draws, so the noise is measured over the same kind of pixels.
DRAW = """
const c = document.createElement('canvas');
c.width = 240; c.height = 120;
const x = c.getContext('2d');
const g = x.createLinearGradient(0, 0, 240, 120);
g.addColorStop(0, '#f60'); g.addColorStop(0.5, '#39f'); g.addColorStop(1, '#0c6');
x.fillStyle = g; x.fillRect(0, 0, 240, 120);
x.font = '18px sans-serif'; x.fillStyle = 'rgba(255,255,255,0.85)';
x.fillText('Fury \\u2014 canvas 0030', 8, 40);
x.beginPath(); x.arc(180, 80, 28, 0, Math.PI * 2);
x.fillStyle = 'rgba(0,0,0,0.35)'; x.fill();
"""

# Everything the page can answer without leaving it.
READ = """
(async () => {
  %(draw)s
  const whole = x.getImageData(0, 0, 240, 120);
  const again = x.getImageData(0, 0, 240, 120);
  const region = x.getImageData(50, 50, 10, 10);

  const hex = (a) => Array.from(a).map(v => v.toString(16).padStart(2, '0')).join('');
  const slice = (d, sx, sy, w, h) => {
    const out = [];
    for (let row = 0; row < h; row++)
      for (let col = 0; col < w * 4; col++)
        out.push(d.data[((sy + row) * 240 * 4) + (sx * 4) + col]);
    return out;
  };

  // Kept only for the log line. "Small" is measured on the flat fill below,
  // not here: an unconfigured build reads [253, 102, 1] at the gradient's first
  // stop, because a gradient is interpolated and colour-managed before anyone
  // adds noise. Measured, and it cost this script one wrong control.
  const corner = [whole.data[0], whole.data[1], whole.data[2], whole.data[3]];

  const off = new OffscreenCanvas(240, 120);
  const ox = off.getContext('2d');
  ox.fillStyle = '#f60'; ox.fillRect(0, 0, 240, 120);
  const offData = ox.getImageData(0, 0, 240, 120);
  // A flat fill: every pixel was asked to be exactly (255, 102, 0), so any
  // pixel that is not is one the noise moved, and how many were moved is
  // measurable rather than assumed.
  let moved = 0, worst = 0;
  for (let i = 0; i < offData.data.length; i += 4) {
    const d = Math.max(
      Math.abs(offData.data[i] - 255),
      Math.abs(offData.data[i + 1] - 102),
      Math.abs(offData.data[i + 2] - 0),
      Math.abs(offData.data[i + 3] - 255));
    if (d) { moved++; worst = Math.max(worst, d); }
  }

  return JSON.stringify({
    whole: hex(whole.data.slice(0, 4096)),
    again: hex(again.data.slice(0, 4096)),
    region: hex(region.data),
    regionFromWhole: hex(slice(whole, 50, 50, 10, 10)),
    corner,
    dataURL: c.toDataURL('image/png'),
    dataURLAgain: c.toDataURL('image/png'),
    flatMoved: moved, flatWorst: worst, flatTotal: offData.data.length / 4,
  });
})()
""" % {"draw": DRAW}

# The same, but inside a real Worker, where there is no document and only
# OffscreenCanvas exists.
WORKER = """
(async () => {
  const src = `
    const off = new OffscreenCanvas(64, 64);
    const x = off.getContext('2d');
    x.fillStyle = '#f60'; x.fillRect(0, 0, 64, 64);
    const d = x.getImageData(0, 0, 64, 64).data;
    postMessage(Array.from(d.slice(0, 16)).join(','));
  `;
  const w = new Worker(URL.createObjectURL(new Blob([src])));
  const out = await new Promise((r) => { w.onmessage = (e) => r(e.data); });
  w.terminate();
  return out;
})()
"""


def decode_png(data_url):
    """The pixels out of a canvas PNG: 8-bit RGBA, non-interlaced, always.

    Written out rather than imported so this script needs nothing that a CI
    runner might not have.
    """
    raw = base64.b64decode(data_url.split(",", 1)[1])
    assert raw[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos, idat, width, height = 8, b"", 0, 0
    while pos < len(raw):
        (length,) = struct.unpack(">I", raw[pos:pos + 4])
        kind = raw[pos + 4:pos + 8]
        body = raw[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            width, height, depth, colour = struct.unpack(">IIBB", body[:10])
            assert (depth, colour) == (8, 6), f"expected 8-bit RGBA, got {depth}/{colour}"
            assert body[12] == 0, "interlaced"
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
        pos += 12 + length

    stride = width * 4
    out = bytearray(stride * height)
    src = zlib.decompress(idat)
    prev = bytearray(stride)
    for y in range(height):
        f = src[y * (stride + 1)]
        line = bytearray(src[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - 4] if i >= 4 else 0
            b = prev[i]
            c = prev[i - 4] if i >= 4 else 0
            if f == 1:
                line[i] = (line[i] + a) & 0xFF
            elif f == 2:
                line[i] = (line[i] + b) & 0xFF
            elif f == 3:
                line[i] = (line[i] + ((a + b) >> 1)) & 0xFF
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 0xFF
        out[y * stride:(y + 1) * stride] = line
        prev = line
    return width, height, bytes(out)


def main():
    claims = Claims("0030 — canvas noise", CORE)

    with launch(CORE, {"noise": {"canvasSeed": SEED_A}}) as s:
        a = json.loads(s.js(READ))
        a_worker = s.js(WORKER)
        print(f"  seed A canvas hash: {hashlib.sha256(a['whole'].encode()).hexdigest()[:16]}")
        print(f"  seed A flat fill: {a['flatMoved']}/{a['flatTotal']} pixels "
              f"moved, worst {a['flatWorst']}")

        claims.check(a["whole"] == a["again"],
                     "reading the same canvas twice gives the same bytes — noise "
                     "that varies per call is worse than none")
        claims.check(a["dataURL"] == a["dataURLAgain"],
                     "and so does encoding it twice")

        claims.check(a["region"] == a["regionFromWhole"],
                     "getImageData(50,50,10,10) returns exactly the pixels a "
                     "whole-canvas read has at (50,50) — the noise is a function "
                     "of absolute position, not of the buffer it lands in")

        # The two code paths, compared outside the page.
        w, h, pixels = decode_png(a["dataURL"])
        from_png = pixels[:4096].hex()
        claims.check((w, h) == (240, 120),
                     f"the PNG decodes to the canvas's own size ({w}x{h})")
        claims.check(from_png == a["whole"],
                     "the encoded PNG and getImageData agree byte for byte — "
                     "ImageDataBuffer and BaseRenderingContext2D are two paths "
                     "and a site can compare them")

        # Small, measured on a flat #f60 fill where every pixel's intended
        # value is known exactly.
        share = a["flatMoved"] / a["flatTotal"]
        claims.check(a["flatWorst"] == 1,
                     f"no pixel of a flat fill moved by more than 1 "
                     f"(worst was {a['flatWorst']})")
        claims.check(0.05 < share < 0.25,
                     f"and {share:.1%} of them moved at all — enough to change "
                     f"every hash, little enough to be invisible "
                     f"({a['flatMoved']} of {a['flatTotal']})")

        worker_px = [int(v) for v in a_worker.split(",")[:4]]
        claims.check(worker_px != [255, 102, 0, 255],
                     f"a Worker's OffscreenCanvas is noised too — same "
                     f"BaseRenderingContext2D, no document (got {worker_px})")

    with launch(CORE, {"noise": {"canvasSeed": SEED_B}}) as s:
        b = json.loads(s.js(READ))
        print(f"  seed B canvas hash: {hashlib.sha256(b['whole'].encode()).hexdigest()[:16]}")
        claims.check(a["whole"] != b["whole"],
                     "a different seed gives different pixels — otherwise every "
                     "profile shares one hash, which fingerprints the product")

    with launch(CORE, None) as s:
        bare = json.loads(s.js(READ))
        _, _, bare_png = decode_png(bare["dataURL"])
        print(f"  unconfigured: {bare['flatMoved']} pixels moved, "
              f"PNG agrees: {bare_png[:4096].hex() == bare['whole']}")
        claims.control(
            bare["flatMoved"] == 0
            and bare["whole"] != a["whole"]
            and bare_png[:4096].hex() == bare["whole"],
            f"an unconfigured build moves no pixel of a flat fill, hashes "
            f"differently from both seeds, and has the two readback paths "
            f"already agreeing — which is the bar the noised build has to meet "
            f"(moved {bare['flatMoved']})")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
