// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { IconButton } from "./Icon";
import { useCallback, useEffect, useState } from "react";
import { api, type LocalProxy, type Profile } from "../api";
import { useI18n } from "../i18n";
import { useAsk } from "./Ask";
import { ProxyForm, addedByMe, proxyLock } from "./ProxyForm";
import { ProxyPaste } from "./ProxyPaste";
import { PartnerCard } from "./Partner";
import { proxyUrl } from "../proxyUrl";

/** The proxies this machine knows about.
 *
 *  A section rather than a list buried in the profile editor, because an exit
 *  outlives the profile that first needed it: the same residential IP carries
 *  three accounts, and the place to see that is here — the "used by" column is
 *  the one number that matters, since several accounts behind one address is
 *  the oldest farm signal there is. */
export function Proxies({ profiles, local, myEmail = null }: { profiles: Profile[]; local: boolean; myEmail?: string | null }) {
  const { t, say } = useI18n();
  const { ask, dialog } = useAsk();
  const [rows, setRows] = useState<LocalProxy[]>([]);
  const [editing, setEditing] = useState<LocalProxy | null | undefined>(undefined);
  const [pasting, setPasting] = useState(false);
  const [shopping, setShopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Checking every proxy at once (5.7): per-row outcome, and how far along.
  const [checks, setChecks] = useState<Map<string, { ok: boolean; ms?: number; error?: string }>>(new Map());
  const [checking, setChecking] = useState<{ done: number; total: number } | null>(null);

  /** The URL the single-proxy form would build. In team mode the password is
   *  not here, and check_proxy fetches it by id instead. */
  const urlOf = (p: LocalProxy) => proxyUrl(p.kind, p.host, p.port, p.username, p.password);

  const checkAll = async () => {
    const list = rows;
    setChecks(new Map());
    setChecking({ done: 0, total: list.length });
    let done = 0;
    // Three at a time: the checker is a third party, and forty simultaneous
    // requests from one address is how a checker starts refusing.
    const queue = [...list];
    const worker = async () => {
      for (;;) {
        const p = queue.shift();
        if (!p) return;
        try {
          const r = await api.checkProxy(urlOf(p), p.checker_url, p.id);
          setChecks((m) => new Map(m).set(p.id, { ok: r.ok, ms: r.ms, error: r.ok ? undefined : r.error }));
        } catch (e) {
          setChecks((m) => new Map(m).set(p.id, { ok: false, error: say(e) }));
        }
        done++;
        setChecking({ done, total: list.length });
      }
    };
    await Promise.all([worker(), worker(), worker()]);
    setChecking(null);
    await load();
  };

  /** Local proxies whose password the keychain will not open now. They read
   *  back blank, and nothing used to say so: a tester re-typed every one after
   *  an update (15.10.2026). */
  const [unreadable, setUnreadable] = useState<string[]>([]);
  const [stillLocked, setStillLocked] = useState(false);

  const load = useCallback(async () => {
    try {
      setRows(await api.proxies());
      setError(null);
    } catch (e) {
      setError(say(e));
    }
    if (local) {
      void api.vaultStatus().then((v) => setUnreadable(v.unreadable), () => {});
    }
  }, [local]);

  useEffect(() => {
    void load();
  }, [load]);

  const usedBy = (id: string) => profiles.filter((p) => p.proxy?.id === id).length;

  return (
    <>
      {dialog}
      <div className="toolbar">
        <button className="primary" onClick={() => setEditing(null)}>
          {t("px.newOne")}
        </button>
        <button onClick={() => setPasting(true)}>{t("pp.paste")}</button>
        {rows.length > 0 && (
          <button className="ghost" disabled={checking !== null} onClick={() => void checkAll()}>
            {checking ? t("px.checkingN", { done: checking.done, total: checking.total }) : t("px.checkAll", { n: rows.length })}
          </button>
        )}
        {!checking && checks.size > 0 && (
          <span className="muted small">
            {t("px.checkSummary", { ok: [...checks.values()].filter((c) => c.ok).length, bad: [...checks.values()].filter((c) => !c.ok).length })}
          </span>
        )}
        <div className="spacer" />
        {rows.length > 0 && (
          <button className="ghost" onClick={() => setShopping((v) => !v)}>
            {t("partner.buy")}
          </button>
        )}
        <IconButton icon="refresh" label={t("bar.refresh")} onClick={() => void load()} />
      </div>

      {error && <div className="notice warnBar">{error}</div>}
      {unreadable.length > 0 && (
        <div className="notice warnBar">
          {t("px.vaultUnreadable", { n: unreadable.length, list: unreadable.join(", ") })}
          {stillLocked && <> {t("px.vaultStillLocked")}</>}{" "}
          <button
            className="linky"
            onClick={async () => {
              const v = await api.vaultStatus(true).catch(() => null);
              setStillLocked(v !== null && v.unreadable.length > 0);
              await load();
            }}
          >
            {t("px.vaultRetry")}
          </button>
        </div>
      )}

      {shopping && rows.length > 0 && <PartnerCard onHide={() => setShopping(false)} />}

      <div className="tableWrap">
        {rows.length === 0 ? (
          <div className="pad">
            <p className="empty">{t("px.none")}</p>
            <PartnerCard />
          </div>
        ) : (
          <table className="grid">
            <thead>
              <tr>
                <th>{t("px.label")}</th>
                <th>{t("px.address")}</th>
                <th>{t("px.lastSeen")}</th>
                <th>{t("px.usedBy")}</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {rows.map((p) => (
                <tr key={p.id}>
                  <td>
                    <div className="name">{p.name}</div>
                    <div className="muted small">
                      {p.kind}
                      {p.added_by && <> · {addedByMe(p, myEmail) ? t("px.addedByYou") : t("px.addedBy", { who: p.added_by })}</>}
                    </div>
                  </td>
                  <td className="mono">
                    {p.host}:{p.port}
                    {p.username && <div className="muted small">{p.username}</div>}
                  </td>
                  <td className="muted small">
                    {p.last_ip ?? "—"}
                    {p.last_country ? ` · ${p.last_country}` : ""}
                    {(() => {
                      const c = checks.get(p.id);
                      if (!c) return null;
                      return c.ok ? (
                        <div className="ok">{t("px.checkOk", { ms: c.ms ?? 0 })}</div>
                      ) : (
                        <div className="warn" title={c.error}>{t("px.checkBad")}</div>
                      );
                    })()}
                  </td>
                  <td className="muted">
                    {usedBy(p.id) === 0 ? "—" : t("px.usedByN", { n: usedBy(p.id) })}
                  </td>
                  <td className="actions">
                    <div>
                      {/* The same two icons a profile row has, in the same
                          places: one table's pencil is every table's pencil. */}
                      <IconButton icon="pencil" label={t("row.edit")} onClick={() => setEditing(p)} />
                      <IconButton
                        icon="trash"
                        danger
                        label={t("row.delete")}
                        title={proxyLock(p, myEmail, t) ?? undefined}
                        disabled={proxyLock(p, myEmail, t) !== null}
                        onClick={async () => {
                          const go = await ask({
                            title: t("row.delete"),
                            detail: t("px.confirmDelete", { name: p.name }),
                            confirmLabel: t("ui.delete"),
                            danger: true,
                          });
                          if (go === null) return;
                          try {
                            await api.deleteProxy(p.id);
                          } catch (e) {
                            setError(say(e));
                          }
                          await load();
                        }}
                      />
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      {editing !== undefined && (
        <ProxyForm
          editing={editing}
          myEmail={myEmail}
          onClose={() => setEditing(undefined)}
          onSaved={async () => {
            setEditing(undefined);
            await load();
          }}
        />
      )}

      {pasting && (
        <ProxyPaste onClose={() => setPasting(false)} onSaved={() => void load()} />
      )}
    </>
  );
}
