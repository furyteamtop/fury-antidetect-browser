#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""Patch 0031, part two: each kind of WebGL context answers as itself.

A WebGL1 and a WebGL2 context on the same machine list different extensions:
WebGL2 has its own (EXT_color_buffer_float, OES_draw_buffers_indexed) and
drops the ones it made core (ANGLE_instanced_arrays, OES_texture_float). One
list for both left a WebGL2 context with only the names the two shared -- 18
where Chrome on the same Windows machine lists 32 (ctx-fury baseline). So:

  * gpu.webgl1Extensions and gpu.webgl2Extensions answer their own context;
  * without them, gpu.webglExtensions still answers both (an older persona);
  * getExtension() agrees with getSupportedExtensions(): a name the persona
    does not list is null even when the host's driver has it, and names
    compare without case, as getExtension() does;
  * getShaderPrecisionFormat() answers what gpu.shaderPrecision states (a
    Mali-G76: 15,15,10 for lowp and mediump floats) and the host for the rest.

Usage: core/verify/verify-0031-contexts.py <core binary>
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]

READ = """
(() => {
  const out = {};
  for (const kind of ['webgl', 'webgl2']) {
    const gl = document.createElement('canvas').getContext(kind);
    if (!gl) { out[kind] = null; continue; }
    const prec = {};
    for (const sh of ['VERTEX_SHADER', 'FRAGMENT_SHADER'])
      for (const p of ['LOW_FLOAT', 'MEDIUM_FLOAT', 'HIGH_FLOAT', 'LOW_INT', 'MEDIUM_INT', 'HIGH_INT']) {
        const f = gl.getShaderPrecisionFormat(gl[sh], gl[p]);
        prec[sh + '.' + p] = [f.rangeMin, f.rangeMax, f.precision].join(',');
      }
    out[kind] = {
      list: gl.getSupportedExtensions(),
      get: Object.fromEntries(['OES_texture_float', 'oes_TEXTURE_float', 'EXT_color_buffer_float',
                               'ANGLE_instanced_arrays', 'WEBGL_lose_context']
                              .map(n => [n, gl.getExtension(n) !== null])),
      prec,
    };
  }
  return JSON.stringify(out);
})()
"""

WEBGL1 = ["ANGLE_instanced_arrays", "EXT_blend_minmax", "OES_texture_float", "WEBGL_lose_context"]
WEBGL2 = ["EXT_color_buffer_float", "EXT_texture_filter_anisotropic", "WEBGL_lose_context"]
MALI = {"VERTEX_SHADER": {"LOW_FLOAT": "15,15,10", "MEDIUM_FLOAT": "15,15,10", "HIGH_INT": "31,30,0"},
        "FRAGMENT_SHADER": {"LOW_FLOAT": "15,15,10", "MEDIUM_FLOAT": "15,15,10"}}


def main():
    claims = Claims("0031 — WebGL1 and WebGL2 contexts, shader precision", CORE)

    with launch(CORE, None) as s:
        bare = json.loads(s.js(READ))
    if not bare.get("webgl") or not bare.get("webgl2"):
        print("\nFAIL — this host has no WebGL1 or WebGL2 context; nothing below means anything.")
        return 1
    host1, host2 = set(bare["webgl"]["list"]), set(bare["webgl2"]["list"])
    claims.control(host1 != host2 and {"OES_texture_float", "EXT_color_buffer_float"} <= host1 | host2,
                   f"the host lists {len(host1)} extensions in WebGL1 and {len(host2)} in WebGL2, "
                   f"not the same list, and has the ones asked about below")

    config = {"gpu": {"webglExtensions": WEBGL1 + WEBGL2, "webgl1Extensions": WEBGL1,
                      "webgl2Extensions": WEBGL2, "shaderPrecision": MALI}}
    with launch(CORE, config) as s:
        got = json.loads(s.js(READ))
    w1, w2 = got["webgl"], got["webgl2"]
    print(f"  webgl1: {w1['list']}\n  webgl2: {w2['list']}")
    claims.check(sorted(w1["list"]) == sorted(WEBGL1) and sorted(w2["list"]) == sorted(WEBGL2),
                 f"each context lists its own extensions: WebGL1 {w1['list']}, WebGL2 {w2['list']}")
    claims.check(w1["get"]["OES_texture_float"] and w1["get"]["oes_TEXTURE_float"]
                 and not w1["get"]["EXT_color_buffer_float"] and w1["get"]["WEBGL_lose_context"],
                 f"WebGL1 getExtension() agrees with its list, without case: {w1['get']}")
    claims.check(not w2["get"]["OES_texture_float"] and not w2["get"]["ANGLE_instanced_arrays"]
                 and w2["get"]["EXT_color_buffer_float"],
                 f"WebGL2 getExtension() agrees with its list: {w2['get']}")
    for kind, p in (("webgl", w1["prec"]), ("webgl2", w2["prec"])):
        stated = {f"{sh}.{k}": v for sh, kv in MALI.items() for k, v in kv.items()}
        claims.check(all(p[k] == v for k, v in stated.items())
                     and all(p[k] == bare[kind]["prec"][k] for k in p if k not in stated),
                     f"{kind}: shader precision is the persona's where it says (VERTEX lowp "
                     f"{p['VERTEX_SHADER.LOW_FLOAT']}, HIGH_INT {p['VERTEX_SHADER.HIGH_INT']}) "
                     f"and the host's elsewhere (FRAGMENT HIGH_INT {p['FRAGMENT_SHADER.HIGH_INT']})")

    # A persona from before the per-context lists: one list for both.
    with launch(CORE, {"gpu": {"webglExtensions": WEBGL1}}) as s:
        old = json.loads(s.js(READ))
    claims.check(sorted(old["webgl"]["list"]) == sorted(WEBGL1)
                 and set(old["webgl2"]["list"]) <= set(WEBGL1)
                 and old["webgl"]["prec"] == bare["webgl"]["prec"],
                 f"without per-context lists the one list answers both, as before "
                 f"(WebGL2 {old['webgl2']['list']}), and precision is the host's")
    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
