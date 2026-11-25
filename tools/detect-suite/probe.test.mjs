// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors
//
//     node --test tools/detect-suite/probe.test.mjs
//
// What issue #48 asked to be sure of: no browser API can hold up the capture,
// and what did not answer is named. Run on fake browser objects, so a Promise
// that never settles is one line and not a browser in a bad mood.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

const SRC = readFileSync(new URL('./probe.js', import.meta.url), 'utf8');
const never = () => new Promise(() => {});

/** probe.js in a realm whose clock runs `speed` times fast. */
function load(globals = {}, speed = 50) {
  const t0 = Date.now();
  class FastDate extends Date {
    static now() { return t0 + (Date.now() - t0) * speed; }
  }
  const sandbox = {
    module: { exports: {} },
    console,
    Date: FastDate,
    setTimeout: (f, ms, ...a) => setTimeout(f, (ms || 0) / speed, ...a),
    clearTimeout,
    setInterval: (f, ms, ...a) => setInterval(f, (ms || 0) / speed, ...a),
    clearInterval,
    navigator: {},
    location: { hostname: 'example.test', protocol: 'https:', search: '', href: 'https://example.test/' },
    ...globals,
  };
  sandbox.window = sandbox;
  sandbox.parent = sandbox;
  sandbox.self = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(SRC, sandbox);
  return sandbox.module.exports;
}

/** speechSynthesis that plays `script`: [[ms, voices], ...]. */
function speech(script, speed, repeatEvery) {
  const listeners = new Set();
  let voices = [];
  const fire = () => listeners.forEach((f) => f());
  for (const [ms, v] of script) {
    setTimeout(() => { voices = v; fire(); }, ms / speed).unref();
  }
  if (repeatEvery) setInterval(fire, repeatEvery / speed).unref();
  return {
    getVoices: () => voices,
    addEventListener: (_, f) => listeners.add(f),
    removeEventListener: (_, f) => listeners.delete(f),
    listeners,
  };
}
const voice = (name) => ({ name, lang: 'en-US', localService: true, default: false });

test('a promise that never settles times out and is cleaned up', async () => {
  const p = load();
  let closed = 0;
  const v = await p.withTimeout(never(), 500, 'x', () => closed++);
  assert.equal(v.__timeout, 'x');
  assert.equal(closed, 1);
});

test('an answer in time is the answer, and nothing is closed behind it', async () => {
  const p = load();
  let closed = 0;
  assert.equal(await p.withTimeout(Promise.resolve(7), 500, 'x', () => closed++), 7);
  await new Promise((r) => setTimeout(r, 40));
  assert.equal(closed, 0);
});

test('every kind of outcome has its own status', async () => {
  const p = load();
  const report = {};
  const fail = (name) => () => { const e = new Error('x'); e.name = name; throw e; };
  await p.vector(report, 'ok', async () => ({ a: 1 }));
  await p.vector(report, 'absent', async () => ({ __absent: true }));
  await p.vector(report, 'blocked', fail('NotAllowedError'));
  await p.vector(report, 'error', fail('TypeError'));
  await p.vector(report, 'timeout', never, 300);
  await p.vector(report, 'part', async () => ({ a: 1, b: { __timeout: 'b' } }));
  for (const k of ['ok', 'absent', 'blocked', 'error', 'timeout']) assert.equal(report[k].status, k);
  assert.equal(report.part.status, 'timeout');
  assert.equal(report.error.error, 'TypeError');
});

test('a voice list that repeats itself settles', async () => {
  const list = [voice('A'), voice('B')];
  const s = speech([[200, list]], 50, 100);
  const p = load({ speechSynthesis: s });
  const report = {};
  const v = await p.vector(report, 'speech', () => p.collectSpeech(report), 10000);
  assert.equal(v.count, 2);
  assert.equal(report.speech.status, 'ok');
  assert.equal(report.speech.history.length, 1);
  assert.ok(report.speech.events > 3, 'the repeats were counted');
  assert.equal(s.listeners.size, 0);
});

