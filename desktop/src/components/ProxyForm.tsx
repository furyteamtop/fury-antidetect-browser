// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useRef, useState } from "react";
import { api, type LocalProxy } from "../api";
import { Icon } from "./Icon";
import { NetworkReport } from "./NetworkReport";
import { useI18n } from "../i18n";
import { spreadPastedProxy } from "../proxyLine";
import { proxyUrl, wgEndpoint } from "../proxyUrl";
import { WireGuardConf } from "./WireGuardConf";

const KINDS = ["socks5", "http", "https", "wireguard"] as const;

export const addedByMe = (p: LocalProxy, myEmail: string | null) =>
  !!p.added_by && !!myEmail && p.added_by.toLowerCase() === myEmail.toLowerCase();

/** Why this account may not change a team proxy, as a sentence, or null when
 *  it may. The server's rule (`may_change_proxy`), as the list reports it, so
 *  nobody types every field in again to learn it from the Save button
 *  (tester, 20.10.2026). */
export function proxyLock(
  p: LocalProxy,
  myEmail: string | null,
  t: ReturnType<typeof useI18n>["t"],
): string | null {
  if (p.may_change !== false) return null;
  if (addedByMe(p, myEmail)) {
    return t("px.lockedLostRight", { right: t("perm.edit_proxy") });
  }
  return p.added_by ? t("px.lockedBy", { who: p.added_by }) : t("px.lockedUnknown");
}

/** The proxy fields, as a dialog.
 *
 *  The same set the profile editor shows inline — deliberately the same, so
 *  someone who learned them in one place recognises them in the other. The
 *  check is here for the same reason it is there: a proxy that does not work
 *  becomes a profile that will not open, and finding that out at launch is the
 *  wrong moment. */
