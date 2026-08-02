// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useCallback, useEffect, useState } from "react";
import { api, type Profile } from "../api";
import { useI18n } from "../i18n";
import { useAsk } from "./Ask";

/** Deleted profiles, and the two things you can do with one.
 *
 *  Deleting hides rather than destroys because a profile holds a warmed
 *  account, and the cost of losing one to a stray click is not symmetric with
 *  the cost of keeping a row nobody wanted. Emptying is the destructive
 *  operation, so it is the one that names what goes: the browser data too. */
export function Trash({ onChanged }: { onChanged: () => void }) {
  const { t } = useI18n();
  const [rows, setRows] = useState<Profile[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { ask, dialog } = useAsk();

  const load = useCallback(async () => {
    // An empty trash and an unreachable one look identical unless the failure
    // is shown. This one hid a deserialisation error for a whole release.
    try {
      setRows(await api.trash());
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (error) {
    return (
      <div className="notice warnBar" role="status">
        {error}
      </div>
    );
  }

  if (rows.length === 0) {
    return (
      <div className="tableWrap">
        {dialog}
        <p className="empty pad">{t("trash.empty")}</p>
      </div>
    );
  }

  return (
    <>
      {dialog}
      <p className="hint" style={{ marginTop: 0, marginBottom: "var(--s-3)" }}>
        {t("trash.hint")}
      </p>
      <div className="tableWrap">
        <table className="grid">
          <thead>
            <tr>
              <th>{t("col.name")}</th>
              <th>{t("col.persona")}</th>
              <th>{t("trash.deletedAt")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((p) => (
              <tr key={p.id}>
                <td>
                  <div className="name">{p.name}</div>
                  {p.tags.length > 0 && (
                    <div className="tags">{p.tags.map((x) => <span key={x}>{x}</span>)}</div>
                  )}
                </td>
                <td className="mono muted">{p.persona_id}</td>
                <td className="muted small">
                  {p.last_opened_at ? new Date(p.last_opened_at).toLocaleString() : ""}
                </td>
                <td className="actions">
                  <div>
                    <button
                      disabled={busy}
                      onClick={async () => {
                        setBusy(true);
                        await api.restoreProfile(p.id);
                        await load();
                        onChanged();
                        setBusy(false);
                      }}
                    >
                      {t("trash.restore")}
                    </button>
                    <button
                      className="danger"
                      disabled={busy}
                      onClick={async () => {
                        const go = await ask({
                          title: t("trash.purge"),
                          detail: t("trash.confirmPurge", { name: p.name }),
                          confirmLabel: t("ui.deleteForGood"),
                          danger: true,
                        });
                        if (go === null) return;
                        setBusy(true);
                        await api.purgeProfile(p.id);
                        await load();
                        onChanged();
                        setBusy(false);
                      }}
                    >
                      {t("trash.purge")}
                    </button>
                  </div>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}
