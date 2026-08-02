// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useState } from "react";
import { api, type LocalProxy } from "../api";
import { useI18n } from "../i18n";

const KINDS = ["socks5", "http", "https"] as const;

/** The proxy fields, as a dialog.
 *
 *  The same set the profile editor shows inline — deliberately the same, so
 *  someone who learned them in one place recognises them in the other. The
 *  check is here for the same reason it is there: a proxy that does not work
 *  becomes a profile that will not open, and finding that out at launch is the
 *  wrong moment. */
export function ProxyForm({
  editing,
  onClose,
  onSaved,
}: {
  editing: LocalProxy | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useI18n();
  const [name, setName] = useState(editing?.name ?? "");
  const [kind, setKind] = useState(editing?.kind ?? "socks5");
  const [host, setHost] = useState(editing?.host ?? "");
  const [port, setPort] = useState(String(editing?.port ?? ""));
  const [user, setUser] = useState(editing?.username ?? "");
  const [pass, setPass] = useState(editing?.password ?? "");
  const [rotate, setRotate] = useState(editing?.rotate_url ?? "");
  const [checker, setChecker] = useState(editing?.checker_url ?? "");
  const [check, setCheck] = useState<{
    ok: boolean; error?: string; ip?: string; country?: string;
    city?: string; timezone?: string; ms?: number;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const complete = host.trim() !== "" && Number(port) > 0;
  const url = () => {
    const auth = user ? `${encodeURIComponent(user)}:${encodeURIComponent(pass)}@` : "";
    return `${kind}://${auth}${host.trim()}:${Number(port)}`;
  };

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "88vh" }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{editing ? t("px.edit") : t("px.new")}</h2>
        </div>

        <div className="form" style={{ paddingTop: "var(--s-5)" }}>
          <div className="field">
            <label>{t("px.type")}</label>
            <div className="segmented">
              {KINDS.map((k) => (
                <button key={k} aria-pressed={kind === k} onClick={() => setKind(k)}>
                  {k}
                </button>
              ))}
            </div>
          </div>

          <div className="field">
            <label htmlFor="f-host">{t("px.address")}</label>
            <div>
              <div className="row">
                <input id="f-host" value={host} autoFocus placeholder="exit.provider.net"
                  onChange={(e) => setHost(e.target.value)} />
                <input style={{ width: 92 }} value={port} placeholder="1080" inputMode="numeric"
                  onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))} />
                <button style={{ whiteSpace: "nowrap" }} disabled={busy || !complete}
                  onClick={async () => {
                    setBusy(true);
                    setCheck(null);
                    try {
                      setCheck(await api.checkProxy(url(), checker, editing?.id ?? null));
                    } finally {
                      setBusy(false);
                    }
                  }}>
                  {busy ? t("px.checking") : t("px.checkButton")}
                </button>
              </div>
              {check && (
                <div className={check.ok ? "verdict good" : "verdict bad"} style={{ marginTop: "var(--s-2)" }}>
                  {check.ok
                    ? [check.ip, check.country, check.city, check.timezone].filter(Boolean).join(" · ")
                    : check.error}
                </div>
              )}
            </div>
          </div>

          <div className="field">
            <label htmlFor="f-user">{t("px.credentials")}</label>
            <div className="row">
              <input id="f-user" value={user} placeholder={t("px.user")} autoComplete="off"
                onChange={(e) => setUser(e.target.value)} />
              <input value={pass} placeholder={t("px.password")} type="password" autoComplete="off"
                onChange={(e) => setPass(e.target.value)} />
            </div>
          </div>

          <div className="field">
            <label htmlFor="f-name">{t("px.label")}</label>
            <div>
              <input id="f-name" value={name}
                placeholder={host ? `${host}:${port || "…"}` : "EU residential"}
                onChange={(e) => setName(e.target.value)} />
              <p className="hint">{t("px.labelHint")}</p>
            </div>
          </div>

          <div className="field">
            <label htmlFor="f-rotate">{t("px.rotate")}</label>
            <div>
              <input id="f-rotate" value={rotate} autoComplete="off"
                placeholder="https://provider.example/rotate?key=…"
                onChange={(e) => setRotate(e.target.value)} />
              <p className="hint">{t("px.rotateHint")}</p>
            </div>
          </div>

          <div className="field">
            <label htmlFor="f-checker">{t("px.checker")}</label>
            <div>
              <input id="f-checker" value={checker} autoComplete="off"
                placeholder={t("px.checkerDefault")}
                onChange={(e) => setChecker(e.target.value)} />
              <p className="hint">{t("px.checkerHint")}</p>
            </div>
          </div>
        </div>

        <div className="modalFoot">
          {error && <span className="error">{error}</span>}
          <div className="spacer" />
          <button className="ghost" onClick={onClose}>{t("ui.cancel")}</button>
          <button className="primary" disabled={busy || !complete}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                await api.saveProxy({
                  id: editing?.id ?? "",
                  name: name.trim() || `${host.trim()}:${port}`,
                  kind, host: host.trim(), port: Number(port),
                  username: user || null, password: pass || null,
                  last_country: check?.country ?? editing?.last_country ?? null,
                  last_ip: check?.ip ?? editing?.last_ip ?? null,
                  rotate_url: rotate.trim() || null,
                  checker_url: checker.trim() || null,
                });
                onSaved();
              } catch (e) {
                setError((e as Error).message);
              } finally {
                setBusy(false);
              }
            }}>
            {editing ? t("ui.save") : t("px.add")}
          </button>
        </div>
      </div>
    </div>
  );
}