export function ProxyForm({
  editing,
  myEmail = null,
  onClose,
  onSaved,
}: {
  editing: LocalProxy | null;
  myEmail?: string | null;
  onClose: () => void;
  onSaved: (id: string) => void;
}) {
  const { t, say } = useI18n();
  const locked = editing ? proxyLock(editing, myEmail, t) : null;
  const [name, setName] = useState(editing?.name ?? "");
  const [kind, setKind] = useState(editing?.kind ?? "socks5");
  /** The type the pasted address answered on, while it is still the one set. */
  const [sniffed, setSniffed] = useState<string | null>(null);
  /** A type button pressed after the paste outranks what the address said. */
  const kindTouched = useRef(false);
  const [typedHost, setHost] = useState(editing?.host ?? "");
  const [typedPort, setPort] = useState(String(editing?.port ?? ""));
  /** A WireGuard proxy's whole .conf. Stored where a password goes. */
  const [conf, setConf] = useState(editing?.kind === "wireguard" ? (editing.password ?? "") : "");
  const wg = kind === "wireguard";
  // For WireGuard the address is the config's Endpoint, not something typed.
  const endpoint = wg ? wgEndpoint(conf) : null;
  // On a team server the form never receives the stored config, so an edit
  // that does not paste a new one keeps the old one (save_proxy reads an
  // empty secret on an edit as "leave it") and its address.
  const keepConf = wg && conf.trim() === "" && editing?.kind === "wireguard";
  const host = wg ? (endpoint?.host ?? (keepConf ? editing!.host : "")) : typedHost;
  const port = wg ? String(endpoint?.port ?? (keepConf ? editing!.port : "")) : typedPort;
  const [user, setUser] = useState(editing?.username ?? "");
  const [pass, setPass] = useState(editing?.password ?? "");
  const [rotate, setRotate] = useState(editing?.rotate_url ?? "");
  const [checker, setChecker] = useState(editing?.checker_url ?? "");
  const [check, setCheck] = useState<{
    ok: boolean; error?: string; ip?: string; country?: string;
    city?: string; timezone?: string; ms?: number; suggested_kind?: string | null;
    error_code?: string | null; error_detail?: string | null; checker_country?: string | null;
  } | null>(null);
  const [diagnosing, setDiagnosing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const complete = host.trim() !== "" && Number(port) > 0;
  /** The address as typed. `k` is passed explicitly when a check follows a
   *  change of protocol in the same click: setKind lands on the next render and
   *  the request would otherwise go out under the kind that just failed. */
  const url = (k = kind) => proxyUrl(k, host, port, user, k === "wireguard" ? conf : pass);

  const runCheck = async (k = kind) => {
    setBusy(true);
    setCheck(null);
    try {
      setCheck(await api.checkProxy(url(k), checker, editing?.id ?? null));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "88vh" }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{locked ? t("px.view") : editing ? t("px.edit") : t("px.new")}</h2>
        </div>

        {locked && (
          <div className="modalNotice" role="status">
            <Icon name="lock" size={16} />
            <div>
              <strong>{t("px.lockedTitle")}</strong>
              <p>{locked}</p>
            </div>
          </div>
        )}

        {/* Locked: the fields still show the proxy and the check still runs,
            but nothing reads as editable that Save would only refuse. */}
        <div className={locked ? "form locked" : "form"} style={{ paddingTop: "var(--s-5)" }}>
          <div className="field">
            <label>{t("px.type")}</label>
            <div className="segmented">
              {KINDS.map((k) => (
                <button key={k} aria-pressed={kind === k} disabled={locked !== null && kind !== k}
                  onClick={() => { setKind(k); kindTouched.current = true; setSniffed(null); }}>
                  {k}
                </button>
              ))}
            </div>
            {sniffed && <div className="hint">{t("px.kindSniffed", { kind: sniffed })}</div>}
          </div>

          <div className="field">
            <label htmlFor={wg ? "f-conf" : "f-host"}>{wg ? t("px.wgConf") : t("px.address")}</label>
            <div>
              {wg && <WireGuardConf id="f-conf" value={conf} onChange={setConf} readOnly={locked !== null} />}
              <div className="row" style={wg ? { marginTop: "var(--s-2)" } : undefined}>
                {!wg && <>
                <input id="f-host" value={host} autoFocus={!locked} readOnly={locked !== null} placeholder="exit.provider.net"
                  onChange={(e) => setHost(e.target.value)}
                  onPaste={spreadPastedProxy((p) => {
                    kindTouched.current = false;
                    setSniffed(null);
                    setHost(p.host);
                    if (p.port) setPort(p.port);
                    if (p.kind) setKind(p.kind as (typeof KINDS)[number]);
                    if (p.username !== undefined) setUser(p.username);
                    if (p.password !== undefined) setPass(p.password);
                  }, (k) => {
                    if (kindTouched.current) return;
                    setKind(k);
                    setSniffed(k);
                  })} />
                <input style={{ width: 92 }} value={port} readOnly={locked !== null} placeholder="1080" inputMode="numeric"
                  onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))} />
                </>}
                <button style={{ whiteSpace: "nowrap" }} disabled={busy || !complete}
                  onClick={() => runCheck()}>
                  {busy ? t("px.checking") : t("px.checkButton")}
                </button>
                <button style={{ whiteSpace: "nowrap" }} className="ghost" disabled={busy || !complete}
                  onClick={() => setDiagnosing(true)}>
                  {t("net.diagnose")}
                </button>
              </div>
              {diagnosing && (
                <NetworkReport
                  url={url()}
                  proxy={editing ? { id: editing.id, name: editing.name, kind: editing.kind, display: `${editing.host}:${editing.port}`, country: editing.last_country } : null}
                  checkerUrl={checker}
                  onClose={() => setDiagnosing(false)}
                />
              )}
              {check && (
                <div className={check.ok ? "verdict good" : "verdict bad"} style={{ marginTop: "var(--s-2)" }}>
                  {check.ok
                    ? [check.ip, check.country, check.city, check.timezone].filter(Boolean).join(" · ")
                    : say({ code: check.error_code ? `pxc.${check.error_code}` : null, message: check.error, vars: { why: check.error_detail ?? "" } })}
                  {check.ok && check.checker_country && (
                    <div className="small" title={t("px.serverInWhy")}>
                      {t("px.serverIn", { country: check.checker_country })}
                    </div>
                  )}
                </div>
              )}
              {/* A provider hands out `host:port:user:pass`, which says nothing
                  about the protocol, so the form has to guess and is wrong
                  about half the time. Being wrong looks identical to a dead
                  proxy — the check waits out its timeout and reports silence —
                  and the fix is one dropdown away. So say which one, and move
                  it from here. */}
              {check && !check.ok && check.suggested_kind && (
                <div className="hint" style={{ marginTop: "var(--s-1)" }}>
                  {t("px.wrongKind", { kind: check.suggested_kind, was: kind })}
                  <button className="ghost" style={{ marginLeft: "var(--s-2)", whiteSpace: "nowrap" }}
                    disabled={busy}
                    onClick={() => {
                      const k = check.suggested_kind as string;
                      setKind(k);
                      setSniffed(null);
                      runCheck(k);
                    }}>
                    {t("px.switchKind", { kind: check.suggested_kind })}
                  </button>
                </div>
              )}
            </div>
          </div>

          {!wg && <div className="field">
            <label htmlFor="f-user">{t("px.credentials")}</label>
            <div>
              <div className="row">
                <input id="f-user" value={user} readOnly={locked !== null} placeholder={editing?.sealed ? t("px.savedUser") : t("px.user")} autoComplete="off"
                  onChange={(e) => setUser(e.target.value)} />
                <input value={pass} readOnly={locked !== null} placeholder={editing?.sealed ? t("px.savedPassword") : t("px.password")} type="password" autoComplete="off"
                  onChange={(e) => setPass(e.target.value)} />
              </div>
              {/* A team proxy's login and password never reach this form, and
                  two empty fields read as "the update lost them". */}
              {editing?.sealed && !locked && <p className="hint">{t("px.savedHint")}</p>}
            </div>
          </div>}

          <div className="field">
            <label htmlFor="f-name">{t("px.label")}</label>
            <div>
              <input id="f-name" value={name} readOnly={locked !== null}
                placeholder={host ? `${host}:${port || "…"}` : "EU residential"}
                onChange={(e) => setName(e.target.value)} />
              {!locked && <p className="hint">{t("px.labelHint")}</p>}
            </div>
          </div>

          {/* A tunnel has no rotation link: its exit is the peer's. */}
          {!wg && <div className="field">
            <label htmlFor="f-rotate">{t("px.rotate")}</label>
            <div>
              <input id="f-rotate" value={rotate} readOnly={locked !== null} autoComplete="off"
                placeholder={locked ? "—" : "https://provider.example/rotate?key=…"}
                onChange={(e) => setRotate(e.target.value)} />
              {!locked && <p className="hint">{t("px.rotateHint")}</p>}
            </div>
          </div>}

          <div className="field">
            <label htmlFor="f-checker">{t("px.checker")}</label>
            <div>
              <input id="f-checker" value={checker} readOnly={locked !== null} autoComplete="off"
                placeholder={t("px.checkerDefault")}
                onChange={(e) => setChecker(e.target.value)} />
              {!locked && <p className="hint">{t("px.checkerHint")}</p>}
            </div>
          </div>
        </div>

        <div className="modalFoot">
          {error && <span className="error">{error}</span>}
          <div className="spacer" />
          <button className={locked ? undefined : "ghost"} onClick={onClose}>{locked ? t("ui.close") : t("ui.cancel")}</button>
          {!locked && <button className="primary" disabled={busy || !complete}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                const saved = await api.saveProxy({
                  id: editing?.id ?? "",
                  name: name.trim() || `${host.trim()}:${port}`,
                  kind, host: host.trim(), port: Number(port),
                  username: wg ? null : (user || null),
                  password: wg ? (conf || null) : (pass || null),
                  last_country: check?.country ?? editing?.last_country ?? null,
                  last_ip: check?.ip ?? editing?.last_ip ?? null,
                  rotate_url: wg ? null : (rotate.trim() || null),
                  checker_url: checker.trim() || null,
                });
                onSaved(saved.id);
              } catch (e) {
                setError(say(e));
              } finally {
                setBusy(false);
              }
            }}>
            {editing ? t("ui.save") : t("px.add")}
          </button>}
        </div>
      </div>
    </div>
  );
}
