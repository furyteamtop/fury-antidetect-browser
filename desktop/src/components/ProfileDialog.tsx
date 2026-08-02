// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { useI18n } from "../i18n";
import { api, type LocalProxy, type Persona, type Preview, type Profile } from "../api";

const TABS = ["General", "Proxy", "Device", "Advanced"] as const;
const TAB_KEYS = {
  General: "pd.tabGeneral",
  Proxy: "pd.tabProxy",
  Device: "pd.tabDevice",
  Advanced: "pd.tabAdvanced",
} as const;
type Tab = (typeof TABS)[number];

/** Creating or editing a profile.
 *
 *  The layout is the argument. On the left is what you choose; on the right is
 *  what the browser will actually claim, recomputed as you type. Every
 *  anti-detect browser on the market lets you assemble a fingerprint from
 *  independent dropdowns and never shows you the result — which is how people
 *  end up with a macOS user agent reporting an NVIDIA renderer. That is not a
 *  weaker disguise; it is a signal, because no real machine looks like that.
 *
 *  So the device is chosen as a whole, from measured machines, and the panel on
 *  the right proves what it produced before anything is saved. */
export function ProfileDialog({
  projectId,
  editing,
  onClose,
  onSaved,
}: {
  /** Null when Profiles is unfiltered: a new profile then belongs to no
   *  project, which is a place it can live. It can be filed later. */
  projectId: string | null;
  editing: Profile | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useI18n();
  const [tab, setTab] = useState<Tab>("General");
  const [personas, setPersonas] = useState<Persona[]>([]);
  const [proxies, setProxies] = useState<LocalProxy[]>([]);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // "configure" edits the fields below and saves a proxy with the profile;
  // "saved" picks one that already exists. AdsPower's split, and it is the
  // right one: the first proxy anyone adds is added while making a profile, and
  // sending them elsewhere to do it loses whatever they had typed.
  const [proxyMode, setProxyMode] = useState<"configure" | "saved">(
    editing?.proxy ? "saved" : "configure",
  );
  const [pxKind, setPxKind] = useState("socks5");
  const [pxHost, setPxHost] = useState("");
  const [pxPort, setPxPort] = useState("");
  const [pxUser, setPxUser] = useState("");
  const [pxPass, setPxPass] = useState("");
  const [pxRotate, setPxRotate] = useState("");
  const [pxChecker, setPxChecker] = useState("");
  const [pxCheck, setPxCheck] = useState<{
    ok: boolean; error?: string; ip?: string; country?: string;
    city?: string; timezone?: string; ms?: number;
  } | null>(null);

  const [name, setName] = useState(editing?.name ?? "");
  const [tags, setTags] = useState((editing?.tags ?? []).join(", "));
  const [personaId, setPersonaId] = useState(editing?.persona_id ?? "");
  const [proxyId, setProxyId] = useState(editing?.proxy?.id ?? "");
  // Empty means "follow the proxy's exit", which is what the agent resolves at
  // launch. Pre-filling Europe/Berlin made every profile ever created claim
  // Berlin — including the ones going out through São Paulo — and made the
  // follow-the-exit path unreachable, because the field was never empty.
  const [timezone, setTimezone] = useState(editing?.timezone ?? "");
  const [languages, setLanguages] = useState((editing?.languages ?? []).join(", "));
  const [startUrls, setStartUrls] = useState("");
  const [notes, setNotes] = useState("");

  useEffect(() => {
    void api.personas().then((p) => {
      setPersonas(p);
      setPersonaId((current) => current || p[0]?.id || "");
    });
    void api.proxies().then(setProxies);
  }, []);

  // Recomputed on every change rather than on a "preview" button: a value you
  // have to ask for is a value nobody looks at.
  useEffect(() => {
    if (!personaId) return;
    let cancelled = false;
    void api
      .preview({
        persona_id: personaId,
        fp_seed: editing?.fp_seed ?? 0,
        // An empty field follows the exit, and the preview has to say what the
        // launch will claim rather than what the field contains. When the proxy
        // has been checked, that is its zone.
        timezone: timezone.trim() || pxCheck?.timezone || null,
        languages: languages.trim() ? splitList(languages) : null,
      })
      .then((p) => !cancelled && setPreview(p))
      .catch(() => !cancelled && setPreview(null));
    return () => {
      cancelled = true;
    };
  }, [personaId, timezone, languages, pxCheck?.timezone, editing?.fp_seed]);

  const problems = preview?.problems ?? [];

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      // A proxy typed inline is saved first, so the profile can point at it.
      let useProxyId = proxyId;
      if (proxyMode === "configure" && pxComplete) {
        const saved = await api.saveProxy({
          id: "",
          name: `${pxHost.trim()}:${pxPort}`,
          kind: pxKind,
          host: pxHost.trim(),
          port: Number(pxPort),
          username: pxUser || null,
          password: pxPass || null,
          last_country: pxCheck?.country ?? null,
          last_ip: pxCheck?.ip ?? null,
          rotate_url: pxRotate.trim() || null,
          checker_url: pxChecker.trim() || null,
        });
        useProxyId = saved.id;
      }

      await api.saveProfile({
        id: editing?.id ?? "",
        // An existing profile keeps where it is filed; only a new one takes
        // the project currently being viewed. Editing a profile from the flat
        // list would otherwise move it out of its project.
        project_id: editing ? editing.project_id : projectId,
        name: name.trim() || "Untitled",
        notes,
        tags: splitList(tags),
        persona_id: personaId,
        // Zero means "assign one": the seed is generated once, on creation, and
        // never moves afterwards. Changing it would give a warmed account a
        // different fingerprint, which is the one thing it must never do.
        fp_seed: editing?.fp_seed ?? 0,
        proxy: useProxyId ? { id: useProxyId } : null,
        timezone: timezone.trim() || null,
        languages: languages.trim() ? splitList(languages) : null,
        start_urls: splitList(startUrls, "\n"),
        last_opened_at: null,
      });
      onSaved();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const pxComplete = pxHost.trim() !== "" && Number(pxPort) > 0;
  const pxUrl = () => {
    const auth = pxUser ? `${encodeURIComponent(pxUser)}:${encodeURIComponent(pxPass)}@` : "";
    return `${pxKind}://${auth}${pxHost.trim()}:${Number(pxPort)}`;
  };

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{editing ? t("pd.edit") : t("pd.new")}</h2>
        </div>

        <div className="tabs" role="tablist">
          {TABS.map((tb) => (
            <button
              key={tb}
              className="tab"
              role="tab"
              aria-selected={tb === tab}
              onClick={() => setTab(tb)}
            >
              {t(TAB_KEYS[tb])}
            </button>
          ))}
        </div>

        <div className="modalBody">
          <div className="form">
            {tab === "General" && (
              <>
                <div className="field">
                  <label htmlFor="p-name">{t("pd.name")}</label>
                  <div>
                    <input
                      id="p-name"
                      value={name}
                      autoFocus
                      placeholder="Shop DE"
                      onChange={(e) => setName(e.target.value)}
                    />
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-tags">{t("pd.tags")}</label>
                  <div>
                    <input
                      id="p-tags"
                      value={tags}
                      placeholder="de, marketplace"
                      onChange={(e) => setTags(e.target.value)}
                    />
                    <p className="hint">{t("pd.tagsHint")}</p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-urls">{t("pd.startUrls")}</label>
                  <div>
                    <textarea
                      id="p-urls"
                      rows={3}
                      value={startUrls}
                      placeholder={"https://example.com\nhttps://another.example"}
                      onChange={(e) => setStartUrls(e.target.value)}
                    />
                    <p className="hint">{t("pd.startUrlsHint")}</p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-notes">{t("pd.notes")}</label>
                  <div>
                    <textarea
                      id="p-notes"
                      rows={3}
                      value={notes}
                      onChange={(e) => setNotes(e.target.value)}
                    />
                  </div>
                </div>
              </>
            )}

            {tab === "Proxy" && (
              <>
                <div className="field">
                  <label>{t("pd.proxy")}</label>
                  <div className="segmented">
                    <button
                      aria-pressed={proxyMode === "configure"}
                      onClick={() => setProxyMode("configure")}
                    >
                      {t("px.configure")}
                    </button>
                    <button
                      aria-pressed={proxyMode === "saved"}
                      onClick={() => setProxyMode("saved")}
                    >
                      {t("px.saved")}
                    </button>
                  </div>
                </div>

                {proxyMode === "saved" ? (
                  <div className="field">
                    <label htmlFor="p-proxy">{t("px.saved")}</label>
                    <div>
                      <select
                        id="p-proxy"
                        value={proxyId}
                        onChange={(e) => setProxyId(e.target.value)}
                      >
                        <option value="">{t("pd.proxyNone")}</option>
                        {proxies.map((p) => (
                          <option key={p.id} value={p.id}>
                            {p.name} · {p.kind}://{p.host}:{p.port}
                          </option>
                        ))}
                      </select>
                      {!proxyId && <p className="hint">{t("pd.proxyRequired")}</p>}
                    </div>
                  </div>
                ) : (
                  <>
                    <div className="field">
                      <label>{t("px.type")}</label>
                      <div className="segmented">
                        {["socks5", "http", "https"].map((k) => (
                          <button key={k} aria-pressed={pxKind === k} onClick={() => setPxKind(k)}>
                            {k}
                          </button>
                        ))}
                      </div>
                    </div>

                    <div className="field">
                      <label htmlFor="px-host">{t("px.address")}</label>
                      <div>
                        <div className="row">
                          <input
                            id="px-host"
                            value={pxHost}
                            placeholder="exit.provider.net"
                            onChange={(e) => setPxHost(e.target.value)}
                          />
                          <input
                            style={{ width: 92 }}
                            value={pxPort}
                            placeholder="1080"
                            inputMode="numeric"
                            onChange={(e) => setPxPort(e.target.value.replace(/\D/g, ""))}
                          />
                          <button
                            style={{ whiteSpace: "nowrap" }}
                            disabled={busy || !pxComplete}
                            onClick={async () => {
                              setBusy(true);
                              setPxCheck(null);
                              try {
                                // The inline fields, which carry a typed password — so the URL is
                                // complete and there is no stored proxy to open.
                                setPxCheck(await api.checkProxy(pxUrl(), pxChecker));
                              } finally {
                                setBusy(false);
                              }
                            }}
                          >
                            {busy ? t("px.checking") : t("px.checkButton")}
                          </button>
                        </div>
                        {pxCheck && (
                          <div
                            className={pxCheck.ok ? "verdict good" : "verdict bad"}
                            style={{ marginTop: "var(--s-2)" }}
                          >
                            {pxCheck.ok
                              ? [pxCheck.ip, pxCheck.country, pxCheck.city, pxCheck.timezone]
                                  .filter(Boolean)
                                  .join(" · ")
                              : pxCheck.error}
                          </div>
                        )}
                        {/* The exit's zone is what the profile has to agree
                            with, so the check offers it rather than leaving the
                            operator to copy it across two tabs. */}
                        {pxCheck?.ok && pxCheck.timezone && timezone && pxCheck.timezone !== timezone && (
                          <p className="hint">
                            {t("px.setTimezone", { tz: pxCheck.timezone })}{" "}
                            <button
                              className="linky"
                              style={{ fontSize: 12 }}
                              onClick={() => setTimezone(pxCheck.timezone!)}
                            >
                              →
                            </button>
                          </p>
                        )}
                      </div>
                    </div>

                    <div className="field">
                      <label htmlFor="px-user">{t("px.credentials")}</label>
                      <div className="row">
                        <input
                          id="px-user"
                          value={pxUser}
                          placeholder={t("px.user")}
                          autoComplete="off"
                          onChange={(e) => setPxUser(e.target.value)}
                        />
                        <input
                          value={pxPass}
                          placeholder={t("px.password")}
                          type="password"
                          autoComplete="off"
                          onChange={(e) => setPxPass(e.target.value)}
                        />
                      </div>
                    </div>

                    <div className="field">
                      <label htmlFor="px-rotate">{t("px.rotate")}</label>
                      <div>
                        <input
                          id="px-rotate"
                          value={pxRotate}
                          placeholder="https://provider.example/rotate?key=…"
                          autoComplete="off"
                          onChange={(e) => setPxRotate(e.target.value)}
                        />
                        <p className="hint">{t("px.rotateHint")}</p>
                      </div>
                    </div>

                    <div className="field">
                      <label htmlFor="px-checker">{t("px.checker")}</label>
                      <div>
                        <input
                          id="px-checker"
                          value={pxChecker}
                          placeholder={t("px.checkerDefault")}
                          autoComplete="off"
                          onChange={(e) => setPxChecker(e.target.value)}
                        />
                        <p className="hint">{t("px.checkerHint")}</p>
                      </div>
                    </div>
                  </>
                )}
              </>
            )}

            {tab === "Device" && (
              <>
                <div className="field">
                  <label>{t("pd.machine")}</label>
                  <div>
                    {personas.map((p) => (
                      <button
                        key={p.id}
                        className="personaCard"
                        aria-pressed={p.id === personaId}
                        onClick={() => setPersonaId(p.id)}
                      >
                        <div className="name">
                          {p.os} · {p.screen}
                        </div>
                        <div className="muted small ellipsis">{p.gpu}</div>
                        <div className="muted small">
                          {t("pd.share", { pct: (p.weight * 100).toFixed(1) })}
                          {p.source === "measured" ? ` · ${t("pd.measured")}` : ""}
                        </div>
                      </button>
                    ))}
                    <p className="hint">
                      {t("pd.machineHint")}
                    </p>
                  </div>
                </div>
              </>
            )}

            {tab === "Advanced" && (
              <>
                <div className="field">
                  <label htmlFor="p-tz">{t("pd.timezone")}</label>
                  <div>
                    <input
                      id="p-tz"
                      value={timezone}
                      onChange={(e) => setTimezone(e.target.value)}
                    />
                    <p className="hint">
                      {t("pd.timezoneHint")}
                    </p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-lang">{t("pd.languages")}</label>
                  <div>
                    <input
                      id="p-lang"
                      value={languages}
                      onChange={(e) => setLanguages(e.target.value)}
                    />
                    <p className="hint">
                      {t("pd.languagesHint")}
                    </p>
                  </div>
                </div>
                <div className="field">
                  <label>{t("pd.noise")}</label>
                  <div>
                    <div className="muted small">
                      {t("pd.noiseHint")}
                    </div>
                  </div>
                </div>
              </>
            )}
          </div>

          <aside className="overview">
            <h3>{t("pd.overview")}</h3>
            {preview ? (
              <>
                <dl className="kv">
                  <dt>{t("pd.ovPlatform")}</dt>
                  <dd>{preview.platform}</dd>
                  <dt>{t("pd.ovUserAgent")}</dt>
                  <dd className="mono small">{preview.user_agent}</dd>
                  <dt>{t("pd.ovScreen")}</dt>
                  <dd>{preview.screen}</dd>
                  <dt>{t("pd.ovGpu")}</dt>
                  <dd className="small">{preview.gpu_renderer}</dd>
                  <dt>{t("pd.ovCpuRam")}</dt>
                  <dd>
                    {t("pd.ovCores", { n: preview.hardware_concurrency, gb: preview.device_memory })}
                  </dd>
                  <dt>{t("pd.ovTimezone")}</dt>
                  <dd>{preview.timezone}</dd>
                  <dt>{t("pd.ovLanguages")}</dt>
                  <dd>{preview.languages.join(", ")}</dd>
                  <dt>{t("pd.ovClientHints")}</dt>
                  <dd>{preview.client_hints_platform}</dd>
                  <dt>{t("pd.ovFonts")}</dt>
                  <dd>{preview.fonts}</dd>
                  <dt>{t("pd.ovNoise")}</dt>
                  <dd>
                    {[
                      preview.noise.canvas && t("pd.noiseCanvas"),
                      preview.noise.audio && t("pd.noiseAudio"),
                      preview.noise.client_rects && t("pd.noiseGeometry"),
                    ]
                      .filter(Boolean)
                      .join(", ") || t("pd.noiseNone")}
                  </dd>
                </dl>

                <div style={{ marginTop: "var(--s-4)" }}>
                  {problems.length === 0 ? (
                    <div className="verdict good">
                      {t("pd.consistent")}
                    </div>
                  ) : (
                    <div className="verdict bad">
                      {problems.map((p) => (
                        <div key={p}>{p}</div>
                      ))}
                    </div>
                  )}
                </div>
              </>
            ) : (
              <p className="muted small">{t("pd.pickMachine")}</p>
            )}
          </aside>
        </div>

        <div className="modalFoot">
          {error && <span className="error">{error}</span>}
          <div className="spacer" />
          <button className="ghost" onClick={onClose}>
            {t("ui.cancel")}
          </button>
          <button
            className="primary"
            disabled={busy || !personaId || problems.length > 0}
            onClick={save}
          >
            {busy ? t("ui.saving") : editing ? t("ui.save") : t("ui.create")}
          </button>
        </div>
      </div>
    </div>
  );
}

function splitList(raw: string, sep = ","): string[] {
  return raw
    .split(sep)
    .map((s) => s.trim())
    .filter(Boolean);
}
