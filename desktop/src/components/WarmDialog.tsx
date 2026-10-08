// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { api, type Profile } from "../api";
import { useI18n } from "../i18n";

/** Warm a batch of profiles: visit a list of sites in each so the jar is not
 *  empty when the account first logs in. The list is the operator's; the
 *  default is offered, not imposed. See agent/src/warm.rs. */
export function WarmDialog({
  profiles,
  onStarted,
  onClose,
}: {
  profiles: Profile[];
  onStarted: (note: string | null) => void;
  onClose: () => void;
}) {
  const { t, say } = useI18n();
  const [urls, setUrls] = useState("");
  const [lo, setLo] = useState(8);
  const [hi, setHi] = useState(25);
  const [follow, setFollow] = useState(true);
  const [closeAfter, setCloseAfter] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    // The profiles' own start URLs first — the platform each exists for —
    // then the default list behind them.
    void api.warmDefaults().then((d) => {
      const own = [...new Set(profiles.flatMap((p) => p.start_urls ?? []))];
      setUrls([...own, ...d.urls].join("\n"));
    });
  }, [profiles]);

  const list = urls.split(/\r?\n/).map((s) => s.trim()).filter(Boolean);

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "88vh", width: 620 }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{t("warm.title", { n: profiles.length })}</h2>
        </div>
        <div className="form" style={{ paddingTop: "var(--s-5)", overflowY: "auto" }}>
          <p className="hint">{t("warm.why")}</p>
          <div className="field">
            <label htmlFor="w-urls">{t("warm.urls")}</label>
            <div>
              <textarea
                id="w-urls"
                rows={9}
                value={urls}
                spellCheck={false}
                style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 12 }}
                onChange={(e) => setUrls(e.target.value)}
              />
              <p className="hint">{t("warm.urlsHint", { n: list.length })}</p>
            </div>
          </div>
          <div className="field">
            <label>{t("warm.dwell")}</label>
            <div className="row">
              <input type="number" min={2} max={600} value={lo} style={{ width: 80 }} onChange={(e) => setLo(Number(e.target.value))} />
              <span className="muted">—</span>
              <input type="number" min={2} max={900} value={hi} style={{ width: 80 }} onChange={(e) => setHi(Number(e.target.value))} />
              <span className="muted">{t("warm.seconds")}</span>
            </div>
          </div>
          <div className="field">
            <label>{t("warm.options")}</label>
            <div>
              <label className="row" style={{ gap: 6 }}>
                <input type="checkbox" style={{ width: 14, height: 14, accentColor: "var(--accent)" }} checked={follow} onChange={(e) => setFollow(e.target.checked)} />
                <span>{t("warm.follow")}</span>
              </label>
              <label className="row" style={{ gap: 6, marginTop: "var(--s-1)" }}>
                <input type="checkbox" style={{ width: 14, height: 14, accentColor: "var(--accent)" }} checked={closeAfter} onChange={(e) => setCloseAfter(e.target.checked)} />
                <span>{t("warm.closeAfter")}</span>
              </label>
              <p className="hint">{t("warm.cost")}</p>
            </div>
          </div>
          {error && <p className="error">{error}</p>}
        </div>
        <div className="modalFoot">
          <span className="muted small">{t("warm.estimate", { min: Math.round((list.length * ((lo + hi) / 2 + 6) * (follow ? 2 : 1)) / 60) })}</span>
          <div className="spacer" />
          <button className="ghost" onClick={onClose}>{t("ui.cancel")}</button>
          <button
            className="primary"
            disabled={busy || list.length === 0 || lo < 1 || hi < lo}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                const r = await api.warmStart(profiles.map((p) => p.id), {
                  urls: list,
                  dwell_seconds: [lo, hi],
                  follow_link: follow,
                  close_after: closeAfter,
                });
                onStarted(
                  r.refused.length > 0
                    ? r.refused
                        .map((x) => {
                          const name = profiles.find((p) => p.id === x.id)?.name ?? x.id;
                          return x.reason === "open_without_cdp" ? t("mir.refusedOpen", { name }) : `${name}: ${say({ code: x.code, message: x.reason })}`;
                        })
                        .join(" ")
                    : null,
                );
                onClose();
              } catch (e) {
                setError(say(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            {t("warm.start", { n: profiles.length })}
          </button>
        </div>
      </div>
    </div>
  );
}
