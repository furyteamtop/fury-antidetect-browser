#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""The WebGL card: its name, its limits, its extensions, and its pixels.

`UNMASKED_RENDERER_WEBGL` is what everyone reads and the least of it. A GPU is
described by three things a page can cross-check:

  * the STRINGS — vendor, renderer, version, shading language version.
  * the LIMITS — max texture size, max viewport dims, uniform vector counts.
    These are the card's real capabilities, they differ between an Apple M-series
    and an NVIDIA discrete part, and a profile claiming an NVIDIA renderer while
    reporting this Mac's limits has contradicted itself in the same call.
  * the EXTENSION LIST, which differs by driver.

And past all three, the rendered PIXELS: `readPixels` on a shaded triangle is a
fingerprint no string spoof touches. 0031 noises it through the same function
as canvas, so the same two rules apply — stable across reads, moving with the
seed.

The last claim is about the strings the config did NOT set: those must still be
the truth, because a getParameter that answers for everything is a different
kind of tell.

Usage: core/verify/verify-0031.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

# An NVIDIA discrete card on Windows, with the limits that go with it. Nothing
# here is what this Mac reports.
PARAMS = {
    "VENDOR": "Google Inc. (NVIDIA)",
    "RENDERER": "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)",
    "UNMASKED_VENDOR_WEBGL": "Google Inc. (NVIDIA)",
    "UNMASKED_RENDERER_WEBGL": "ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0, D3D11)",
    "MAX_TEXTURE_SIZE": 16384,
    "MAX_CUBE_MAP_TEXTURE_SIZE": 16384,
    "MAX_RENDERBUFFER_SIZE": 16384,
    "MAX_VERTEX_ATTRIBS": 16,
    "MAX_VERTEX_UNIFORM_VECTORS": 4096,
    "MAX_FRAGMENT_UNIFORM_VECTORS": 1024,
    # Comma-joined, not a JSON array: this is how the catalogue stores it
    # (tools/detect-suite/baselines) and how the patch parses it. Passed as an
    # array the key is simply not found and the real driver answers, which is
    # how this script first read 16384 back.
    "MAX_VIEWPORT_DIMS": "32767,32767",
}

EXTENSIONS = [
    "ANGLE_instanced_arrays", "EXT_blend_minmax", "EXT_texture_filter_anisotropic",
    "OES_element_index_uint", "OES_standard_derivatives", "OES_texture_float",
    "WEBGL_debug_renderer_info", "WEBGL_lose_context",
]

PROBE = """
(() => {
  const c = document.createElement('canvas');
  c.width = 64; c.height = 64;
  const gl = c.getContext('webgl');
  if (!gl) return JSON.stringify({error: 'no webgl'});

  const dbg = gl.getExtension('WEBGL_debug_renderer_info');
  const P = (n) => gl.getParameter(gl[n]);

  // A shaded triangle: the pixels are the fingerprint no string spoof reaches.
  const vs = gl.createShader(gl.VERTEX_SHADER);
  gl.shaderSource(vs, 'attribute vec2 p;varying vec2 v;void main(){v=p;gl_Position=vec4(p,0.,1.);}');
  gl.compileShader(vs);
  const fs = gl.createShader(gl.FRAGMENT_SHADER);
  gl.shaderSource(fs, 'precision highp float;varying vec2 v;void main(){gl_FragColor=vec4(abs(sin(v.x*7.)),abs(cos(v.y*11.)),v.x*v.y+0.5,1.);}');
  gl.compileShader(fs);
  const pr = gl.createProgram();
  gl.attachShader(pr, vs); gl.attachShader(pr, fs); gl.linkProgram(pr); gl.useProgram(pr);
  const buf = gl.createBuffer();
  gl.bindBuffer(gl.ARRAY_BUFFER, buf);
  gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1,-1, 3,-1, -1,3]), gl.STATIC_DRAW);
  const loc = gl.getAttribLocation(pr, 'p');
  gl.enableVertexAttribArray(loc);
  gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
  gl.drawArrays(gl.TRIANGLES, 0, 3);

  const read = () => {
    const px = new Uint8Array(64 * 64 * 4);
    gl.readPixels(0, 0, 64, 64, gl.RGBA, gl.UNSIGNED_BYTE, px);
    return Array.from(px.slice(0, 256)).join(',');
  };

  return JSON.stringify({
    vendor: P('VENDOR'),
    renderer: P('RENDERER'),
    version: P('VERSION'),
    slVersion: P('SHADING_LANGUAGE_VERSION'),
    unmaskedVendor: dbg ? gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) : null,
    unmaskedRenderer: dbg ? gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null,
    maxTexture: P('MAX_TEXTURE_SIZE'),
    maxCube: P('MAX_CUBE_MAP_TEXTURE_SIZE'),
    maxAttribs: P('MAX_VERTEX_ATTRIBS'),
    maxVertexUniform: P('MAX_VERTEX_UNIFORM_VECTORS'),
    viewportDims: Array.from(P('MAX_VIEWPORT_DIMS')),
    viewportKind: P('MAX_VIEWPORT_DIMS').constructor.name,
    extensions: gl.getSupportedExtensions(),
    pixels: read(),
    pixelsAgain: read(),
  });
})()
"""


