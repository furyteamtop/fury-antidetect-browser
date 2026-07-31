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
  projectId: string;
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

  const [name, setName] = useState(editing?.name ?? "");
  const [tags, setTags] = useState((editing?.tags ?? []).join(", "));
  const [personaId, setPersonaId] = useState(editing?.persona_id ?? "");
  const [proxyId, setProxyId] = useState(editing?.proxy?.id ?? "");
  const [timezone, setTimezone] = useState("Europe/Berlin");
  const [languages, setLanguages] = useState("de-DE, de, en-US, en");
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
        timezone,
        languages: splitList(languages),
      })
      .then((p) => !cancelled && setPreview(p))
      .catch(() => !cancelled && setPreview(null));
    return () => {
      cancelled = true;
    };
  }, [personaId, timezone, languages, editing?.fp_seed]);

  const problems = preview?.problems ?? [];
  const proxy = proxies.find((p) => p.id === proxyId) ?? null;

  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.saveProfile({
        id: editing?.id ?? "",
        project_id: projectId,
        name: name.trim() || "Untitled",
        notes,
        tags: splitList(tags),
        persona_id: personaId,
        // Zero means "assign one": the seed is generated once, on creation, and
        // never moves afterwards. Changing it would give a warmed account a
        // different fingerprint, which is the one thing it must never do.
        fp_seed: editing?.fp_seed ?? 0,
        proxy: proxyId ? { id: proxyId } : null,
        timezone,
        languages: splitList(languages),
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
                  <label htmlFor="p-proxy">{t("pd.proxy")}</label>
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
                    {/* Not a recommendation. The agent refuses to launch without
                        one, because the core is started pointing at a relay and
                        traffic would otherwise leave from this machine's own
                        address. */}
                    {!proxyId && (
                      <p className="hint">
                        {t("pd.proxyRequired")}
                      </p>
                    )}
                  </div>
                </div>
                {proxy && (
                  <div className="field">
                    <label>{t("pd.exit")}</label>
                    <div className="mono muted">
                      {proxy.host}:{proxy.port}
                      {proxy.last_country ? ` · ${proxy.last_country}` : ""}
                    </div>
                  </div>
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
