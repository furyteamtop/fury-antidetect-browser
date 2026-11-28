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

  7. EXACT WHERE EVERY MACHINE IS EXACT. A canvas holding nothing but solid,
     opaque, pixel-aligned rects reads back byte-identical on every machine,
     so noise there hides nothing and is the one thing a detector can check
     exactly. fv.pro fills an 8x8 grid of random colours and reads each pixel
     back; pixelscan fills fourteen solid rects and compares two toDataURL()s.
     Both called us "not real" / "masking" for it on 27.09.2026 and neither
     says so of a real Chrome. Noise starts with the first draw a machine
     renders its own way (text, a path, an image, a blur) and ends at reset.

  9. IT LOOKS LIKE A RASTERISER. Noise moves coverage at antialiased edges
     and blends inside opaque content; it never paints the inside of a
     single-colour glyph another colour (real Chrome: 0 of 429 such pixels
     off-colour) and never moves a flat area. The first noise did both, and
     pixelscan said "Masking detected" until it stopped (29.09.2026).

  8. toBlob() AGREES. On the main thread toBlob() encodes progressively from
     the source pixels and never touched ImageDataBuffer, so before 27.09.2026
     it returned the canvas's REAL pixels beside a noised toDataURL() — the
     real rendering, and a contradiction to go with it.

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
  // Text first, so this canvas is one that gets noise at all (claim 7), then a
  // fill over all of it: every pixel's intended value is known, and a flat
  // fill must come back exact even on a noised canvas (claim 9).
  ox.font = '10px sans-serif'; ox.fillText('x', 2, 10);
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
    glyphs: (() => {
      const t = document.createElement('canvas'); t.width = 300; t.height = 60;
      const tx = t.getContext('2d'); tx.fillStyle = '#006699'; tx.font = '24px Arial';
      tx.fillText('Cwm fjordbank glyphs vext quiz', 4, 40);
      const d = tx.getImageData(0, 0, 300, 60).data;
      let opaqueOff = 0;
      for (let i = 0; i < d.length; i += 4)
        if (d[i+3] === 255 && !(d[i] === 0 && d[i+1] === 0x66 && d[i+2] === 0x99)) opaqueOff++;
      return {opaqueOff, hex: hex(d)};
    })(),
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
    x.font = '10px sans-serif'; x.fillText('x', 2, 10);
    x.fillStyle = '#f60'; x.fillRect(0, 0, 64, 64);
    const d = x.getImageData(0, 0, 64, 64).data;
    const t = new OffscreenCanvas(300, 60); const tx = t.getContext('2d');
    tx.fillStyle = '#006699'; tx.font = '24px sans-serif'; tx.fillText('Cwm fjordbank glyphs vext quiz', 4, 40);
    const g = tx.getImageData(0, 0, 300, 60).data;
    let h = 0; for (let i = 0; i < g.length; i++) h = (h * 31 + g[i]) >>> 0;
    postMessage(String(h));
  `;
  const w = new Worker(URL.createObjectURL(new Blob([src])));
  const out = await new Promise((r) => { w.onmessage = (e) => r(e.data); });
  w.terminate();
  return out;
})()
"""


# Claim 7: what fv.pro and pixelscan do, reduced to what they check.
EXACT = """
(async () => {
  const rnd = (i) => (i * 2654435761 >>> 0) % 256;
  // fv.pro: an 8x8 grid of 1x1 random colours, each read back alone.
  const g = document.createElement('canvas'); g.width = 8; g.height = 8;
  const gx = g.getContext('2d');
  const want = [];
  for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) {
    const c = [rnd(y * 8 + x), rnd(y * 8 + x + 64), rnd(y * 8 + x + 128)];
    want.push(c);
    gx.fillStyle = `rgba(${c[0]}, ${c[1]}, ${c[2]}, 255)`;
    gx.fillRect(x, y, 1, 1);
  }
  let gridWrong = 0;
  for (let y = 0; y < 8; y++) for (let x = 0; x < 8; x++) {
    const d = gx.getImageData(x, y, 1, 1).data, c = want[y * 8 + x];
    if (d[0] !== c[0] || d[1] !== c[1] || d[2] !== c[2] || d[3] !== 255) gridWrong++;
  }
  // pixelscan: solid rects, encoded.
  const r = document.createElement('canvas'); r.width = 140; r.height = 10;
  const rx = r.getContext('2d');
  const colours = ['#ff0000', '#00ff00', '#0000ff', '#ffff00', '#ff00ff', '#00ffff',
                   '#010101', '#fefefe', '#000000', '#333333', '#666666', '#999999',
                   '#cccccc', '#ffffff'];
  colours.forEach((c, i) => { rx.fillStyle = c; rx.fillRect(i * 10, 0, 10, 10); });
  // Reset: text makes a canvas noisy, and assigning its width wipes it.
  const t = document.createElement('canvas'); t.width = 32; t.height = 32;
  const tx = t.getContext('2d');
  tx.font = '20px sans-serif'; tx.fillText('W', 2, 24);
  t.width = 32;
  tx.fillStyle = '#336699'; tx.fillRect(0, 0, 32, 32);
  let resetWrong = 0;
  const td = tx.getImageData(0, 0, 32, 32).data;
  for (let i = 0; i < td.length; i += 4)
    if (td[i] !== 0x33 || td[i + 1] !== 0x66 || td[i + 2] !== 0x99) resetWrong++;
  return JSON.stringify({gridWrong, rects: r.toDataURL(), colours, resetWrong});
})()
"""

# Claim 8: toBlob() on the main thread, beside toDataURL() and getImageData().
BLOB = """
(async () => {
  %(draw)s
  const blob = await new Promise((res) => c.toBlob(res, 'image/png'));
  const buf = new Uint8Array(await blob.arrayBuffer());
  let bin = '';
  for (let i = 0; i < buf.length; i++) bin += String.fromCharCode(buf[i]);
  const hex = (a) => Array.from(a).map(v => v.toString(16).padStart(2, '0')).join('');
  return JSON.stringify({
    blob: 'data:image/png;base64,' + btoa(bin),
    whole: hex(x.getImageData(0, 0, 240, 120).data.slice(0, 4096)),
  });
})()
""" % {"draw": DRAW}


# Half-transparent content, which the scene above has none of, and where every
# claim above passed while BrowserScan said "Canvas Tampering" (issue 46,
# 26.11.2026). On core 157 the PNG paths got unpremultiplied pixels and clamped
# their colours to alpha, and the noise reached the flat background beside a
# glyph. The first 158 build fixed both and still moved every pixel of a
# half-transparent fill by one level (its own conversion from premultiplied
# rounded a tie the other way from Chrome's), and 20 noised pixels of a line of
# text read back changed from their own PNG.
#
# BrowserScan asks all of it: the same drawing through an OffscreenCanvas and an
# on-screen one, a canvas drawn from its own PNG read back against the original,
# and the pixels around one letter on a half-transparent fill.
ALPHA = """
(async () => {
  const W = 120, H = 60;
  const blobUrl = (b) => new Promise(r => { const f = new FileReader(); f.onload = () => r(f.result); f.readAsDataURL(b); });
  const img = (u) => new Promise(r => { const i = new Image(); i.onload = () => r(i); i.src = u; });
  const scenes = {
    text: (x) => {
      x.fillStyle = '#f60'; x.fillRect(50, 1, 31, 20);
      x.fillStyle = '#069'; x.font = '11pt serif'; x.fillText('Cwm fjord', 2, 15);
      x.fillStyle = 'rgba(102, 204, 0, 0.2)'; x.font = '18pt sans-serif'; x.fillText('Cwm', 4, 45);
    },
    veil: (x) => {
      x.fillStyle = 'rgba(102,102,102,0.4)'; x.fillRect(0, 0, W, H);
      x.font = '18px serif'; x.fillText('H', 40, 30);
    },
  };
  const hex = (d) => Array.from(d).map(v => v.toString(16).padStart(2, '0')).join('');
  const out = {};
  for (const [name, draw] of Object.entries(scenes)) {
    const c = document.createElement('canvas'); c.width = W; c.height = H;
    const x = c.getContext('2d'); draw(x);
    const url = c.toDataURL();
    const direct = x.getImageData(0, 0, W, H).data;
    const c2 = document.createElement('canvas'); c2.width = W; c2.height = H;
    const x2 = c2.getContext('2d'); x2.drawImage(await img(url), 0, 0);
    const back = x2.getImageData(0, 0, W, H).data;
    const o = new OffscreenCanvas(W, H); draw(o.getContext('2d'));
    out[name] = {
      url, direct: hex(direct),
      blob: await new Promise(r => c.toBlob(b => blobUrl(b).then(r))),
      offscreen: await blobUrl(await o.convertToBlob()),
      backWrong: direct.reduce((n, v, i) => n + (v !== back[i]), 0),
    };
  }
  return JSON.stringify(out);
})()
"""


def one_image(scene):
    """toBlob's and convertToBlob's pixels, compared decoded. toDataURL is not
    one of them: an unconfigured build already encodes a half-transparent
    pixel there one level off from toBlob (premultiplied 41 at alpha 102 is
    103 in toDataURL and getImageData, 102 in toBlob and convertToBlob, core
    158 on macOS, 26.11.2026), and the PNG bytes differ even where the pixels
    do not. What a page can compare is what real Chrome already keeps equal."""
    return decode_png(scene["blob"])[2] == decode_png(scene["offscreen"])[2]


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

        claims.check(a["flatMoved"] == 0,
                     f"a flat fill comes back exact even on a noised canvas "
                     f"({a['flatMoved']} of {a['flatTotal']} pixels moved)")
        claims.check(a["glyphs"]["opaqueOff"] == 0,
                     f"no opaque pixel of single-colour text is another colour "
                     f"({a['glyphs']['opaqueOff']} off)")

        exact = json.loads(s.js(EXACT))
        claims.check(exact["gridWrong"] == 0,
                     f"an 8x8 grid of 1x1 solid random colours reads back "
                     f"exactly, pixel by pixel — fv.pro's check "
                     f"({exact['gridWrong']} of 64 wrong)")
        rw, rh, rpx = decode_png(exact["rects"])
        rects_wrong = 0
        for i, colour in enumerate(exact["colours"]):
            want = bytes.fromhex(colour[1:])
            for y in range(rh):
                for x in range(i * 10, i * 10 + 10):
                    o = (y * rw + x) * 4
                    if rpx[o:o + 3] != want or rpx[o + 3] != 255:
                        rects_wrong += 1
        claims.check(rects_wrong == 0,
                     f"fourteen solid rects encode to exactly their colours — "
                     f"pixelscan's check ({rects_wrong} of {rw * rh} pixels wrong)")
        claims.check(exact["resetWrong"] == 0,
                     f"text makes a canvas noisy and assigning its width makes it "
                     f"exact again ({exact['resetWrong']} of 1024 wrong after the "
                     f"reset)")

        blob = json.loads(s.js(BLOB))
        _, _, blob_px = decode_png(blob["blob"])
        claims.check(blob_px[:4096].hex() == blob["whole"],
                     "toBlob() on the main thread matches getImageData byte for "
                     "byte — the progressive encoder is noised too")

        alpha_a = json.loads(s.js(ALPHA))
        for name, scene in alpha_a.items():
            _, _, px = decode_png(scene["url"])
            claims.check(px.hex() == scene["direct"],
                         f"{name}: half-transparent pixels are the same colour "
                         f"in the PNG as in getImageData")
            claims.check(one_image(scene),
                         f"{name}: toBlob and an OffscreenCanvas's "
                         f"convertToBlob encode one image")
            claims.check(scene["backWrong"] == 0,
                         f"{name}: drawn back from its own PNG it reads the "
                         f"same ({scene['backWrong']} bytes differ)")

    with launch(CORE, {"noise": {"canvasSeed": SEED_B}}) as s:
        b = json.loads(s.js(READ))
        print(f"  seed B canvas hash: {hashlib.sha256(b['whole'].encode()).hexdigest()[:16]}")
        claims.check(a["whole"] != b["whole"],
                     "a different seed gives different pixels — otherwise every "
                     "profile shares one hash, which fingerprints the product")

    with launch(CORE, None) as s:
        bare = json.loads(s.js(READ))
        bare_worker = s.js(WORKER)
        alpha_bare = json.loads(s.js(ALPHA))
        for name, scene in alpha_bare.items():
            _, _, px = decode_png(scene["url"])
            claims.control(px.hex() == scene["direct"] and scene["backWrong"] == 0
                           and one_image(scene),
                           f"{name}: an unconfigured build passes the same three")
        va = bytes.fromhex(alpha_a["veil"]["direct"])
        vb = bytes.fromhex(alpha_bare["veil"]["direct"])
        ground = vb[:4]
        veil_moved = sum(va[i:i + 4] != vb[i:i + 4] for i in range(0, len(va), 4)
                         if vb[i:i + 4] == ground)
        claims.check(veil_moved == 0,
                     f"veil: the background beside a glyph is where nothing was "
                     f"drawn, and is not touched ({veil_moved} pixels moved)")
        claims.check(alpha_a["text"]["direct"] != alpha_bare["text"]["direct"],
                     "text: and the glyphs on it still are")
        ga, gb = bytes.fromhex(a["glyphs"]["hex"]), bytes.fromhex(bare["glyphs"]["hex"])
        # By one step, measured where the step is taken. Coverage moves by one
        # and the colour must stay the fill colour, but getImageData hands back
        # UNpremultiplied pixels, and at alpha a one premultiplied unit is 255/a
        # levels there -- real Chrome's own edge pixels scatter by that much
        # around #006699. So a partly covered pixel may move by the rounding its
        # NEW coverage allows, 2 + 128/a', and not by more; opaque ones by 1. The
        # first model kept the premultiplied numbers and moved alpha-1 pixels by
        # 127 (30.09.2026), which "worst <= 1" over bytes caught and this still
        # does.
        moved = over = worst = 0
        for i in range(0, len(ga), 4):
            p, q = ga[i:i + 4], gb[i:i + 4]
            if p == q:
                continue
            moved += 1
            d_alpha = abs(p[3] - q[3])
            d_rgb = max(abs(x - y) for x, y in zip(p[:3], q[:3]))
            allowed = 1 if p[3] == 255 == q[3] else 2 + 128 // max(p[3], 1)
            worst = max(worst, d_rgb)
            if d_alpha > 1 or d_rgb > allowed:
                over += 1
        claims.check(0 < moved and over == 0,
                     f"text is noised, by one step at most: {moved} pixels moved, "
                     f"{over} by more than their coverage allows (largest colour "
                     f"move {worst})")
        claims.check(a_worker != bare_worker,
                     f"a Worker's OffscreenCanvas is noised too — same "
                     f"BaseRenderingContext2D, no document")
        bare_blob = json.loads(s.js(BLOB))
        _, _, bare_blob_px = decode_png(bare_blob["blob"])
        claims.check(bare_blob_px[:4096].hex() != blob["whole"],
                     "and it is not the unnoised canvas: an unconfigured build's "
                     "toBlob() differs from seed A's")
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