def main():
    claims = Claims("0031 — WebGL parameters and pixels", CORE)

    config = {
        "gpu": {"webglParams": PARAMS, "webglExtensions": EXTENSIONS},
        "noise": {"canvasSeed": 0x061A0001},
    }
    with launch(CORE, config) as s:
        a = json.loads(s.js(PROBE))
        if a.get("error"):
            print(f"\nFAIL — {a['error']}: this host has no WebGL to measure")
            return 1
        print(f"  renderer: {a['renderer']}")
        print(f"  limits: texture {a['maxTexture']} attribs {a['maxAttribs']} "
              f"viewport {a['viewportDims']}")
        print(f"  extensions: {len(a['extensions'])}")

        claims.check(a["vendor"] == PARAMS["VENDOR"]
                     and a["renderer"] == PARAMS["RENDERER"],
                     f"VENDOR and RENDERER are the persona's card")
        claims.check(a["unmaskedVendor"] == PARAMS["UNMASKED_VENDOR_WEBGL"]
                     and a["unmaskedRenderer"] == PARAMS["UNMASKED_RENDERER_WEBGL"],
                     f"and so are the UNMASKED_* pair behind "
                     f"WEBGL_debug_renderer_info, which is where every library "
                     f"actually looks")

        # The limits, which is where a strings-only spoof contradicts itself.
        claims.check(a["maxTexture"] == PARAMS["MAX_TEXTURE_SIZE"]
                     and a["maxCube"] == PARAMS["MAX_CUBE_MAP_TEXTURE_SIZE"],
                     f"the texture limits are the card's, not this machine's "
                     f"({a['maxTexture']}, {a['maxCube']})")
        claims.check(a["maxAttribs"] == PARAMS["MAX_VERTEX_ATTRIBS"]
                     and a["maxVertexUniform"] == PARAMS["MAX_VERTEX_UNIFORM_VECTORS"],
                     f"and so are the uniform and attribute counts "
                     f"({a['maxAttribs']}, {a['maxVertexUniform']})")
        claims.check(a["viewportDims"] == [32767, 32767],
                     f"MAX_VIEWPORT_DIMS is the persona's, parsed out of the "
                     f"comma-joined form the catalogue stores "
                     f"(got {a['viewportDims']})")
        claims.check(a["viewportKind"] == "Int32Array",
                     f"and it comes back as an Int32Array, as it does on every "
                     f"real WebGL — a Float32Array here would name this build in "
                     f"one line of script (got {a['viewportKind']!r})")

        claims.check(sorted(a["extensions"]) == sorted(EXTENSIONS),
                     f"getSupportedExtensions returns exactly the persona's list "
                     f"({len(a['extensions'])} of {len(EXTENSIONS)})")

        # The strings nobody configured must still be true.
        claims.check("WebGL" in a["version"] and "GLSL" in a["slVersion"],
                     f"VERSION and SHADING_LANGUAGE_VERSION were not configured "
                     f"and still answer honestly — a getParameter that answers "
                     f"for everything is its own tell "
                     f"({a['version']!r}, {a['slVersion']!r})")

        claims.check(a["pixels"] == a["pixelsAgain"],
                     "readPixels twice gives the same bytes — the same rule as "
                     "canvas, and for the same reason")

    with launch(CORE, {"gpu": {"webglParams": PARAMS},
                       "noise": {"canvasSeed": 0x061A0002}}) as s:
        b = json.loads(s.js(PROBE))
        claims.check(b["pixels"] != a["pixels"],
                     "a different seed renders different pixels — the shaded "
                     "triangle is a fingerprint no string spoof reaches")

    with launch(CORE, None) as s:
        bare = json.loads(s.js(PROBE))
        print(f"  unconfigured renderer: {bare['renderer']}")
        print(f"  unconfigured extensions: {len(bare['extensions'])}")
        # Not MAX_TEXTURE_SIZE: this Mac reports 16384 as well, and a control
        # that happens to agree with the persona proves nothing.
        claims.control(
            bare["renderer"] != PARAMS["RENDERER"]
            and bare["viewportDims"] != [32767, 32767]
            and len(bare["extensions"]) != len(EXTENSIONS)
            and bare["pixels"] != a["pixels"],
            f"an unconfigured build reports this machine's card, its viewport "
            f"limits, its full extension list and its untouched pixels "
            f"(got {bare['renderer']!r}, {bare['viewportDims']}, "
            f"{len(bare['extensions'])} extensions)",
        )

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
