// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useCallback, useEffect, useState } from "react";
import { api, type LocalProxy, type Profile } from "../api";
import { useI18n } from "../i18n";
import { useAsk } from "./Ask";
import { ProxyForm } from "./ProxyForm";
import { ProxyPaste } from "./ProxyPaste";

/** The proxies this machine knows about.
 *
 *  A section rather than a list buried in the profile editor, because an exit
 *  outlives the profile that first needed it: the same residential IP carries
 *  three accounts, and the place to see that is here — the "used by" column is
 *  the one number that matters, since several accounts behind one address is
 *  the oldest farm signal there is. */
export function Proxies({ profiles }: { profiles: Profile[] }) {
  const { t, say } = useI18n();
  const { ask, dialog } = useAsk();
  const [rows, setRows] = useState<LocalProxy[]>([]);
  const [editing, setEditing] = useState<LocalProxy | null | undefined>(undefined);
  const [pasting, setPasting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setRows(await api.proxies());
      setError(null);
    } catch (e) {
      setError(say(e));
    }
  }, []);

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
        <div className="spacer" />
        <button className="ghost" onClick={() => void load()}>
          {t("bar.refresh")}
        </button>
      </div>

      {error && <div className="notice warnBar">{error}</div>}

      <div className="tableWrap">
        {rows.length === 0 ? (
          <p className="empty pad">{t("px.none")}</p>
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
                    <div className="muted small">{p.kind}</div>
                  </td>
                  <td className="mono">
                    {p.host}:{p.port}
                    {p.username && <div className="muted small">{p.username}</div>}
                  </td>
                  <td className="muted small">
                    {p.last_ip ?? "—"}
                    {p.last_country ? ` · ${p.last_country}` : ""}
                  </td>
                  <td className="muted">
                    {usedBy(p.id) === 0 ? "—" : t("px.usedByN", { n: usedBy(p.id) })}
                  </td>
                  <td className="actions">
                    <div>
                      <button className="ghost" onClick={() => setEditing(p)}>
                        {t("row.edit")}
                      </button>
                      <button
                        className="ghost"
                        onClick={async () => {
                          const go = await ask({
                            title: t("row.delete"),
                            detail: t("px.confirmDelete", { name: p.name }),
                            confirmLabel: t("ui.delete"),
                            danger: true,
                          });
                          if (go === null) return;
                          await api.deleteProxy(p.id);
                          await load();
                        }}
                      >
                        {t("row.delete")}
                      </button>
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
