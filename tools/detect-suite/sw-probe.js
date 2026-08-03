// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors
//
// GENERATED from WORKER_PROBE_SRC in probe.js by tools/detect-suite/sync-sw.py.
// Do not edit: a ServiceWorker that answers differently from the dedicated
// Worker would report a disagreement that is the harness's, not the browser's.
//
// It exists as a file because a ServiceWorker script must be same-origin and
// cannot be registered from a blob: URL — which is why that context read
// `__absent: TypeError` in every capture before this.

    self.onmessage = async function (ev) {
      const reply = (d) => (ev.ports && ev.ports[0] ? ev.ports[0].postMessage(d) : reply(d));
      function hash(str) {
        let h = 0x811c9dc5;
        for (let i = 0; i < str.length; i++) { h ^= str.charCodeAt(i); h = Math.imul(h, 0x01000193); }
        return (h >>> 0).toString(16).padStart(8, '0');
      }
      function safe(fn) { try { const v = fn(); return v === undefined ? null : v; } catch (e) { return '__error:' + e.name; } }

      const out = {
        userAgent: safe(() => navigator.userAgent),
        platform: safe(() => navigator.platform),
        hardwareConcurrency: safe(() => navigator.hardwareConcurrency),
        deviceMemory: safe(() => navigator.deviceMemory),
        language: safe(() => navigator.language),
        languages: safe(() => (navigator.languages || []).join(',')),
        timezone: safe(() => Intl.DateTimeFormat().resolvedOptions().timeZone),
        timezoneOffset: safe(() => new Date().getTimezoneOffset()),
        locale: safe(() => Intl.DateTimeFormat().resolvedOptions().locale),
        numberFormat: safe(() => (123456.789).toLocaleString(undefined, {})),
        calendar: safe(() => Intl.DateTimeFormat().resolvedOptions().calendar),
        webdriver: safe(() => navigator.webdriver),
        userAgentDataPlatform: safe(() => navigator.userAgentData && navigator.userAgentData.platform),
        userAgentDataMobile: safe(() => navigator.userAgentData && navigator.userAgentData.mobile),
      };

      // OffscreenCanvas gives a Worker the full canvas and WebGL surface. A
      // spoofer that only patches the main-thread canvas is caught right here.
      out.canvasHash = safe(() => {
        const c = new OffscreenCanvas(280, 60);
        const ctx = c.getContext('2d');
        ctx.textBaseline = 'alphabetic';
        ctx.font = '14px Arial';
        ctx.fillStyle = '#f60';
        ctx.fillRect(125, 1, 62, 20);
        ctx.fillStyle = '#069';
        ctx.fillText('Fury <canvas> 1.0 \\u00e9\\u00e8\\u00ea \\ud83d\\ude03', 2, 15);
        const d = ctx.getImageData(0, 0, 280, 60).data;
        return hash(Array.from(d).join(','));
      });

      out.webglUnmasked = safe(() => {
        const c = new OffscreenCanvas(64, 64);
        const gl = c.getContext('webgl');
        if (!gl) return null;
        const dbg = gl.getExtension('WEBGL_debug_renderer_info');
        if (!dbg) return null;
        return gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) + ' :: ' + gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL);
      });

      out.webglMaxTextureSize = safe(() => {
        const gl = new OffscreenCanvas(8, 8).getContext('webgl');
        return gl ? gl.getParameter(gl.MAX_TEXTURE_SIZE) : null;
      });

      reply(out);
    };
  
