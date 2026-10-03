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

/** Monotonic across every paste of every form instance. Only the newest
 *  paste may touch the form: a slower answer about the line before must not
 *  overwrite the one pasted since -- not the fields, and not the sniffed
 *  type. Module-level on purpose: the component builds a new handler on
 *  each render, and a per-handler counter would reset to 0 and let a stale
 *  answer through. */
let pasteSeq = 0;

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
  return (e: ClipboardEvent<HTMLInputElement>) => {
    const text = e.clipboardData.getData("text").trim();
    // Take the number before anything async, recognised line or not: this
    // paste is now the latest, and every answer still in flight for the one
    // before is stale.
    const mine = ++pasteSeq;
    // A line with no separator is a hostname and nothing else. Let the
    // browser paste it and do not make a round trip to say so.
    if (!text || !/[:@]/.test(text) || text.includes("\n")) return;
    e.preventDefault();
    const input = e.currentTarget;
    // The field as the paste met it. A keystroke after the paste outranks
    // the paste: an answer about the old line must not overwrite what the
    // operator is typing now.
    const metValue = input.value;
    // Not a proxy line after all -- give it what the plain paste would have.
    const plainPaste = () => {
      const start = input.selectionStart ?? input.value.length;
      const end = input.selectionEnd ?? start;
      apply({ host: input.value.slice(0, start) + text + input.value.slice(end), port: "" });
    };
    void api
      .parseProxyLine(text)
      .then((p) => {
        if (mine !== pasteSeq) return;
        // The operator edited the field while the parse ran; the answer no
        // longer describes what is on screen.
        if (input.value !== metValue) return;
        if (!p) {
          plainPaste();
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
        api
          .sniffProxyKind(p.host, p.port)
          .then((r) => {
            if (mine !== pasteSeq) return;
            if (r.kind) onKind(r.kind);
          })
          // No agent, or it could not tell: the button stays as it was, which
          // is what happened before there was a guess at all.
          .catch(() => {});
      })
      // The parser could not be asked (agent down, transport gone). The
      // paste already left the clipboard, so refuse nothing: what the plain
      // paste would have put in the field, put there.
      .catch(() => {
        if (mine !== pasteSeq || input.value !== metValue) return;
        plainPaste();
      });
  };
}
