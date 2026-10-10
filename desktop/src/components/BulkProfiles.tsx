// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { api, type LocalProxy, type Profile, type ProfileTemplate } from "../api";
import { useI18n } from "../i18n";
import { SUGGESTED } from "../status";

// `created` is only ever counted. Locally the agent returns ids; on a team
// server each entry is whatever the create endpoint answered, and pinning that
// shape here would make this file break whenever the server's does.
type Made = { created: unknown[]; failed?: { n: number; error: string }[] };

/** Making many profiles at once, and copying one.
 *
 *  Two shapes of the same screen, because the two are the same decision seen
 *  from either end: "give me twenty of these" starting from nothing, and "give
 *  me twenty more of that one". Splitting them into two dialogs would mean
 *  saying the same three things twice.
 *
 *  What is NOT offered here is a shared seed. Every profile made on this screen
 *  gets its own, and there is no control to change that: two profiles with one
 *  seed produce byte-identical canvas, audio and geometry readings, so the
 *  accounts are linked to each other for as long as they exist. That is the one
 *  thing bulk creation could get catastrophically wrong, so it is not a setting. */
export function BulkProfiles({
  cloneOf,
  projectId,
  origin,
  canChoose,
  onClose,
  onDone,
}: {
  /** Present when copying an existing profile rather than making new ones. */
  cloneOf: Profile | null;
  projectId: string | null;
  /** Where new profiles are made: the open project's world, or the shell's
   *  when none is open (see ProfileDialog). A team profile must have a
   *  project and a proxy — the server refuses otherwise, and refusing here
   *  means one sentence instead of N identical failures. */
  origin: "local" | "team";
  /** No project open and a server connected: the person picks the world. */
  canChoose: boolean;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t, say } = useI18n();
  const cloning = cloneOf !== null;

  const [count, setCount] = useState("10");
  const [pattern, setPattern] = useState(
    cloning ? `${cloneOf.name} {n}` : "Profile {n}",
  );
  const [proxyId, setProxyId] = useState("");
  const [tags, setTags] = useState("");
  // The rest of what a batch shares (5.6): stage, sites, languages, zone,
  // a note. Each is optional; empty means the profile's own default.
  const [status, setStatus] = useState("");
  const [startUrls, setStartUrls] = useState("");
  const [languages, setLanguages] = useState("");
  const [timezone, setTimezone] = useState("");
  const [notes, setNotes] = useState("");
  const [more, setMore] = useState(false);
  // Saved templates: the same answers under a name, kept by the agent.
  const [templates, setTemplates] = useState<ProfileTemplate[]>([]);
  const [templateName, setTemplateName] = useState("");
  const [saving, setSaving] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [made, setMade] = useState<Made | null>(null);
  // Loaded here rather than passed in: the list is only needed while this
  // dialog is open, and threading it through the screen behind would make
  // every profile view refetch proxies it never shows.
  const [proxies, setProxies] = useState<LocalProxy[]>([]);
  const [where, setWhere] = useState<"local" | "team">(origin);
  useEffect(() => {
    if (cloning) return;
    void api.templates().then(setTemplates).catch(() => setTemplates([]));
  }, [cloning]);
  // The proxies of the world the batch goes to, not of the shell's mode.
  useEffect(() => {
    if (cloning) return;
    void api.proxies(where).then(setProxies).catch(() => setProxies([]));
  }, [cloning, where]);

  const split = (s: string) => s.split(/[,;\n]/).map((x) => x.trim()).filter(Boolean);
  const current = (name: string): ProfileTemplate => ({
    name,
    pattern,
    proxy_id: proxyId,
    tags: split(tags),
    status: status.trim(),
    start_urls: split(startUrls),
    languages: split(languages),
    timezone: timezone.trim(),
    notes,
  });
  const apply = (tp: ProfileTemplate) => {
    setPattern(tp.pattern || "Profile {n}");
    setProxyId(tp.proxy_id ?? "");
    setTags(tp.tags.join(", "));
    setStatus(tp.status ?? "");
    setStartUrls(tp.start_urls.join("\n"));
    setLanguages(tp.languages.join(", "));
    setTimezone(tp.timezone ?? "");
    setNotes(tp.notes ?? "");
    setTemplateName(tp.name);
    if (tp.status || tp.start_urls.length || tp.languages.length || tp.timezone || tp.notes) setMore(true);
  };

  const n = Number(count);
  const inRange = Number.isInteger(n) && n >= 1 && n <= 500;
  // On a team server the project is what carries access and the proxy is what
  // the browser goes through; the create endpoint requires both.
  const missing = cloning || where === "local"
    ? null
    : !projectId
      ? t("bp.needProject")
      : !proxyId
        ? t("bp.needProxy")
        : null;
  const valid = inRange && missing === null;
  // {n} is substituted by the agent, so the preview has to do the same
  // substitution or it would promise a name nobody gets.
  const preview = (i: number) =>
    (pattern.includes("{n}") ? pattern : `${pattern} {n}`).replace("{n}", String(i));

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "88vh" }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{cloning ? t("bp.cloneTitle", { name: cloneOf.name }) : t("bp.title")}</h2>
        </div>

        <div className="form" style={{ paddingTop: "var(--s-5)", overflowY: "auto" }}>
          {/* "Why are there two different windows for a profile?" (a user,
              10.10.2026): the batch window says what it is for, and that the
              single one is the button on the left. */}
          {!made && !cloning && <p className="hint" style={{ marginTop: 0 }}>{t("bp.what")}</p>}
          {!made && (
            <>
              {!cloning && (templates.length > 0 || saving) && (
                <div className="field">
                  <label htmlFor="bp-template">{t("bp.template")}</label>
                  <div>
                    <div className="row">
                      <select
                        id="bp-template"
                        value={templateName}
                        onChange={(e) => {
                          const tp = templates.find((x) => x.name === e.target.value);
                          if (tp) apply(tp);
                          else setTemplateName("");
                        }}
                      >
                        <option value="">{t("bp.noTemplate")}</option>
                        {templates.map((x) => (
                          <option key={x.name} value={x.name}>{x.name}</option>
                        ))}
                      </select>
                      {templateName && templates.some((x) => x.name === templateName) && (
                        <button
                          className="ghost danger"
                          disabled={busy}
                          onClick={async () => {
                            await api.deleteTemplate(templateName).catch(() => {});
                            setTemplates(await api.templates().catch(() => []));
                            setTemplateName("");
                          }}
                        >
                          {t("row.delete")}
                        </button>
                      )}
                    </div>
                    <p className="hint">{t("bp.templateHint")}</p>
                  </div>
                </div>
              )}
              {canChoose && !cloning && (
                <div className="field">
                  <label>{t("pd.where")}</label>
                  <div>
                    <div className="segmented">
                      <button aria-pressed={where === "local"} onClick={() => { setWhere("local"); setProxyId(""); }}>
                        {t("pd.whereLocal")}
                      </button>
                      <button aria-pressed={where === "team"} onClick={() => { setWhere("team"); setProxyId(""); }}>
                        {t("pd.whereTeam")}
                      </button>
                    </div>
                    <p className="hint">{where === "local" ? t("pd.whereLocalHint") : t("pd.whereTeamHint")}</p>
                  </div>
                </div>
              )}
              <div className="field">
                <label htmlFor="bp-count">{t("bp.count")}</label>
                <div>
                  <input
                    id="bp-count"
                    value={count}
                    autoFocus
                    inputMode="numeric"
                    style={{ width: 92 }}
                    onChange={(e) => setCount(e.target.value)}
                  />
                  {!inRange && count !== "" && <p className="error">{t("bp.countRange")}</p>}
                </div>
              </div>

              <div className="field">
                <label htmlFor="bp-pattern">{t("bp.names")}</label>
                <div>
                  <input
                    id="bp-pattern"
                    value={pattern}
                    spellCheck={false}
                    onChange={(e) => setPattern(e.target.value)}
                  />
                  <p className="hint">
                    {t("bp.namesHint")}{" "}
                    {inRange && (
                      <span className="mono">
                        {preview(1)} … {preview(n)}
                      </span>
                    )}
                  </p>
                </div>
              </div>

              {!cloning && (
                <>
                  <div className="field">
                    <label htmlFor="bp-proxy">{t("bp.proxy")}</label>
                    <div>
                      <select
                        id="bp-proxy"
                        value={proxyId}
                        onChange={(e) => setProxyId(e.target.value)}
                      >
                        <option value="">{t("bp.noProxy")}</option>
                        {proxies.map((p) => (
                          <option key={p.id} value={p.id}>
                            {p.name}
                          </option>
                        ))}
                      </select>
                      <p className="hint">{t("bp.proxyHint")}</p>
                    </div>
                  </div>

                  <div className="field">
                    <label htmlFor="bp-tags">{t("bp.tags")}</label>
                    <div>
                      <input
                        id="bp-tags"
                        value={tags}
                        placeholder="etsy, batch-3"
                        onChange={(e) => setTags(e.target.value)}
                      />
                    </div>
                  </div>
                  {!more ? (
                    <div className="field">
                      <div />
                      <div>
                        <button className="linky" onClick={() => setMore(true)}>{t("bp.more")}</button>
                      </div>
                    </div>
                  ) : (
                    <>
                      <div className="field">
                        <label htmlFor="bp-status">{t("pd.status")}</label>
                        <div>
                          <input id="bp-status" list="bp-status-options" value={status} placeholder={t("pd.statusPlaceholder")} onChange={(e) => setStatus(e.target.value)} />
                          <datalist id="bp-status-options">
                            {SUGGESTED.map((x) => <option key={x} value={x}>{t(`status.${x}`)}</option>)}
                          </datalist>
                        </div>
                      </div>
                      <div className="field">
                        <label htmlFor="bp-urls">{t("bp.startUrls")}</label>
                        <div>
                          <textarea id="bp-urls" rows={2} value={startUrls} spellCheck={false} placeholder={"https://www.facebook.com/"} onChange={(e) => setStartUrls(e.target.value)} />
                        </div>
                      </div>
                      <div className="field">
                        <label htmlFor="bp-langs">{t("bp.languages")}</label>
                        <div>
                          <input id="bp-langs" value={languages} spellCheck={false} placeholder="de-DE, de, en" onChange={(e) => setLanguages(e.target.value)} />
                          <p className="hint">{t("bp.followExit")}</p>
                        </div>
                      </div>
                      <div className="field">
                        <label htmlFor="bp-tz">{t("bp.timezone")}</label>
                        <div>
                          <input id="bp-tz" value={timezone} spellCheck={false} placeholder="Europe/Berlin" onChange={(e) => setTimezone(e.target.value)} />
                        </div>
                      </div>
                      <div className="field">
                        <label htmlFor="bp-notes">{t("pd.notes")}</label>
                        <div>
                          <textarea id="bp-notes" rows={2} value={notes} onChange={(e) => setNotes(e.target.value)} />
                        </div>
                      </div>
                    </>
                  )}
                  {/* Save these answers under a name, for next time. */}
                  <div className="field">
                    <div />
                    <div>
                      {!saving ? (
                        <button className="linky" onClick={() => setSaving(true)}>{t("bp.saveTemplate")}</button>
                      ) : (
                        <div className="row">
                          <input
                            value={templateName}
                            placeholder={t("bp.templateName")}
                            style={{ width: 220 }}
                            onChange={(e) => setTemplateName(e.target.value)}
                          />
                          <button
                            className="primary"
                            disabled={busy || !templateName.trim()}
                            onClick={async () => {
                              try {
                                await api.saveTemplate(current(templateName.trim()));
                                setTemplates(await api.templates());
                                setSaving(false);
                              } catch (e) {
                                setError(say(e));
                              }
                            }}
                          >
                            {t("set.listSave")}
                          </button>
                          <button className="ghost" onClick={() => setSaving(false)}>{t("ui.cancel")}</button>
                        </div>
                      )}
                    </div>
                  </div>
                </>
              )}

              {/* The device, said rather than chosen. Not a control, because
                  the useful answer is always "spread them", and the harmful
                  one — put two hundred accounts on one unusual machine — is
                  the one a dropdown would make easy. */}
              <p className="hint">
                {cloning ? t("bp.cloneKeeps") : t("bp.personas")}
              </p>
              <p className="hint">{t("bp.seeds")}</p>
              {missing && <p className="error">{missing}</p>}
            </>
          )}

          {made && (
            <>
              <p>{t("bp.made", { n: made.created.length })}</p>
              {(made.failed?.length ?? 0) > 0 && (
                <div className="settingsGroup">
                  <h2>{t("bp.failed", { n: made.failed?.length ?? 0 })}</h2>
                  <ul className="hint" style={{ paddingLeft: "1.2em", lineHeight: 1.7 }}>
                    {(made.failed ?? []).map((f) => (
                      <li key={f.n}>
                        {preview(f.n)}: {f.error}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </>
          )}

          {error && <p className="error">{error}</p>}
        </div>

        <div className="modalFoot">
          <div className="spacer" />
          {made ? (
            <button
              className="primary"
              onClick={() => {
                onDone();
                onClose();
              }}
            >
              {t("ui.done")}
            </button>
          ) : (
            <>
              <button onClick={onClose}>{t("ui.cancel")}</button>
              <button
                className="primary"
                disabled={busy || !valid}
                onClick={async () => {
                  setBusy(true);
                  setError(null);
                  try {
                    if (cloning) {
                      setMade(await api.cloneProfile(cloneOf.id, n, pattern, cloneOf.origin));
                    } else {
                      setMade(
                        await api.createProfiles(n, pattern, {
                          id: "",
                          project_id: projectId,
                          name: "",
                          notes,
                          status: status.trim(),
                          tags: split(tags),
                          // Empty means "spread them over the catalogue by how
                          // common each machine is" — see the agent.
                          persona_id: "",
                          fp_seed: 0,
                          proxy: proxyId
                            ? (proxies.find((p) => p.id === proxyId) ?? null)
                            : null,
                          // Absent means the profile follows its exit, which is
                          // the better default and the one the rest of the app
                          // now uses.
                          timezone: timezone.trim() || null,
                          languages: split(languages).length > 0 ? split(languages) : null,
                          start_urls: split(startUrls),
                          last_opened_at: null,
                        }, where),
                      );
                    }
                  } catch (e) {
                    setError(say(e));
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {busy
                  ? t("bp.working")
                  : cloning
                    ? t("bp.cloneGo", { n })
                    : t("bp.go", { n })}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
