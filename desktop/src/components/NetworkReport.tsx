// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { api, type Diagnosis, type ProxySummary } from "../api";
import { useI18n } from "../i18n";

/** What the world sees when this proxy is used, and — when it cannot be used —
 *  which step failed, in a sentence that names the fix.
 *
 *  One component for two places: the row of a profile (what network is this
 *  profile on) and the proxy form (does this proxy work). Both used to get a
 *  yes/no; the no is where the time went, and AdsPower and Kameleo both ship a
 *  step-by-step tester for that reason (docs/12).
 *
 *  Codes come from agent/src/diagnose.rs and are translated here; the agent's
 *  own English sentence is kept as a fallback for a code this file has not
 *  learned yet, so an untranslated cause beats a blank one. */
export function NetworkReport({
  url,
  proxy,
  checkerUrl,
  onClose,
}: {
  /** The proxy URL when the interface has one (local mode, or the form). */
  url?: string;
  /** The stored proxy, when checking one that already exists — its id lets a
   *  team server open sealed credentials on our behalf. */
  proxy?: ProxySummary | null;
  checkerUrl?: string | null;
  onClose: () => void;
}) {
  const { t, say } = useI18n();
  const [report, setReport] = useState<Diagnosis | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setReport(null);
    setError(null);
    api
      .diagnoseProxy(url ?? "", checkerUrl ?? null, proxy?.id ?? null)
      .then((r) => !cancelled && setReport(r))
      .catch((e) => !cancelled && setError(say(e)));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [url, proxy?.id, checkerUrl]);

  const stepName = (s: string) => t(`net.step.${s}` as never);
  const code = (c: string | null | undefined, fallback: string) => {
    if (!c) return fallback;
    const key = `net.code.${c}`;
    const translated = t(key as never);
    return translated === key ? fallback : translated;
  };

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "88vh", width: 560 }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{t("net.title", { name: proxy?.name ?? proxy?.display ?? url ?? "" })}</h2>
        </div>
        <div className="form" style={{ paddingTop: "var(--s-5)", overflowY: "auto" }}>
          {!report && !error && <p className="hint">{t("net.running")}</p>}
          {error && <p className="error">{error}</p>}
          {report && (
            <>
              {report.ok && (
                <dl className="kv" style={{ marginBottom: "var(--s-4)" }}>
                  <dt>{t("net.ip")}</dt>
                  <dd className="mono">{report.exit.ip ?? "?"}</dd>
                  <dt>{t("net.where")}</dt>
                  <dd>{[report.exit.country, report.exit.region, report.exit.city].filter(Boolean).join(" / ") || "?"}</dd>
                  <dt>{t("net.network")}</dt>
                  <dd className="small">{report.exit.org ?? "?"}</dd>
                  <dt>{t("net.timezone")}</dt>
                  <dd>{report.exit.timezone ?? "?"}</dd>
                </dl>
              )}
              <ol className="reportSteps">
                {report.steps.map((s) => (
                  <li key={s.step} className={s.ok ? "ok" : "bad"}>
                    <span className="stepMark">{s.ok ? "✓" : "✕"}</span>
                    <span className="stepName">{stepName(s.step)}</span>
                    <span className="stepDetail">
                      {s.ok ? s.detail : code(s.code, s.detail)}
                      {s.ms !== null && s.ms !== undefined && <span className="muted"> · {s.ms} ms</span>}
                    </span>
                    {!s.ok && s.code && s.code !== "not_a_proxy" && (
                      <div className="hint">{s.detail}</div>
                    )}
                  </li>
                ))}
              </ol>
              {report.notes.length > 0 && (
                <div style={{ marginTop: "var(--s-4)" }}>
                  {report.notes.map((n) => (
                    <p key={n.code} className="hint">
                      {code(n.code, n.detail)}
                    </p>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
        <div className="modalFoot">
          <div className="spacer" />
          <button onClick={onClose}>{t("ui.close")}</button>
        </div>
      </div>
    </div>
  );
}
