// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import type { ClipboardEvent } from "react";
import { api } from "./api";

/** The fields a pasted line can fill. Everything is optional because a plain
 *  `host:port` fills two of them and `user:pass@host:port` fills four, and a
 *  field the line did not carry is left as the operator had it. */
export interface Spread {
  kind?: string;
  host: string;
  port: string;
  username?: string;
  password?: string;
}

/** A whole proxy line, pasted into the host box.
 *
 *  Suppliers hand out `ip:port:login:pass` on one line and people paste that
 *  whole line into the first field, which is the reasonable thing to do with
 *  it. Before this, the host field then held all four values joined by colons
 *  and the check button said the host did not resolve. Now the line goes to
 *  the same parser the "paste a list" screen uses, and what it finds lands in
 *  the right boxes.
 *
 *  Only on paste, never on every keystroke: `host:1` while somebody is still
 *  typing a port is a valid `host:port` line, and moving the "1" into the port
 *  box mid-word would be the kind of help nobody wants.
 *
 *  Returns a handler for the input's `onPaste`. Everything that is not a line
 *  the parser recognises -- a bare hostname, an IP, anything else -- is pasted
 *  as it always was, so the fallback is a paste, not a refusal.
 *
 *  `onKind`, when given, hears which protocol the address itself answered on,
 *  for a line that did not say. `host:port:user:pass` names none, and keeping
 *  the pressed type button -- SOCKS5 in a fresh form -- saved HTTP proxies as
 *  SOCKS5, where they look exactly like dead ones (23.09.2026, an operator told
 *  his working proxy was "off"). It arrives after the fields, one round trip
 *  later, and only when exactly one protocol answered. */
export function spreadPastedProxy(
  apply: (found: Spread) => void,
  onKind?: (kind: "http" | "socks5") => void,
) {
  // Only the latest paste may set the type: a slow answer about the line
  // before must not overwrite the one pasted since.
  let seq = 0;
  return (e: ClipboardEvent<HTMLInputElement>) => {
    const text = e.clipboardData.getData("text").trim();
    // A line with no separator is a hostname and nothing else. Let the
    // browser paste it and do not make a round trip to say so.
    if (!text || !/[:@]/.test(text) || text.includes("\n")) return;
    e.preventDefault();
    const input = e.currentTarget;
    void api.parseProxyLine(text).then((p) => {
      if (!p) {
        // Not a proxy line after all -- give it what the paste would have done.
        const start = input.selectionStart ?? input.value.length;
        const end = input.selectionEnd ?? start;
        apply({ host: input.value.slice(0, start) + text + input.value.slice(end), port: "" });
        return;
      }
      apply({
        // The scheme only when the line carried one: `host:port:user:pass`
        // says nothing about the type, and the parser's default is a guess.
        // What the address answers on is found out below.
        kind: p.shape === "Url" ? p.kind : undefined,
        host: p.host,
        port: String(p.port),
        username: p.username ?? undefined,
        password: p.password ?? undefined,
      });
      if (p.shape === "Url" || !onKind) return;
      const mine = ++seq;
      api
        .sniffProxyKind(p.host, p.port)
        .then((r) => {
          if (mine === seq && r.kind) onKind(r.kind);
        })
        // No agent, or it could not tell: the button stays as it was, which
        // is what happened before there was a guess at all.
        .catch(() => {});
    });
  };
}
