import { useCallback, useEffect, useState } from "react";
import { api, type Profile } from "../api";
import { useI18n } from "../i18n";

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

  const load = useCallback(async () => {
    setRows(await api.trash());
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (rows.length === 0) {
    return (
      <div className="tableWrap">
        <p className="empty pad">{t("trash.empty")}</p>
      </div>
    );
  }

  return (
    <>
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
                        if (!confirm(t("trash.confirmPurge", { name: p.name }))) return;
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
