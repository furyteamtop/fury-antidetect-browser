// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useCallback, useEffect, useState } from "react";
import { api, type Profile } from "../api";
import { useI18n } from "../i18n";
import { useAsk } from "./Ask";
import { withStepUp } from "../stepUp";
import { useWhenStyle, when } from "../when";

/** Deleted profiles, and the two things you can do with one.
 *
 *  Deleting hides rather than destroys because a profile holds a warmed
 *  account, and the cost of losing one to a stray click is not symmetric with
 *  the cost of keeping a row nobody wanted. Emptying is the destructive
 *  operation, so it is the one that names what goes: the browser data too. */
export function Trash({ onChanged }: { onChanged: () => void }) {
  const { t, say, resolved } = useI18n();
  const [whenStyle] = useWhenStyle();
  const [rows, setRows] = useState<Profile[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // What emptying the trash could not remove. Not `error`, which replaces
  // the whole list: the rows that stayed are the ones the person needs to see.
  const [notice, setNotice] = useState<string | null>(null);
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

  // Everything at once, one confirmation for all of it. Each row still goes
  // through the same purge as its own button, so a team profile this account
  // may not delete is refused by the server and the rest are not held up by
  // it: the ones that failed stay in the list, and the count says how many.
  // The server asks for a code before the first purge at most; it does not
  // ask again for ten minutes (docs/16 5.32), so the rest go straight through.
  const purgeAll = async () => {
    const go = await ask({
      title: t("trash.purgeAll"),
      detail: t("trash.confirmPurgeAll", { n: rows.length }),
      confirmLabel: t("ui.deleteForGood"),
      danger: true,
    });
    if (go === null) return;
    setBusy(true);
    setError(null);
    setNotice(null);
    let failed = 0;
    let firstError: unknown = null;
    try {
      for (const p of rows) {
        try {
          // `true` rather than the purge's own answer, so that null can only
          // mean the code was not given -- then stop rather than ask again
          // for every row.
          const done = await withStepUp(ask, t, async () => {
            await api.purgeProfile(p.id, p.origin);
            return true;
          });
          if (done === null) break;
        } catch (e) {
          failed += 1;
          firstError ??= e;
        }
      }
    } finally {
      await load();
      onChanged();
      setBusy(false);
    }
    if (failed > 0) {
      setNotice(t("trash.purgeAllFailed", { n: failed, reason: say(firstError) }));
    }
  };

  return (
    <>
      {dialog}
      <div className="trashHead">
        <p className="hint" style={{ margin: 0 }}>
          {t("trash.hint")}
        </p>
        <button className="danger" disabled={busy} onClick={() => void purgeAll()}>
          {t("trash.purgeAll")}
        </button>
      </div>
      {notice && (
        <div className="notice warnBar" role="status" style={{ marginBottom: "var(--s-3)" }}>
          {notice}
        </div>
      )}
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
                  {p.last_opened_at && (() => {
                    const w = when(p.last_opened_at, whenStyle, resolved);
                    return <span title={w.title}>{w.text}</span>;
                  })()}
                </td>
                <td className="actions">
                  <div>
                    <button
                      disabled={busy}
                      onClick={async () => {
                        setBusy(true);
                        await api.restoreProfile(p.id, p.origin);
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
                        setError(null);
                        try {
                          // The server may ask for a code first (docs/16 5.32).
                          const done = await withStepUp(ask, t, () => api.purgeProfile(p.id, p.origin));
                          if (done !== null) {
                            await load();
                            onChanged();
                          }
                        } catch (e) {
                          setError(say(e));
                        } finally {
                          setBusy(false);
                        }
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