test('voices that arrive in two parts are both there (issue #44)', async () => {
  const s = speech([[200, [voice('Google')]], [1200, [voice('Google'), voice('System')]]], 50);
  const p = load({ speechSynthesis: s });
  const report = {};
  const v = await p.vector(report, 'speech', () => p.collectSpeech(report), 10000);
  assert.equal(v.count, 2);
  assert.equal(report.speech.history.length, 2);
});

test('a voice list that never stops changing is a timeout with its history', async () => {
  const script = Array.from({ length: 60 }, (_, i) => [i * 300, [voice('v' + i)]]);
  const s = speech(script, 50);
  const p = load({ speechSynthesis: s });
  const report = {};
  const v = await p.vector(report, 'speech', () => p.collectSpeech(report), 10000);
  assert.equal(report.speech.status, 'timeout');
  assert.ok(v.count === 1 && report.speech.history.length > 3);
  assert.equal(s.listeners.size, 0);
});

test('no voices at all is an empty list, not a timeout', async () => {
  const p = load({ speechSynthesis: speech([], 50) });
  const report = {};
  const v = await p.vector(report, 'speech', () => p.collectSpeech(report), 10000);
  assert.equal(v.__empty, true);
  assert.equal(report.speech.status, 'ok');
});

/** A browser in which everything asynchronous hangs. */
function hungBrowser(log) {
  return {
    navigator: {
      userAgentData: { getHighEntropyValues: never },
      gpu: { requestAdapter: never },
      mediaDevices: { enumerateDevices: never },
      requestMediaKeySystemAccess: never,
      permissions: { query: never },
      keyboard: { getLayoutMap: never },
      getBattery: never,
      storage: { estimate: never },
      serviceWorker: { register: never },
    },
    RTCPeerConnection: class {
      createDataChannel() {}
      createOffer() { return never(); }
      close() { log.push('pc'); }
    },
    Worker: class { postMessage() {} terminate() { log.push('worker'); } },
    SharedWorker: class {
      port = { start() {}, postMessage() {}, close() { log.push('port'); } };
    },
    Blob: class {},
    URL: { createObjectURL: () => 'blob:x', revokeObjectURL() {} },
    addEventListener() {},
    removeEventListener() {},
  };
}

test('a capture in a browser that answers nothing still finishes, and twice', async () => {
  const log = [];
  const p = load(hungBrowser(log), 100);
  for (let run = 0; run < 2; run++) {
    const started = Date.now();
    const d = await p.furyProbe();
    assert.ok(Date.now() - started < 1000, 'inside the longest deadline');
    assert.equal(d.__capture.status, 'partial');
    for (const k of ['clientHints', 'webgpu', 'mediaDevices', 'drm', 'webrtc', 'permissions',
      'keyboard', 'battery', 'storage', 'worker', 'sharedWorker', 'serviceWorker']) {
      assert.equal(d.__capture.vectors[k].status, 'timeout', k);
      assert.ok(d.__capture.incomplete.includes(k), k);
    }
    // Serialisable, or there is no file to save.
    assert.equal(JSON.parse(JSON.stringify(d)).__capture.status, 'partial');
  }
  for (const what of ['pc', 'worker', 'port']) {
    assert.equal(log.filter((x) => x === what).length, 2, what + ' closed once per run');
  }
});

test('an answer after the end changes nothing', async () => {
  const p = load({
    navigator: { getBattery: () => new Promise((r) => setTimeout(() => r({ level: 1 }), 400)) },
  }, 100);
  const d = await p.furyProbe();
  const before = JSON.stringify(d);
  await new Promise((r) => setTimeout(r, 500));
  assert.equal(JSON.stringify(d), before);
  assert.equal(d.engine.misc ? d.engine.misc.battery.__timeout : d.__capture.vectors.battery.status,
    d.engine.misc ? 'battery' : 'timeout');
});
