#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
# Copyright 2026 Bogdan Shapovalov and the Fury authors
"""WebRTC gathers what a stock Chrome gathers, and none of it is real.

Under 0070's policy a page gathers no candidates at all, which detectors read
as "WebRTC disabled" (issue 31). 0071 makes the allocator announce, when the
config carries the exit address, the shape a stock Chrome 157 gathered on the
same Mac on 13.11.2026:

  no STUN server:  <uuid>.local typ host                       2113937151
  with one:        the same, plus <ip> typ srflx raddr 0.0.0.0  1677729535
                   rport 0, and the SDP's c= line on that address

Claims:

  * control: the same policy without webrtc.publicIp gathers nothing.
  * no STUN server: one host candidate, under the configured name, no srflx.
  * a STUN server: host plus srflx carrying the configured address, raddr
    cleared, priorities as Chrome's, and the SDP's c= line on that address.
  * two peer connections share the name, as frames of one Chrome run do.
  * a STUN server that cannot answer (loopback) gets no srflx.
  * nothing connects: a pair of local peer connections does not reach
    "connected", because no candidate has a socket behind it.

What this does not show is that nothing leaves the machine; the port has no
socket to send from, which is a property of the code, not of this page.

Usage: core/verify/verify-0071.py <core binary>
"""

import json
import os
import sys
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harness import Claims, launch  # noqa: E402

CORE = sys.argv[1]
EXIT = "203.0.113.7"
NAME = f"{uuid.uuid4()}.local"
POLICY = {"ipHandlingPolicy": "disable_non_proxied_udp"}

GATHER = """(async () => {
  const run = async (servers) => {
    const pc = new RTCPeerConnection({iceServers: servers});
    pc.createDataChannel('x');
    const cands = [];
    pc.onicecandidate = e => e.candidate && cands.push({
      line: e.candidate.candidate, type: e.candidate.type,
      address: e.candidate.address, priority: e.candidate.priority,
      raddr: e.candidate.relatedAddress, rport: e.candidate.relatedPort});
    await pc.setLocalDescription(await pc.createOffer());
    await new Promise(r => { const t = setTimeout(r, 8000);
      pc.onicegatheringstatechange = () => pc.iceGatheringState === 'complete' && (clearTimeout(t), r()); });
    const sdp = pc.localDescription.sdp.split('\\r\\n');
    pc.close();
    return {state: pc.iceGatheringState, cands, c: sdp.find(l => l.startsWith('c=')) || ''};
  };
  return JSON.stringify({
    none: await run([]),
    stun: await run([{urls: 'stun:stun.l.google.com:19302'}]),
    again: await run([{urls: 'stun:stun.l.google.com:19302'}]),
    loopback: await run([{urls: 'stun:127.0.0.1:3478'}]),
  });
})()"""

CONNECT = """(async () => {
  const a = new RTCPeerConnection(), b = new RTCPeerConnection();
  a.onicecandidate = e => e.candidate && b.addIceCandidate(e.candidate).catch(() => {});
  b.onicecandidate = e => e.candidate && a.addIceCandidate(e.candidate).catch(() => {});
  a.createDataChannel('x');
  await a.setLocalDescription(await a.createOffer());
  await b.setRemoteDescription(a.localDescription);
  await b.setLocalDescription(await b.createAnswer());
  await a.setRemoteDescription(b.localDescription);
  await new Promise(r => setTimeout(r, 6000));
  const s = a.iceConnectionState; a.close(); b.close(); return s;
})()"""


def of(run, kind):
    return [c for c in run["cands"] if c["type"] == kind]


def main():
    claims = Claims("0071 — synthetic WebRTC candidates", CORE)

    with launch(CORE, {"webrtc": dict(POLICY)}) as s:
        bare = json.loads(s.js(GATHER))
    claims.control(
        not bare["none"]["cands"] and not bare["stun"]["cands"],
        f"0070's policy alone gathers nothing, with or without a STUN server "
        f"({len(bare['stun']['cands'])} candidates)",
    )

    with launch(CORE, {"webrtc": dict(POLICY, publicIp=EXIT, hostName=NAME)}) as s:
        got = json.loads(s.js(GATHER))
        state = s.js(CONNECT)
    print(json.dumps(got, indent=1))

    none, stun = got["none"], got["stun"]
    claims.check(
        [c["address"] for c in none["cands"]] == [NAME] and of(none, "host"),
        f"without a STUN server: one host candidate, named {NAME} "
        f"({[c['line'] for c in none['cands']]})",
    )
    host, srflx = of(stun, "host"), of(stun, "srflx")
    claims.check(len(host) == 1 and host[0]["address"] == NAME and host[0]["priority"] == 2113937151,
                 f"with a STUN server the host candidate is there, as Chrome's "
                 f"({[(c['address'], c['priority']) for c in host]})")
    claims.check(
        len(srflx) == 1 and srflx[0]["address"] == EXIT and srflx[0]["priority"] == 1677729535
        and srflx[0]["raddr"] == "0.0.0.0" and srflx[0]["rport"] == 0,
        f"and one srflx candidate carries the exit address with raddr 0.0.0.0 "
        f"rport 0 and Chrome's priority ({[c['line'] for c in srflx]})",
    )
    claims.check(stun["c"] == f"c=IN IP4 {EXIT}",
                 f"the SDP's default address is the exit, as in Chrome ({stun['c']})")
    claims.check(not any(EXIT in c["line"] for c in none["cands"]) and none["c"] == "c=IN IP4 0.0.0.0",
                 "without a STUN server the exit address appears nowhere")
    claims.check([c["address"] for c in of(got["again"], "host")] == [NAME],
                 "a second peer connection shows the same name")
    claims.check(not of(got["loopback"], "srflx"),
                 f"a STUN server on loopback, which could not answer, gives no srflx "
                 f"({[c['line'] for c in got['loopback']['cands']]})")
    claims.check(state not in ("connected", "completed"),
                 f"and nothing connects: a local pair stays at {state!r}")

    return claims.done()


if __name__ == "__main__":
    sys.exit(main())
