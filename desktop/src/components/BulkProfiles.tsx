// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { api, type LocalProxy, type Profile } from "../api";
import { useI18n } from "../i18n";

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
  local,
  onClose,
  onDone,
}: {
  /** Present when copying an existing profile rather than making new ones. */
  cloneOf: Profile | null;
  projectId: string | null;
  /** A team profile must have a project and a proxy — the server refuses
   *  otherwise, and refusing here means one sentence instead of N identical
   *  failures. */
  local: boolean;
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
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [made, setMade] = useState<Made | null>(null);
  // Loaded here rather than passed in: the list is only needed while this
  // dialog is open, and threading it through the screen behind would make
  // every profile view refetch proxies it never shows.
  const [proxies, setProxies] = useState<LocalProxy[]>([]);
  useEffect(() => {
    if (cloning) return;
    void api.proxies().then(setProxies).catch(() => setProxies([]));
  }, [cloning]);

  const n = Number(count);
  const inRange = Number.isInteger(n) && n >= 1 && n <= 500;
  // On a team server the project is what carries access and the proxy is what
  // the browser goes through; the create endpoint requires both.
  const missing = cloning || local
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
          {!made && (
            <>
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
                          notes: "",
                          tags: tags
                            .split(",")
                            .map((x) => x.trim())
                            .filter(Boolean),
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
                          timezone: null,
                          languages: null,
                          start_urls: [],
                          last_opened_at: null,
                        }),
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
