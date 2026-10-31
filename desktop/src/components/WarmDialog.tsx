// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { api, type Profile } from "../api";
import { useI18n } from "../i18n";
import { setStage } from "../status";

/** Warm a batch of profiles: visit a list of sites in each so the jar is not
 *  empty when the account first logs in. Now, once — or on a schedule, a few
 *  sites a day for a number of days (agent/src/farm.rs). The list is the
 *  operator's; the default is offered, not imposed. See agent/src/warm.rs. */
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
  const [mode, setMode] = useState<"now" | "farm">("now");
  const [urls, setUrls] = useState("");
  const [lo, setLo] = useState(8);
  const [hi, setHi] = useState(25);
  const [follow, setFollow] = useState(true);
  const [closeAfter, setCloseAfter] = useState(true);
  // The schedule.
  const [days, setDays] = useState(7);
  const [sitesLo, setSitesLo] = useState(5);
  const [sitesHi, setSitesHi] = useState(12);
  const [hourFrom, setHourFrom] = useState(10);
  const [hourTo, setHourTo] = useState(22);
  const [atOnce, setAtOnce] = useState(2);
  const [stage, setStageOn] = useState(true);
  const [planName, setPlanName] = useState("");
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
  const perPage = ((lo + hi) / 2 + 6) * (follow ? 2 : 1);
  const farmOk =
    days >= 1 && days <= 90 && sitesLo >= 1 && sitesHi >= sitesLo && sitesHi <= 100 &&
    hourFrom >= 0 && hourTo <= 24 && hourFrom < hourTo && atOnce >= 1 && atOnce <= 6;
  const num = (v: string) => (v === "" ? 0 : Math.floor(Number(v)));

  const startNow = async () => {
    const r = await api.warmStart(profiles.map((p) => ({ id: p.id, origin: p.origin })), {
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
  };

  const schedule = async () => {
    await api.farmCreate(
      {
        name: planName.trim(),
        urls: list,
        days,
        sites_per_day: [sitesLo, sitesHi],
        hours: [hourFrom, hourTo],
        dwell_seconds: [lo, hi],
        follow_link: follow,
        at_once: atOnce,
        set_stage: stage,
      },
      // The clock each profile keeps: its own, else its proxy's exit. The
      // agent looks again for a local one; a team one it cannot see.
      profiles.map((p) => ({ id: p.id, name: p.name, origin: p.origin, timezone: p.timezone ?? p.proxy?.last_timezone ?? null })),
    );
    // Only after the plan is in: a refused plan must not leave twenty
    // profiles marked as warming.
    if (stage) {
      await Promise.allSettled(profiles.filter((p) => (p.status ?? "") !== "warming").map((p) => setStage(p, "warming")));
    }
    // No note: the plan shows up in its own bar at once.
    onStarted(null);
  };

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "88vh", width: 620 }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{t("warm.title", { n: profiles.length })}</h2>
        </div>
        <div className="form" style={{ paddingTop: "var(--s-5)", overflowY: "auto" }}>
          <p className="hint">{t("warm.why")}</p>
          <div className="field">
            <label>{t("farm.when")}</label>
            <div>
              <div className="segmented">
                <button aria-pressed={mode === "now"} onClick={() => setMode("now")}>{t("farm.now")}</button>
                <button aria-pressed={mode === "farm"} onClick={() => setMode("farm")}>{t("farm.scheduled")}</button>
              </div>
              {mode === "farm" && <p className="hint">{t("farm.why")}</p>}
            </div>
          </div>
          {mode === "farm" && (
            <>
              <div className="field">
                <label htmlFor="f-days">{t("farm.days")}</label>
                <div className="row">
                  <input id="f-days" type="number" min={1} max={90} value={days} style={{ width: 80 }} onChange={(e) => setDays(num(e.target.value))} />
                </div>
              </div>
              <div className="field">
                <label>{t("farm.sitesPerDay")}</label>
                <div>
                  <div className="row">
                    <input type="number" min={1} max={100} value={sitesLo} style={{ width: 80 }} onChange={(e) => setSitesLo(num(e.target.value))} />
                    <span className="muted">—</span>
                    <input type="number" min={1} max={100} value={sitesHi} style={{ width: 80 }} onChange={(e) => setSitesHi(num(e.target.value))} />
                  </div>
                  <p className="hint">{t("farm.sitesHint")}</p>
                </div>
              </div>
              <div className="field">
                <label>{t("farm.hours")}</label>
                <div>
                  <div className="row">
                    <input type="number" min={0} max={23} value={hourFrom} style={{ width: 80 }} onChange={(e) => setHourFrom(num(e.target.value))} />
                    <span className="muted">—</span>
                    <input type="number" min={1} max={24} value={hourTo} style={{ width: 80 }} onChange={(e) => setHourTo(num(e.target.value))} />
                  </div>
                  <p className="hint">{t("farm.hoursHint")}</p>
                </div>
              </div>
              <div className="field">
                <label htmlFor="f-once">{t("farm.atOnce")}</label>
                <div>
                  <input id="f-once" type="number" min={1} max={6} value={atOnce} style={{ width: 80 }} onChange={(e) => setAtOnce(num(e.target.value))} />
                  <p className="hint">{t("farm.atOnceHint")}</p>
                </div>
              </div>
              <div className="field">
                <label htmlFor="f-name">{t("farm.name")}</label>
                <input id="f-name" value={planName} placeholder={t("farm.namePlaceholder")} onChange={(e) => setPlanName(e.target.value)} />
              </div>
            </>
          )}
          <div className="field">
            <label htmlFor="w-urls">{t("warm.urls")}</label>
            <div>
              <textarea
                id="w-urls"
                rows={mode === "farm" ? 6 : 9}
                value={urls}
                spellCheck={false}
                style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 12 }}
                onChange={(e) => setUrls(e.target.value)}
              />
              <p className="hint">{t(mode === "farm" ? "farm.urlsHint" : "warm.urlsHint", { n: list.length })}</p>
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
              {mode === "now" ? (
                <label className="row" style={{ gap: 6, marginTop: "var(--s-1)" }}>
                  <input type="checkbox" style={{ width: 14, height: 14, accentColor: "var(--accent)" }} checked={closeAfter} onChange={(e) => setCloseAfter(e.target.checked)} />
                  <span>{t("warm.closeAfter")}</span>
                </label>
              ) : (
                <label className="row" style={{ gap: 6, marginTop: "var(--s-1)" }}>
                  <input type="checkbox" style={{ width: 14, height: 14, accentColor: "var(--accent)" }} checked={stage} onChange={(e) => setStageOn(e.target.checked)} />
                  <span>{t("farm.stage")}</span>
                </label>
              )}
              <p className="hint">{t(mode === "farm" ? "farm.needs" : "warm.cost")}</p>
            </div>
          </div>
          {error && <p className="error">{error}</p>}
        </div>
        <div className="modalFoot">
          <span className="muted small">
            {mode === "now"
              ? t("warm.estimate", { min: Math.round((list.length * perPage) / 60) })
              : t("farm.estimate", { min: Math.max(1, Math.round((Math.min((sitesLo + sitesHi) / 2, list.length) * perPage) / 60)) })}
          </span>
          <div className="spacer" />
          <button className="ghost" onClick={onClose}>{t("ui.cancel")}</button>
          <button
            className="primary"
            disabled={busy || list.length === 0 || lo < 1 || hi < lo || (mode === "farm" && !farmOk)}
            onClick={async () => {
              setBusy(true);
              setError(null);
              try {
                await (mode === "now" ? startNow() : schedule());
                onClose();
              } catch (e) {
                setError(say(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            {t(mode === "now" ? "warm.start" : "farm.start", { n: profiles.length })}
          </button>
        </div>
      </div>
    </div>
  );
}
