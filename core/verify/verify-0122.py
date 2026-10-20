#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""NetworkInformation overrides in frames, workers and change notifications.

Usage: python3 core/verify/verify-0122.py <rebuilt core binary>
"""
import sys

from harness import Claims, launch

READ = """(() => {
  const c = navigator.connection;
  return {effectiveType: c.effectiveType, downlink: c.downlink,
          rtt: c.rtt, saveData: c.saveData};
})()"""
CONTEXTS = """(async () => {
  const read = n => ({effectiveType: n.connection.effectiveType,
    downlink: n.connection.downlink, rtt: n.connection.rtt,
    saveData: n.connection.saveData});
  const f = document.createElement('iframe');
  await new Promise((resolve, reject) => {
    f.onload = resolve; f.onerror = reject;
    f.src = 'about:blank'; document.body.appendChild(f);
  });
  const frame = read(f.contentWindow.navigator); f.remove();
  const worker = await new Promise((resolve, reject) => {
    const url = URL.createObjectURL(new Blob([
      `postMessage((${read.toString()})(navigator));`
    ], {type: 'text/javascript'}));
    const w = new Worker(url);
    const timer = setTimeout(() => {w.terminate(); reject(Error('worker timeout'));}, 8000);
    w.onerror = reject;
    w.onmessage = e => {clearTimeout(timer); w.terminate();
      URL.revokeObjectURL(url); resolve(e.data);};
  });
  return {frame, worker};
})()"""


def change_network(s, latency, throughput):
    s.ws.call("Network.emulateNetworkConditions", {
        "offline": False, "latency": latency,
        "downloadThroughput": throughput, "uploadThroughput": throughput,
        "connectionType": "cellular3g",
    }, session=s.session)
    # Let the notifier's networking task deliver the update.
    return s.js("new Promise(r => setTimeout(() => r(true), 500))")


def main():
    core = sys.argv[1]
    claims = Claims("0122 — NetworkInformation", core)
    expected = {"effectiveType": "4g", "downlink": 5.5, "rtt": 150, "saveData": False}
    with launch(core) as s:
        s.js("navigator.connection.addEventListener('change', () => {})")
        change_network(s, 600, 125000)
        bare = s.js(READ)
        claims.control(bare != expected, f"unconfigured network differs: {bare}")

    with launch(core, {"netinfo": expected}) as s:
        claims.check(s.js(READ) == expected, "all four getters return configured values")
        contexts = s.js(CONTEXTS)
        claims.check(contexts == {"frame": expected, "worker": expected},
                     f"iframe and dedicated worker agree: {contexts}")
        claims.check(s.js("Object.getOwnPropertyDescriptor(NetworkInformation.prototype, 'rtt').get.toString().includes('[native code]')"),
                     "the getter remains native")
        s.js("window.netEvents = []; navigator.connection.addEventListener('change', () => netEvents.push('change'))")
        change_network(s, 600, 125000)
        change_network(s, 1000, 62500)
        claims.check(s.js(READ) == expected, "host network changes cannot replace configured values")
        claims.check(s.js("netEvents.length") == 0, "host-only changes do not emit phantom events")

    with launch(core, {"netinfo": {"rtt": 0, "downlink": 0, "saveData": False}}) as s:
        got = s.js(READ)
        claims.check(got["rtt"] == 0 and got["downlink"] == 0 and got["saveData"] is False,
                     "zero and false are overrides, not missing fields")

    with launch(core, {"netinfo": {"rtt": -1, "downlink": -1, "effectiveType": "invalid"}}) as s:
        change_network(s, 600, 125000)
        got = s.js(READ)
        claims.check(got["rtt"] >= 0 and got["downlink"] >= 0 and got["effectiveType"] != "invalid",
                     "invalid fields fall through to Chromium")
    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
