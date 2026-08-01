/*
 * Fury detect-suite — fingerprint probe.
 *
 * Dumps every value an anti-bot system can read, as a flat comparable object,
 * and — crucially — reads the same values again from a Worker and from several
 * kinds of iframe, then reports where the answers disagree.
 *
 * Cross-context disagreement is the point. A product that spoofs the main frame
 * and forgets `new Worker(...)` looks perfect on a naive checker and is trivially
 * caught by CreepJS. `crossContext.disagreements` is therefore the single most
 * important number this probe produces: for real Chrome it is 0, and for Fury it
 * must also be 0.
 *
 * No dependencies, no build step, no network. Runs three ways:
 *   1. probe.html   — open in any browser (AdsPower, real Chrome, anything)
 *   2. DevTools     — paste this file, then `await furyProbe()`
 *   3. CDP          — Runtime.evaluate with awaitPromise (see src/main.rs)
 *
 * Usage:  const dump = await furyProbe();
 */

(function () {
  'use strict';

  const SCHEMA = 1;

  // ---------------------------------------------------------------------------
  // helpers
  // ---------------------------------------------------------------------------

  /* Stable 32-bit string hash (FNV-1a). Used to compress big binary readbacks
   * into something a diff can compare. Not cryptographic — it only needs to
   * change when the bytes change. */
  function hash(str) {
    let h = 0x811c9dc5;
    for (let i = 0; i < str.length; i++) {
      h ^= str.charCodeAt(i);
      h = Math.imul(h, 0x01000193);
    }
    return (h >>> 0).toString(16).padStart(8, '0');
  }

  /* Never let one broken vector kill the whole dump: a missing API is itself a
   * finding, so record it and carry on. */
  function safe(fn, fallback) {
    try {
      const v = fn();
      return v === undefined ? (fallback ?? null) : v;
    } catch (e) {
      return { __error: String((e && e.name) || e) };
    }
  }

  async function safeAsync(fn, fallback) {
    try {
      const v = await fn();
      return v === undefined ? (fallback ?? null) : v;
    } catch (e) {
      return { __error: String((e && e.name) || e) };
    }
  }

  function withTimeout(promise, ms, label) {
    return Promise.race([
      promise,
      new Promise((resolve) => setTimeout(() => resolve({ __timeout: label }), ms)),
    ]);
  }

  // ---------------------------------------------------------------------------
  // navigator
  // ---------------------------------------------------------------------------

  function collectNavigator() {
    const n = navigator;
    return {
      userAgent: safe(() => n.userAgent),
      appVersion: safe(() => n.appVersion),
      appName: safe(() => n.appName),
      platform: safe(() => n.platform),
      vendor: safe(() => n.vendor),
      oscpu: safe(() => n.oscpu, '<absent>'),
      product: safe(() => n.product),
      productSub: safe(() => n.productSub),
      language: safe(() => n.language),
      languages: safe(() => (n.languages || []).join(',')),
      hardwareConcurrency: safe(() => n.hardwareConcurrency),
      deviceMemory: safe(() => n.deviceMemory, '<absent>'),
      maxTouchPoints: safe(() => n.maxTouchPoints),
      cookieEnabled: safe(() => n.cookieEnabled),
      doNotTrack: safe(() => n.doNotTrack),
      pdfViewerEnabled: safe(() => n.pdfViewerEnabled),
      // Present-but-false is normal; present-and-true or absent are both signals.
      webdriver: safe(() => n.webdriver),
      onLine: safe(() => n.onLine),
      // Chrome ships an empty-ish plugin array; a truly empty one is unusual.
      plugins: safe(() =>
        Array.from(n.plugins || [])
          .map((p) => `${p.name}|${p.filename}|${p.description}`)
          .join('~')
      ),
      pluginCount: safe(() => (n.plugins || []).length),
      mimeTypes: safe(() =>
        Array.from(n.mimeTypes || [])
          .map((m) => m.type)
          .join(',')
      ),
      // Feature presence is platform-correlated; absence on a claimed Windows
      // profile is as telling as a wrong value.
      hasBluetooth: safe(() => 'bluetooth' in n),
      hasUsb: safe(() => 'usb' in n),
      hasHid: safe(() => 'hid' in n),
      hasSerial: safe(() => 'serial' in n),
      hasCredentials: safe(() => 'credentials' in n),
      hasInk: safe(() => 'ink' in n),
      hasGpu: safe(() => 'gpu' in n),
    };
  }

  async function collectClientHints() {
    if (!navigator.userAgentData) return { __absent: true };
    const hints = [
      'architecture',
      'bitness',
      'model',
      'platformVersion',
      'uaFullVersion',
      'fullVersionList',
      'wow64',
      'formFactors',
    ];
    return await safeAsync(async () => {
      const high = await navigator.userAgentData.getHighEntropyValues(hints);
      return {
        brands: (navigator.userAgentData.brands || [])
          .map((b) => `${b.brand}/${b.version}`)
          .join(','),
        mobile: navigator.userAgentData.mobile,
        platform: navigator.userAgentData.platform,
        architecture: high.architecture,
        bitness: high.bitness,
        model: high.model,
        platformVersion: high.platformVersion,
        uaFullVersion: high.uaFullVersion,
        fullVersionList: (high.fullVersionList || [])
          .map((b) => `${b.brand}/${b.version}`)
          .join(','),
        wow64: high.wow64,
        formFactors: Array.isArray(high.formFactors) ? high.formFactors.join(',') : high.formFactors,
      };
    });
  }

  // ---------------------------------------------------------------------------
  // screen & window
  // ---------------------------------------------------------------------------

  function collectScreen() {
    const s = screen;
    return {
      width: safe(() => s.width),
      height: safe(() => s.height),
      availWidth: safe(() => s.availWidth),
      availHeight: safe(() => s.availHeight),
      availLeft: safe(() => s.availLeft),
      availTop: safe(() => s.availTop),
      colorDepth: safe(() => s.colorDepth),
      pixelDepth: safe(() => s.pixelDepth),
      orientationType: safe(() => s.orientation && s.orientation.type),
      orientationAngle: safe(() => s.orientation && s.orientation.angle),
      devicePixelRatio: safe(() => devicePixelRatio),
      innerWidth: safe(() => innerWidth),
      innerHeight: safe(() => innerHeight),
      outerWidth: safe(() => outerWidth),
      outerHeight: safe(() => outerHeight),
      // Height of the browser chrome. Differs between Windows and macOS and
      // between versions; contradicting the claimed OS here is a detection.
      chromeHeight: safe(() => outerHeight - innerHeight),
      chromeWidth: safe(() => outerWidth - innerWidth),
      screenX: safe(() => screenX),
      screenY: safe(() => screenY),
      // ~15-17 on Windows, 0 on macOS overlay scrollbars. Gives the OS away
      // on its own, and no amount of navigator spoofing hides it.
      scrollbarWidth: safe(() => {
        const d = document.createElement('div');
        d.style.cssText = 'width:100px;height:100px;overflow:scroll;position:absolute;top:-9999px';
        document.body.appendChild(d);
        const w = d.offsetWidth - d.clientWidth;
        d.remove();
        return w;
      }),
      visualViewportScale: safe(() => visualViewport && visualViewport.scale),
      visualViewportWidth: safe(() => visualViewport && visualViewport.width),
    };
  }

  // ---------------------------------------------------------------------------
  // canvas 2d
  // ---------------------------------------------------------------------------

  function drawCanvasFingerprint(canvas) {
    const ctx = canvas.getContext('2d');
    canvas.width = 280;
    canvas.height = 60;
    ctx.textBaseline = 'top';
    ctx.font = '14px "Arial"';
    ctx.textBaseline = 'alphabetic';
    ctx.fillStyle = '#f60';
    ctx.fillRect(125, 1, 62, 20);
    ctx.fillStyle = '#069';
    // Emoji and diacritics stress the font stack, which is where platform
    // differences actually show up.
    ctx.fillText('Fury <canvas> 1.0 éèê 😃', 2, 15);
    ctx.fillStyle = 'rgba(102, 204, 0, 0.7)';
    ctx.fillText('Fury <canvas> 1.0 éèê 😃', 4, 45);
    ctx.globalCompositeOperation = 'multiply';
    ctx.fillStyle = 'rgb(255,0,255)';
    ctx.beginPath();
    ctx.arc(50, 50, 50, 0, Math.PI * 2, true);
    ctx.closePath();
    ctx.fill();
    return ctx;
  }

  function collectCanvas2d() {
    return safe(() => {
      const canvas = document.createElement('canvas');
      drawCanvasFingerprint(canvas);
      const dataUrl = canvas.toDataURL();
      const ctx = canvas.getContext('2d');
      const pixels = ctx.getImageData(0, 0, canvas.width, canvas.height).data;

      // Two independent readbacks of the same content. A spoofer that reseeds
      // per call fails here, and a site can run exactly this check.
      const canvas2 = document.createElement('canvas');
      drawCanvasFingerprint(canvas2);
      const dataUrl2 = canvas2.toDataURL();

      return {
        toDataURL: hash(dataUrl),
        /* Hash EVERY pixel. An earlier version hashed the first 4096 bytes,
         * which on a 280x60 canvas is the first 3.6 rows — the text is drawn in
         * rows 5-45, so the one hash meant to catch font and anti-aliasing
         * differences saw none of them and matched across every machine tested.
         * Measured, not assumed: three different browsers produced the same
         * value until this was fixed. */
        getImageData: hash(Array.from(pixels).join(',')),
        getImageDataBytes: pixels.length,
        stableAcrossCalls: dataUrl === dataUrl2,
        isPointInPath: ctx.isPointInPath(50, 50),
        // Text metrics are a separate, finer-grained font probe.
        textMetrics: (() => {
          const m = ctx.measureText('Fury Browser mmmmmmmmmmlli');
          return [
            m.width,
            m.actualBoundingBoxAscent,
            m.actualBoundingBoxDescent,
            m.actualBoundingBoxLeft,
            m.actualBoundingBoxRight,
            m.fontBoundingBoxAscent,
            m.fontBoundingBoxDescent,
          ].join('|');
        })(),
        winding: (() => {
          const c = document.createElement('canvas').getContext('2d');
          c.rect(0, 0, 10, 10);
          c.rect(2, 2, 6, 6);
          return c.isPointInPath(5, 5, 'evenodd');
        })(),
      };
    });
  }

  function collectClientRects() {
    return safe(() => {
      const el = document.createElement('div');
      el.style.cssText =
        'position:absolute;left:-9999px;width:11.7331px;height:5.2439px;' +
        'transform:rotate(11.7deg) skewX(3.1deg);font:11.3px Arial';
      el.textContent = 'Fury';
      document.body.appendChild(el);
      const r = el.getBoundingClientRect();
      const out = [r.width, r.height, r.top, r.left, r.right, r.bottom, r.x, r.y].join('|');
      el.remove();
      return { rect: out, hash: hash(out) };
    });
  }

  // ---------------------------------------------------------------------------
  // WebGL
  // ---------------------------------------------------------------------------

  const WEBGL_PARAMS = [
    'VERSION', 'SHADING_LANGUAGE_VERSION', 'VENDOR', 'RENDERER',
    'MAX_TEXTURE_SIZE', 'MAX_VIEWPORT_DIMS', 'MAX_RENDERBUFFER_SIZE',
    'MAX_CUBE_MAP_TEXTURE_SIZE', 'MAX_TEXTURE_IMAGE_UNITS',
    'MAX_VERTEX_TEXTURE_IMAGE_UNITS', 'MAX_COMBINED_TEXTURE_IMAGE_UNITS',
    'MAX_VERTEX_ATTRIBS', 'MAX_VERTEX_UNIFORM_VECTORS',
    'MAX_FRAGMENT_UNIFORM_VECTORS', 'MAX_VARYING_VECTORS',
    'ALIASED_LINE_WIDTH_RANGE', 'ALIASED_POINT_SIZE_RANGE',
    'MAX_TEXTURE_MAX_ANISOTROPY_EXT', 'RED_BITS', 'GREEN_BITS', 'BLUE_BITS',
    'ALPHA_BITS', 'DEPTH_BITS', 'STENCIL_BITS', 'SUBPIXEL_BITS',
    'SAMPLE_BUFFERS', 'SAMPLES',
  ];

  const WEBGL2_PARAMS = [
    'MAX_3D_TEXTURE_SIZE', 'MAX_ARRAY_TEXTURE_LAYERS', 'MAX_COLOR_ATTACHMENTS',
    'MAX_DRAW_BUFFERS', 'MAX_ELEMENT_INDEX', 'MAX_ELEMENTS_INDICES',
    'MAX_ELEMENTS_VERTICES', 'MAX_FRAGMENT_INPUT_COMPONENTS',
    'MAX_FRAGMENT_UNIFORM_BLOCKS', 'MAX_FRAGMENT_UNIFORM_COMPONENTS',
    'MAX_PROGRAM_TEXEL_OFFSET', 'MAX_SAMPLES', 'MAX_SERVER_WAIT_TIMEOUT',
    'MAX_TEXTURE_LOD_BIAS', 'MAX_TRANSFORM_FEEDBACK_SEPARATE_COMPONENTS',
    'MAX_UNIFORM_BLOCK_SIZE', 'MAX_UNIFORM_BUFFER_BINDINGS',
    'MAX_VARYING_COMPONENTS', 'MAX_VERTEX_OUTPUT_COMPONENTS',
    'MAX_VERTEX_UNIFORM_BLOCKS', 'MAX_VERTEX_UNIFORM_COMPONENTS',
    'MIN_PROGRAM_TEXEL_OFFSET', 'UNIFORM_BUFFER_OFFSET_ALIGNMENT',
  ];

  const SHADER_PRECISION_TARGETS = [
    ['VERTEX_SHADER', 'LOW_FLOAT'], ['VERTEX_SHADER', 'MEDIUM_FLOAT'],
    ['VERTEX_SHADER', 'HIGH_FLOAT'], ['VERTEX_SHADER', 'HIGH_INT'],
    ['FRAGMENT_SHADER', 'LOW_FLOAT'], ['FRAGMENT_SHADER', 'MEDIUM_FLOAT'],
    ['FRAGMENT_SHADER', 'HIGH_FLOAT'], ['FRAGMENT_SHADER', 'HIGH_INT'],
  ];

  function collectWebglFrom(gl, isV2) {
    if (!gl) return { __absent: true };

    const params = {};
    const paramTypes = {};
    const names = isV2 ? WEBGL_PARAMS.concat(WEBGL2_PARAMS) : WEBGL_PARAMS;
    for (const name of names) {
      params[name] = safe(() => {
        const v = gl.getParameter(gl[name]);
        if (v && v.length !== undefined && typeof v !== 'string') return Array.from(v).join(',');
        return v;
      });
      // The JS type, separately from the value. Joining an array to a string
      // throws it away, and the type is a fingerprint of its own: MAX_VIEWPORT_
      // DIMS is an Int32Array on every real implementation and ALIASED_LINE_
      // WIDTH_RANGE a Float32Array, so an override that returns the right
      // numbers in the wrong container names the build in one line of script.
      // Ours did, for exactly as long as this probe could not see it.
      paramTypes[name] = safe(() => {
        const v = gl.getParameter(gl[name]);
        return v === null ? 'null' : v.constructor ? v.constructor.name : typeof v;
      });
    }

    // The unmasked strings are the headline values every checker prints, but
    // they are only credible if the numeric params above agree with them.
    const dbg = safe(() => gl.getExtension('WEBGL_debug_renderer_info'));
    const unmasked = dbg && !dbg.__error
      ? {
          vendor: safe(() => gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL)),
          renderer: safe(() => gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL)),
        }
      : { __absent: true };

    const precision = {};
    for (const [shader, prec] of SHADER_PRECISION_TARGETS) {
      precision[`${shader}.${prec}`] = safe(() => {
        const p = gl.getShaderPrecisionFormat(gl[shader], gl[prec]);
        return `${p.rangeMin},${p.rangeMax},${p.precision}`;
      });
    }

    // Render something and read it back: catches noise injected at readPixels
    // and, more usefully, noise that is *not* stable between two reads.
    const render = safe(() => {
      const vs = gl.createShader(gl.VERTEX_SHADER);
      gl.shaderSource(vs, 'attribute vec2 p;void main(){gl_Position=vec4(p,0.,1.);}');
      gl.compileShader(vs);
      const fs = gl.createShader(gl.FRAGMENT_SHADER);
      gl.shaderSource(
        fs,
        'precision mediump float;void main(){gl_FragColor=vec4(gl_FragCoord.x/64.,gl_FragCoord.y/64.,.5,1.);}'
      );
      gl.compileShader(fs);
      const prog = gl.createProgram();
      gl.attachShader(prog, vs);
      gl.attachShader(prog, fs);
      gl.linkProgram(prog);
      gl.useProgram(prog);

      const buf = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, buf);
      gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 1, -1, 0, 1]), gl.STATIC_DRAW);
      const loc = gl.getAttribLocation(prog, 'p');
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, 2, gl.FLOAT, false, 0, 0);
      gl.drawArrays(gl.TRIANGLES, 0, 3);

      const read = () => {
        const px = new Uint8Array(64 * 64 * 4);
        gl.readPixels(0, 0, 64, 64, gl.RGBA, gl.UNSIGNED_BYTE, px);
        return hash(Array.from(px).join(','));
      };
      const a = read();
      const b = read();
      return { readPixels: a, stableAcrossCalls: a === b };
    });

    return {
      unmasked,
      params,
      paramTypes,
      precision,
      render,
      extensions: safe(() => (gl.getSupportedExtensions() || []).sort().join(',')),
      contextAttributes: safe(() => JSON.stringify(gl.getContextAttributes())),
    };
  }

  function collectWebgl() {
    const c = document.createElement('canvas');
    return {
      webgl1: collectWebglFrom(
        safe(() => c.getContext('webgl') || c.getContext('experimental-webgl')),
        false
      ),
      webgl2: collectWebglFrom(
        safe(() => document.createElement('canvas').getContext('webgl2')),
        true
      ),
    };
  }

  // ---------------------------------------------------------------------------
  // WebGPU
  // ---------------------------------------------------------------------------

  /* The vector most competitors still leave open: they rewrite the WebGL
   * renderer string and leave adapter.limits describing the real GPU. */
  async function collectWebgpu() {
    if (!navigator.gpu) return { __absent: true };
    return await safeAsync(async () => {
      const adapter = await navigator.gpu.requestAdapter();
      if (!adapter) return { __noAdapter: true };

      const limits = {};
      for (const k in adapter.limits) limits[k] = adapter.limits[k];

      const info = adapter.info || (adapter.requestAdapterInfo ? await adapter.requestAdapterInfo() : {});

      return {
        vendor: info.vendor,
        architecture: info.architecture,
        device: info.device,
        description: info.description,
        subgroupMinSize: info.subgroupMinSize,
        subgroupMaxSize: info.subgroupMaxSize,
        isFallbackAdapter: adapter.isFallbackAdapter,
        features: Array.from(adapter.features || []).sort().join(','),
        limits,
        limitsHash: hash(JSON.stringify(limits)),
        preferredCanvasFormat: safe(() => navigator.gpu.getPreferredCanvasFormat()),
      };
    });
  }

  // ---------------------------------------------------------------------------
  // Audio
  // ---------------------------------------------------------------------------

  async function collectAudio() {
    const ctxInfo = safe(() => {
      const AC = window.AudioContext || window.webkitAudioContext;
      if (!AC) return { __absent: true };
      const ac = new AC();
      const out = {
        sampleRate: ac.sampleRate,
        baseLatency: ac.baseLatency,
        outputLatency: ac.outputLatency,
        maxChannelCount: ac.destination.maxChannelCount,
        numberOfInputs: ac.destination.numberOfInputs,
        numberOfOutputs: ac.destination.numberOfOutputs,
        channelCount: ac.destination.channelCount,
        channelCountMode: ac.destination.channelCountMode,
        channelInterpretation: ac.destination.channelInterpretation,
      };
      ac.close();
      return out;
    });

    // The classic offline render fingerprint: oscillator through a compressor.
    const renderOnce = () =>
      new Promise((resolve, reject) => {
        try {
          const OAC = window.OfflineAudioContext || window.webkitOfflineAudioContext;
          if (!OAC) return resolve({ __absent: true });
          const ctx = new OAC(1, 44100, 44100);
          const osc = ctx.createOscillator();
          osc.type = 'triangle';
          osc.frequency.value = 10000;
          const comp = ctx.createDynamicsCompressor();
          comp.threshold.value = -50;
          comp.knee.value = 40;
          comp.ratio.value = 12;
          comp.attack.value = 0;
          comp.release.value = 0.25;
          osc.connect(comp);
          comp.connect(ctx.destination);
          osc.start(0);
          ctx.startRendering();
          ctx.oncomplete = (e) => {
            const buf = e.renderedBuffer.getChannelData(0);
            let sum = 0;
            for (let i = 4500; i < 5000; i++) sum += Math.abs(buf[i]);
            resolve({ sum: sum, hash: hash(Array.from(buf.slice(4500, 5000)).join(',')) });
          };
          setTimeout(() => resolve({ __timeout: 'offlineAudio' }), 3000);
        } catch (e) {
          reject(e);
        }
      });

    const first = await safeAsync(renderOnce);
    const second = await safeAsync(renderOnce);

    return {
      context: ctxInfo,
      offline: first,
      stableAcrossCalls: JSON.stringify(first) === JSON.stringify(second),
    };
  }

  // ---------------------------------------------------------------------------
  // Fonts — measurement, not enumeration
  // ---------------------------------------------------------------------------

  /* A list-only filter is defeated by this: render text in the candidate font
   * with a known fallback and compare widths. If the font is missing the width
   * equals the fallback's. Any real font filter must act on font fallback. */
  const FONT_CANDIDATES = [
    // Windows-only
    'Bahnschrift', 'Calibri', 'Cambria', 'Candara', 'Consolas', 'Constantia',
    'Corbel', 'Ebrima', 'Gadugi', 'Leelawadee UI', 'Malgun Gothic',
    'Microsoft JhengHei', 'Microsoft YaHei', 'MS Gothic', 'MV Boli',
    'Nirmala UI', 'Segoe UI', 'Segoe UI Emoji', 'Segoe UI Variable',
    'Sitka', 'Sylfaen', 'Tahoma', 'Yu Gothic',
    // macOS-only
    'Al Bayan', 'American Typewriter', 'Apple Chancery', 'Apple Color Emoji',
    'AppleGothic', 'Avenir', 'Avenir Next', 'Baskerville', 'Big Caslon',
    'Chalkboard', 'Chalkduster', 'Cochin', 'Copperplate', 'Didot',
    'Futura', 'Geneva', 'Gill Sans', 'Helvetica Neue', 'Herculanum',
    'Hoefler Text', 'Lucida Grande', 'Marker Felt', 'Menlo', 'Monaco',
    'Optima', 'Papyrus', 'Phosphate', 'Rockwell', 'SF Pro', 'Skia',
    'Snell Roundhand', 'Zapfino',
    // Cross-platform / bundled with apps
    'Arial', 'Arial Black', 'Comic Sans MS', 'Courier New', 'Georgia',
    'Impact', 'Times New Roman', 'Trebuchet MS', 'Verdana', 'Webdings',
    'Wingdings', 'Roboto', 'Open Sans', 'Inter',
  ];

  function collectFonts() {
    return safe(() => {
      const canvas = document.createElement('canvas');
      const ctx = canvas.getContext('2d');
      const text = 'mmmmmmmmmmlliWWWQQ@#%&';
      const size = '72px';

      const baselines = {};
      for (const fb of ['monospace', 'sans-serif', 'serif']) {
        ctx.font = `${size} ${fb}`;
        baselines[fb] = ctx.measureText(text).width;
      }

      const present = [];
      const widths = {};
      for (const font of FONT_CANDIDATES) {
        let detected = false;
        for (const fb of ['monospace', 'sans-serif', 'serif']) {
          ctx.font = `${size} "${font}", ${fb}`;
          const w = ctx.measureText(text).width;
          if (w !== baselines[fb]) {
            detected = true;
            widths[font] = w;
            break;
          }
        }
        if (detected) present.push(font);
      }

      /* NOT a detection method — verified empirically: Chrome's
       * document.fonts.check() returns true for arbitrary family names,
       * including "ThisFontDoesNotExistAnywhere12345". It answers "can this be
       * rendered", and fallback means the answer is always yes.
       *
       * Recorded anyway because the *behaviour* is a baseline value: if a
       * patched build ever makes check() discriminate, it stops matching real
       * Chrome and becomes a detection in its own right. */
      const fontsCheckDiscriminates = safe(() => {
        const real = document.fonts.check('12px "Arial"');
        const fake = document.fonts.check('12px "NoSuchFamily_zX91qq"');
        return real !== fake;
      });

      return {
        detectedByMeasurement: present.sort(),
        countByMeasurement: present.length,
        widthsHash: hash(JSON.stringify(widths)),
        baselineWidths: baselines,
        // Real Chrome: false. Any other value is itself an anomaly.
        fontsCheckDiscriminates,
        // Real enumeration lives behind a permission prompt.
        hasQueryLocalFonts: 'queryLocalFonts' in window,
        localFontsPermission: null, // filled by collectPermissions -> 'local-fonts'
      };
    });
  }

  // ---------------------------------------------------------------------------
  // Media: devices, codecs, DRM
  // ---------------------------------------------------------------------------

  async function collectMediaDevices() {
    if (!navigator.mediaDevices || !navigator.mediaDevices.enumerateDevices) {
      return { __absent: true };
    }
    return await safeAsync(async () => {
      const devices = await navigator.mediaDevices.enumerateDevices();
      return {
        count: devices.length,
        kinds: devices.map((d) => d.kind).sort().join(','),
        // Without permission labels are empty; deviceIds should be stable per
        // profile but differ between profiles.
        deviceIdsHash: hash(devices.map((d) => d.deviceId).join(',')),
        groupIdsHash: hash(devices.map((d) => d.groupId).join(',')),
        labelsEmpty: devices.every((d) => d.label === ''),
        supportedConstraints: safe(() =>
          Object.keys(navigator.mediaDevices.getSupportedConstraints()).sort().join(',')
        ),
      };
    });
  }

  const CODECS = [
    'video/mp4; codecs="avc1.42E01E"',
    'video/mp4; codecs="avc1.640028"',
    'video/mp4; codecs="hev1.1.6.L93.B0"',
    'video/mp4; codecs="av01.0.05M.08"',
    'video/webm; codecs="vp8"',
    'video/webm; codecs="vp9"',
    'video/ogg; codecs="theora"',
    'audio/mp4; codecs="mp4a.40.2"',
    'audio/mpeg',
    'audio/webm; codecs="opus"',
    'audio/ogg; codecs="vorbis"',
    'audio/flac',
    'audio/wav; codecs="1"',
  ];

  function collectCodecs() {
    return safe(() => {
      const video = document.createElement('video');
      const audio = document.createElement('audio');
      const out = {};
      for (const c of CODECS) {
        const el = c.startsWith('video') ? video : audio;
        out[c] = {
          canPlayType: safe(() => el.canPlayType(c)),
          mseSupported: safe(() => window.MediaSource && MediaSource.isTypeSupported(c)),
        };
      }
      // H.264 and AAC are the tell: a Chromium built without
      // proprietary_codecs=true reports "" where real Chrome reports
      // "probably". Two lines of JS identify a self-built browser.
      const h264 = out['video/mp4; codecs="avc1.42E01E"'];
      const aac = out['audio/mp4; codecs="mp4a.40.2"'];
      return {
        support: out,
        hasProprietaryCodecs: !!(h264 && h264.canPlayType) && !!(aac && aac.canPlayType),
        recorderTypes: safe(() =>
          ['video/webm;codecs=vp9', 'video/webm;codecs=vp8', 'video/mp4']
            .filter((t) => window.MediaRecorder && MediaRecorder.isTypeSupported(t))
            .join(',')
        ),
      };
    });
  }

  async function collectDrm() {
    if (!navigator.requestMediaKeySystemAccess) return { __absent: true };
    const config = [
      {
        initDataTypes: ['cenc'],
        videoCapabilities: [{ contentType: 'video/mp4;codecs="avc1.42E01E"', robustness: '' }],
      },
    ];
    const systems = ['com.widevine.alpha', 'com.microsoft.playready', 'org.w3.clearkey'];
    const out = {};
    for (const sys of systems) {
      out[sys] = await safeAsync(async () => {
        const access = await navigator.requestMediaKeySystemAccess(sys, config);
        return { available: true, keySystem: access.keySystem };
      });
    }
    // Real Chrome answers yes to Widevine. A build with enable_widevine=false
    // says no, which is a detection unless patch 0061 fakes it. See docs/10.
    return out;
  }

  // ---------------------------------------------------------------------------
  // WebRTC
  // ---------------------------------------------------------------------------

  async function collectWebrtc() {
    if (!window.RTCPeerConnection) return { __absent: true };
    return await withTimeout(
      safeAsync(
        () =>
          new Promise((resolve) => {
            const pc = new RTCPeerConnection({
              iceServers: [{ urls: 'stun:stun.l.google.com:19302' }],
            });
            const candidates = [];
            pc.onicecandidate = (e) => {
              if (!e.candidate) {
                pc.close();
                const ips = [];
                for (const c of candidates) {
                  const m = /([0-9]{1,3}(\.[0-9]{1,3}){3}|[a-f0-9]+(:[a-f0-9]*){2,}|[\w-]+\.local)/i.exec(c);
                  if (m) ips.push(m[1]);
                }
                resolve({
                  completed: true,
                  candidateCount: candidates.length,
                  types: candidates
                    .map((c) => (/ typ (\w+)/.exec(c) || [])[1])
                    .filter(Boolean)
                    .join(','),
                  // Any address here that is neither mDNS .local nor the proxy
                  // exit IP is a leak of the real network.
                  addresses: Array.from(new Set(ips)),
                  usesMdns: ips.some((i) => /\.local$/.test(i)),
                  raw: candidates.length ? hash(candidates.join('|')) : null,
                });
                return;
              }
              candidates.push(e.candidate.candidate);
            };
            pc.createDataChannel('fury');
            pc.createOffer().then((o) => pc.setLocalDescription(o));
          })
      ),
      /* 12s, not 6. Gathering runs through the profile's proxy, and a slow
       * exit made a healthy browser look like one with WebRTC disabled. The
       * two outcomes are different findings and must not share a code path:
       * `completed: true, candidateCount: 0` means blocked, a bare `__timeout`
       * means we simply did not wait long enough. */
      12000,
      'webrtc'
    );
  }

  // ---------------------------------------------------------------------------
  // Locale, time, permissions, CSS
  // ---------------------------------------------------------------------------

  function collectLocale() {
    return {
      timezone: safe(() => Intl.DateTimeFormat().resolvedOptions().timeZone),
      timezoneOffset: safe(() => new Date().getTimezoneOffset()),
      // Two dates six months apart: their offsets reveal the DST rules, which
      // must match the claimed zone.
      offsetJan: safe(() => new Date(2026, 0, 15).getTimezoneOffset()),
      offsetJul: safe(() => new Date(2026, 6, 15).getTimezoneOffset()),
      locale: safe(() => Intl.DateTimeFormat().resolvedOptions().locale),
      calendar: safe(() => Intl.DateTimeFormat().resolvedOptions().calendar),
      numberingSystem: safe(() => Intl.DateTimeFormat().resolvedOptions().numberingSystem),
      dateFormatted: safe(() => new Date(0).toString()),
      dateLocaleString: safe(() => new Date(0).toLocaleString()),
      collatorLocale: safe(() => new Intl.Collator().resolvedOptions().locale),
      numberFormat: safe(() => new Intl.NumberFormat().format(123456.789)),
      relativeTime: safe(() => new Intl.RelativeTimeFormat().format(-1, 'day')),
      displayNameRegion: safe(() =>
        new Intl.DisplayNames(undefined, { type: 'region' }).of('US')
      ),
      listFormat: safe(() => new Intl.ListFormat().format(['a', 'b', 'c'])),
      availableCalendars: safe(() =>
        Intl.supportedValuesOf ? Intl.supportedValuesOf('calendar').length : null
      ),
      availableTimezones: safe(() =>
        Intl.supportedValuesOf ? Intl.supportedValuesOf('timeZone').length : null
      ),
    };
  }

  async function collectPermissions() {
    if (!navigator.permissions) return { __absent: true };
    const names = [
      'geolocation', 'notifications', 'camera', 'microphone', 'midi',
      'clipboard-read', 'clipboard-write', 'push', 'persistent-storage',
      'accelerometer', 'gyroscope', 'local-fonts',
    ];
    const out = {};
    for (const name of names) {
      out[name] = await safeAsync(async () => (await navigator.permissions.query({ name })).state);
    }
    /* Classic headless tell: the two APIs report different states for
     * notifications. But they use different vocabularies for the same states —
     * Permissions API says granted/denied/prompt, the Notification API says
     * granted/denied/default — so 'prompt' and 'default' are the SAME state.
     *
     * Verified against real Chrome 150, which reports prompt/default and must
     * therefore be treated as consistent. Comparing the strings directly marks
     * every real browser as broken. */
    out['__notificationApi'] = safe(() => Notification.permission);
    const normalise = (v) => (v === 'default' ? 'prompt' : v);
    out['__consistent'] =
      normalise(out['notifications']) === normalise(out['__notificationApi']);
    return out;
  }

  const MEDIA_QUERIES = [
    'prefers-color-scheme: dark', 'prefers-color-scheme: light',
    'prefers-reduced-motion: reduce', 'prefers-reduced-transparency: reduce',
    'prefers-contrast: more', 'forced-colors: active', 'inverted-colors: inverted',
    'any-hover: hover', 'any-pointer: fine', 'any-pointer: coarse',
    'hover: hover', 'pointer: fine', 'pointer: coarse',
    'dynamic-range: high', 'color-gamut: srgb', 'color-gamut: p3',
    'color-gamut: rec2020', 'display-mode: browser', 'scripting: enabled',
    'update: fast', 'overflow-block: scroll',
  ];

  function collectCss() {
    return safe(() => {
      const out = {};
      for (const q of MEDIA_QUERIES) out[q] = matchMedia(`(${q})`).matches;
      out['__monochrome'] = safe(() => matchMedia('(monochrome)').matches);
      out['__systemFontStack'] = safe(() => {
        const d = document.createElement('div');
        d.style.font = 'menu';
        document.body.appendChild(d);
        const f = getComputedStyle(d).fontFamily;
        d.remove();
        return f;
      });
      return out;
    });
  }

  async function collectSpeech() {
    if (!window.speechSynthesis) return { __absent: true };
    return await safeAsync(
      () =>
        new Promise((resolve) => {
          const read = () => {
            const v = speechSynthesis.getVoices();
            if (!v.length) return null;
            return {
              count: v.length,
              // Platform-specific voice list. Windows ships "Microsoft ..."
              // voices, macOS ships different ones. Rarely spoofed, often read.
              names: v.map((x) => `${x.name}|${x.lang}|${x.localService}`).sort().join('~'),
              hash: hash(v.map((x) => x.name + x.lang).sort().join(',')),
              defaultVoice: (v.find((x) => x.default) || {}).name || null,
            };
          };
          const now = read();
          if (now) return resolve(now);
          speechSynthesis.onvoiceschanged = () => resolve(read() || { count: 0 });
          setTimeout(() => resolve(read() || { count: 0, __empty: true }), 1500);
        })
    );
  }

  // ---------------------------------------------------------------------------
  // Engine internals & automation traces
  // ---------------------------------------------------------------------------

  function collectEngine() {
    return {
      // Format differs between V8 and SpiderMonkey and reveals injected frames.
      errorStackShape: safe(() => {
        try {
          null.f();
        } catch (e) {
          return e.stack.split('\n').slice(0, 2).join(' | ').replace(/:\d+:\d+/g, ':L:C');
        }
      }),
      hasCaptureStackTrace: safe(() => typeof Error.captureStackTrace === 'function'),
      errorMessage: safe(() => {
        try {
          null.f();
        } catch (e) {
          return e.message;
        }
      }),
      performanceNowPrecision: safe(() => {
        const samples = [];
        for (let i = 0; i < 50; i++) samples.push(performance.now());
        const deltas = samples.slice(1).map((v, i) => v - samples[i]).filter((d) => d > 0);
        return deltas.length ? Math.min.apply(null, deltas) : 0;
      }),
      jsHeapSizeLimit: safe(() => performance.memory && performance.memory.jsHeapSizeLimit),
      totalJSHeapSize: safe(() => performance.memory && performance.memory.totalJSHeapSize),
      mathTan: safe(() => Math.tan(-1e300).toString()),
      mathSinh: safe(() => Math.sinh(1).toString()),
      // Native-code check on the functions a JS-injection spoofer would replace.
      nativeToString: safe(() => {
        const targets = [
          [Navigator.prototype, 'hardwareConcurrency'],
          [Navigator.prototype, 'userAgent'],
          [Screen.prototype, 'width'],
        ];
        return targets
          .map(([proto, prop]) => {
            const d = Object.getOwnPropertyDescriptor(proto, prop);
            if (!d || !d.get) return `${prop}:no-getter`;
            const src = Function.prototype.toString.call(d.get);
            return `${prop}:${/\[native code\]/.test(src) ? 'native' : 'PATCHED'}`;
          })
          .join(',');
      }),
      // Descriptor identity: an overridden accessor is not the original object.
      toStringTag: safe(() => Object.prototype.toString.call(navigator)),
      automation: {
        webdriver: safe(() => navigator.webdriver),
        cdcVars: safe(() =>
          Object.getOwnPropertyNames(window).filter((k) => /^[$_]?cdc_|^\$cdc/.test(k)).join(',')
        ),
        documentCdc: safe(() =>
          Object.getOwnPropertyNames(document).filter((k) => /cdc_|selenium|driver/i.test(k)).join(',')
        ),
        hasChromeObject: safe(() => typeof window.chrome === 'object'),
        chromeKeys: safe(() => (window.chrome ? Object.keys(window.chrome).sort().join(',') : null)),
        chromeRuntime: safe(() => !!(window.chrome && window.chrome.runtime)),
        hasChromeLoadTimes: safe(() => !!(window.chrome && window.chrome.loadTimes)),
        permissionsQueryNative: safe(() =>
          /\[native code\]/.test(String(navigator.permissions.query))
        ),
      },
      misc: {
        hasBattery: 'getBattery' in navigator,
        hasNetworkInfo: 'connection' in navigator,
        connectionType: safe(() => navigator.connection && navigator.connection.effectiveType),
        connectionRtt: safe(() => navigator.connection && navigator.connection.rtt),
        connectionDownlink: safe(() => navigator.connection && navigator.connection.downlink),
        hasComputePressure: 'PressureObserver' in window,
        hasDevicePosture: 'devicePosture' in navigator,
        keyboardLayout: null, // filled asynchronously below
      },
    };
  }

  async function collectKeyboard() {
    if (!navigator.keyboard || !navigator.keyboard.getLayoutMap) return { __absent: true };
    return await safeAsync(async () => {
      const map = await navigator.keyboard.getLayoutMap();
      const keys = [];
      for (const [code, key] of map) keys.push(`${code}=${key}`);
      // Layout must agree with the claimed locale: a ru-RU profile on a US
      // layout is a contradiction.
      return { size: map.size, hash: hash(keys.sort().join(',')), sample: keys.sort().slice(0, 8).join(',') };
    });
  }

  async function collectStorage() {
    return {
      quota: await safeAsync(async () => {
        if (!navigator.storage || !navigator.storage.estimate) return { __absent: true };
        const e = await navigator.storage.estimate();
        // Quota is derived from free disk space; it should be plausible for the
        // claimed deviceMemory and machine.
        return { quota: e.quota, usage: e.usage };
      }),
      hasLocalStorage: safe(() => !!window.localStorage),
      hasIndexedDB: safe(() => !!window.indexedDB),
      cookieEnabled: safe(() => navigator.cookieEnabled),
    };
  }

  // ---------------------------------------------------------------------------
  // Cross-context collection — the part that matters most
  // ---------------------------------------------------------------------------

  /* The subset of values readable in a Worker. Kept as a source string because
   * it has to be shipped into the worker realm verbatim. */
  const WORKER_PROBE_SRC = `
    self.onmessage = async function () {
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

      self.postMessage(out);
    };
  `;

  /* Values read inside an iframe realm. Returned as source because it is
   * evaluated through the iframe's own window. */
  function readFromWindow(w) {
    const safeIn = (fn) => {
      try {
        const v = fn();
        return v === undefined ? null : v;
      } catch (e) {
        return '__error:' + (e && e.name);
      }
    };
    return {
      userAgent: safeIn(() => w.navigator.userAgent),
      platform: safeIn(() => w.navigator.platform),
      hardwareConcurrency: safeIn(() => w.navigator.hardwareConcurrency),
      deviceMemory: safeIn(() => w.navigator.deviceMemory),
      languages: safeIn(() => (w.navigator.languages || []).join(',')),
      maxTouchPoints: safeIn(() => w.navigator.maxTouchPoints),
      webdriver: safeIn(() => w.navigator.webdriver),
      timezone: safeIn(() => w.Intl.DateTimeFormat().resolvedOptions().timeZone),
      timezoneOffset: safeIn(() => new w.Date().getTimezoneOffset()),
      screenWidth: safeIn(() => w.screen.width),
      screenHeight: safeIn(() => w.screen.height),
      availHeight: safeIn(() => w.screen.availHeight),
      devicePixelRatio: safeIn(() => w.devicePixelRatio),
      canvasHash: safeIn(() => {
        const c = w.document.createElement('canvas');
        const ctx = c.getContext('2d');
        c.width = 280;
        c.height = 60;
        ctx.textBaseline = 'alphabetic';
        ctx.font = '14px Arial';
        ctx.fillStyle = '#f60';
        ctx.fillRect(125, 1, 62, 20);
        ctx.fillStyle = '#069';
    ctx.fillText('Fury <canvas> 1.0 éèê 😃', 2, 15);
        return hash(Array.from(ctx.getImageData(0, 0, 280, 60).data).join(','));
      }),
      webglUnmasked: safeIn(() => {
        const gl = w.document.createElement('canvas').getContext('webgl');
        if (!gl) return null;
        const dbg = gl.getExtension('WEBGL_debug_renderer_info');
        if (!dbg) return null;
        return (
          gl.getParameter(dbg.UNMASKED_VENDOR_WEBGL) +
          ' :: ' +
          gl.getParameter(dbg.UNMASKED_RENDERER_WEBGL)
        );
      }),
      nativeToString: safeIn(() => {
        const d = Object.getOwnPropertyDescriptor(w.Navigator.prototype, 'hardwareConcurrency');
        if (!d || !d.get) return 'no-getter';
        return /\[native code\]/.test(Function.prototype.toString.call(d.get)) ? 'native' : 'PATCHED';
      }),
    };
  }

  function collectFromMainRealm() {
    return readFromWindow(window);
  }

  async function collectFromWorker() {
    return await withTimeout(
      safeAsync(
        () =>
          new Promise((resolve, reject) => {
            const blob = new Blob([WORKER_PROBE_SRC], { type: 'application/javascript' });
            const url = URL.createObjectURL(blob);
            const w = new Worker(url);
            w.onmessage = (e) => {
              w.terminate();
              URL.revokeObjectURL(url);
              resolve(e.data);
            };
            w.onerror = (e) => {
              w.terminate();
              URL.revokeObjectURL(url);
              reject(new Error('worker: ' + (e.message || 'failed')));
            };
            w.postMessage('go');
          })
      ),
      5000,
      'worker'
    );
  }

  async function collectFromIframe(kind) {
    return await withTimeout(
      safeAsync(
        () =>
          new Promise((resolve) => {
            const f = document.createElement('iframe');
            f.style.cssText = 'position:absolute;left:-9999px;width:100px;height:100px';
            if (kind === 'srcdoc') f.srcdoc = '<!doctype html><title>fury</title>';
            else if (kind === 'blank') f.src = 'about:blank';
            const done = () => {
              let data;
              try {
                data = readFromWindow(f.contentWindow);
              } catch (e) {
                data = { __error: 'cross-origin: ' + e.name };
              }
              f.remove();
              resolve(data);
            };
            f.onload = done;
            document.body.appendChild(f);
            // about:blank sometimes fires load before the handler attaches.
            setTimeout(done, 400);
          })
      ),
      3000,
      'iframe:' + kind
    );
  }

  /* Compare the same field across contexts. This is the headline result.
   *
   * A value of `null` means the API does not exist in that realm — a Worker has
   * no `screen`, and `navigator.webdriver` is a Window-only property. Treating
   * those as disagreements produces false positives that drown the real ones, so
   * only non-null values are compared against each other, and absences are
   * reported separately as informational. */
  function compareContexts(contexts) {
    const disagreements = [];
    const absences = [];
    const names = Object.keys(contexts);
    const main = contexts.main || {};

    for (const field of Object.keys(main)) {
      const values = {};
      const missing = [];

      for (const ctx of names) {
        const v = contexts[ctx];
        if (!v || v.__timeout || v.__error) continue;
        if (!(field in v)) continue;
        if (v[field] === null) {
          missing.push(ctx);
          continue;
        }
        values[ctx] = v[field];
      }

      const distinct = Array.from(new Set(Object.values(values).map((x) => JSON.stringify(x))));
      if (distinct.length > 1) {
        disagreements.push({ field, values });
      }
      if (missing.length && Object.keys(values).length) {
        absences.push({ field, absentIn: missing });
      }
    }

    return {
      contextsProbed: names,
      disagreementCount: disagreements.length,
      disagreements,
      // Expected: a Worker has no screen or webdriver. Compare this list against
      // the baseline rather than eyeballing it — a NEW absence means a patch
      // removed an API it should have spoofed.
      absences,
      // For real Chrome this is true. If it is false for Fury, a patch is not
      // reaching every execution context — see docs/02 layer 3.
      consistent: disagreements.length === 0,
    };
  }

  // ---------------------------------------------------------------------------
  // entry point
  // ---------------------------------------------------------------------------

  async function furyProbe() {
    const started = Date.now();

    const [
      clientHints, webgpu, audio, mediaDevices, drm, webrtc,
      permissions, speech, keyboard, storage,
      worker, iframeSameOrigin, iframeBlank, iframeSrcdoc,
    ] = await Promise.all([
      collectClientHints(),
      collectWebgpu(),
      collectAudio(),
      collectMediaDevices(),
      collectDrm(),
      collectWebrtc(),
      collectPermissions(),
      collectSpeech(),
      collectKeyboard(),
      collectStorage(),
      collectFromWorker(),
      collectFromIframe('same-origin'),
      collectFromIframe('blank'),
      collectFromIframe('srcdoc'),
    ]);

    const engine = collectEngine();
    engine.misc.keyboardLayout = keyboard;

    const contexts = {
      main: collectFromMainRealm(),
      worker,
      'iframe:same-origin': iframeSameOrigin,
      'iframe:about:blank': iframeBlank,
      'iframe:srcdoc': iframeSrcdoc,
    };

    return {
      __schema: SCHEMA,
      __probeVersion: '0.2.0',
      __collectedInMs: Date.now() - started,

      navigator: collectNavigator(),
      clientHints,
      screen: collectScreen(),
      canvas2d: collectCanvas2d(),
      clientRects: collectClientRects(),
      webgl: collectWebgl(),
      webgpu,
      audio,
      fonts: collectFonts(),
      mediaDevices,
      codecs: collectCodecs(),
      drm,
      webrtc,
      locale: collectLocale(),
      permissions,
      css: collectCss(),
      speech,
      storage,
      engine,

      crossContext: compareContexts(contexts),
      contexts,
    };
  }

  if (typeof window !== 'undefined') {
    window.furyProbe = furyProbe;
  }
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = { furyProbe };
  }
})();
