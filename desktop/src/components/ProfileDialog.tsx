import { useEffect, useState } from "react";
import { api, type LocalProxy, type Persona, type Preview, type Profile } from "../api";

const TABS = ["General", "Proxy", "Device", "Advanced"] as const;
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
          <h2>{editing ? "Edit profile" : "New profile"}</h2>
        </div>

        <div className="tabs" role="tablist">
          {TABS.map((t) => (
            <button
              key={t}
              className="tab"
              role="tab"
              aria-selected={t === tab}
              onClick={() => setTab(t)}
            >
              {t}
            </button>
          ))}
        </div>

        <div className="modalBody">
          <div className="form">
            {tab === "General" && (
              <>
                <div className="field">
                  <label htmlFor="p-name">Name</label>
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
                  <label htmlFor="p-tags">Tags</label>
                  <div>
                    <input
                      id="p-tags"
                      value={tags}
                      placeholder="de, marketplace"
                      onChange={(e) => setTags(e.target.value)}
                    />
                    <p className="hint">Comma separated.</p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-urls">Open on start</label>
                  <div>
                    <textarea
                      id="p-urls"
                      rows={3}
                      value={startUrls}
                      placeholder={"https://example.com\nhttps://another.example"}
                      onChange={(e) => setStartUrls(e.target.value)}
                    />
                    <p className="hint">One per line.</p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-notes">Notes</label>
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
                  <label htmlFor="p-proxy">Proxy</label>
                  <div>
                    <select
                      id="p-proxy"
                      value={proxyId}
                      onChange={(e) => setProxyId(e.target.value)}
                    >
                      <option value="">— none —</option>
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
                        A profile without a proxy cannot be opened — everything the
                        browser does goes through one.
                      </p>
                    )}
                  </div>
                </div>
                {proxy && (
                  <div className="field">
                    <label>Exit</label>
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
                  <label>Machine</label>
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
                          {(p.weight * 100).toFixed(1)}% of real machines
                          {p.source === "measured" ? " · measured" : ""}
                        </div>
                      </button>
                    ))}
                    <p className="hint">
                      One real machine's measured configuration, taken whole. Picking
                      a user agent and a GPU separately is how profiles end up
                      describing devices that do not exist.
                    </p>
                  </div>
                </div>
              </>
            )}

            {tab === "Advanced" && (
              <>
                <div className="field">
                  <label htmlFor="p-tz">Time zone</label>
                  <div>
                    <input
                      id="p-tz"
                      value={timezone}
                      onChange={(e) => setTimezone(e.target.value)}
                    />
                    <p className="hint">
                      Must match where the proxy actually exits. A profile leaving in
                      Germany while reporting Asia/Tbilisi is the cheapest detection
                      there is.
                    </p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-lang">Languages</label>
                  <div>
                    <input
                      id="p-lang"
                      value={languages}
                      onChange={(e) => setLanguages(e.target.value)}
                    />
                    <p className="hint">
                      Most preferred first. Also becomes the Accept-Language header —
                      the two cannot disagree.
                    </p>
                  </div>
                </div>
                <div className="field">
                  <label>Noise</label>
                  <div>
                    <div className="muted small">
                      Canvas, audio and element geometry are perturbed with a seed of
                      this profile's own. There is no switch to turn it off: an
                      un-noised canvas is byte-identical to the host machine, which is
                      what makes several commercial browsers trivially linkable.
                    </div>
                  </div>
                </div>
              </>
            )}
          </div>

          <aside className="overview">
            <h3>What it will claim</h3>
            {preview ? (
              <>
                <dl className="kv">
                  <dt>Platform</dt>
                  <dd>{preview.platform}</dd>
                  <dt>User agent</dt>
                  <dd className="mono small">{preview.user_agent}</dd>
                  <dt>Screen</dt>
                  <dd>{preview.screen}</dd>
                  <dt>GPU</dt>
                  <dd className="small">{preview.gpu_renderer}</dd>
                  <dt>CPU · RAM</dt>
                  <dd>
                    {preview.hardware_concurrency} cores · {preview.device_memory} GB
                  </dd>
                  <dt>Time zone</dt>
                  <dd>{preview.timezone}</dd>
                  <dt>Languages</dt>
                  <dd>{preview.languages.join(", ")}</dd>
                  <dt>Client Hints</dt>
                  <dd>{preview.client_hints_platform}</dd>
                  <dt>Fonts</dt>
                  <dd>{preview.fonts}</dd>
                  <dt>Noise</dt>
                  <dd>
                    {[
                      preview.noise.canvas && "canvas",
                      preview.noise.audio && "audio",
                      preview.noise.client_rects && "geometry",
                    ]
                      .filter(Boolean)
                      .join(", ") || "none"}
                  </dd>
                </dl>

                <div style={{ marginTop: "var(--s-4)" }}>
                  {problems.length === 0 ? (
                    <div className="verdict good">
                      Consistent — nothing here contradicts anything else.
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
              <p className="muted small">Choose a machine to see what it reports.</p>
            )}
          </aside>
        </div>

        <div className="modalFoot">
          {error && <span className="error">{error}</span>}
          <div className="spacer" />
          <button className="ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            className="primary"
            disabled={busy || !personaId || problems.length > 0}
            onClick={save}
          >
            {busy ? "Saving…" : editing ? "Save" : "Create"}
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
