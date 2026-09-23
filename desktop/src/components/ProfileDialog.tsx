// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState, useRef } from "react";
import { useI18n } from "../i18n";
import { spreadPastedProxy } from "../proxyLine";
import { SUGGESTED } from "../status";
import { api, type DomainList, type LocalProxy, type MachineOverrides, type Persona, type Preview, type Profile } from "../api";
import { Logins } from "./Logins";

// "Logins" only exists for a profile that has been saved: a login belongs to a
// profile id, and there is no id until the first save. Showing an empty tab on
// a new profile would invite somebody to type a password into something that
// cannot store it yet.
const TABS = ["General", "Proxy", "Device", "Machine", "Logins", "Advanced"] as const;
const TAB_KEYS = {
  General: "pd.tabGeneral",
  Proxy: "pd.tabProxy",
  Device: "pd.tabDevice",
  Machine: "pd.tabMachine",
  Logins: "pd.tabLogins",
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
/** Pick a machine the way the bulk path does: at random, but weighted.
 *
 * The list arrives sorted by how common each machine is, and the default used
 * to be `personas[0]` — the most common one. Which sounds right and is the one
 * thing this catalogue exists to prevent: every profile made from this dialog
 * came out on the same Iris Xe, so every account on the machine shared a GPU, a
 * screen, a font list and an audio latency. A separate proxy and a separate
 * canvas seed do not help when the hardware underneath is identical.
 *
 * Weighted rather than uniform, because the crowd is the point in the other
 * direction too: a profile claiming a machine one person in a thousand owns is
 * conspicuous on its own. Common machines come up more often, and no two
 * profiles have to agree.
 *
 * Mirrors catalogue::pick_weighted, including normalising against the
 * catalogue's own total — the weights are shares of the world's computers and
 * sum to well under one, so an un-normalised draw would fall past the end most
 * of the time and always land on the last entry.
 */
/** What Chrome sends for a country's install, most common first. Offered as a
 *  starting point for the languages box — the same strings the exit table in
 *  shared-rs/src/locale.rs produces, so picking one here claims exactly what a
 *  profile following that exit would have claimed. */
const LANGUAGE_SETS = [
  "en-US, en",
  "en-GB, en-US, en",
  "de-DE, de, en-US, en",
  "fr-FR, fr, en-US, en",
  "es-ES, es, en-US, en",
  "es-419, es, en-US, en",
  "it-IT, it, en-US, en",
  "pt-BR, pt, en-US, en",
  "pt-PT, pt, en-US, en",
  "nl-NL, nl, en-US, en",
  "pl-PL, pl, en-US, en",
  "ru-RU, ru, en-US, en",
  "uk-UA, uk, ru, en-US, en",
  "tr-TR, tr, en-US, en",
  "cs-CZ, cs, en-US, en",
  "ro-RO, ro, en-US, en",
  "sv-SE, sv, en-US, en",
  "ja-JP, ja, en-US, en",
  "ko-KR, ko, en-US, en",
  "zh-CN, zh, en-US, en",
];

/** "ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Direct3D11 vs_5_0 ps_5_0, D3D11)"
 *  is what the page sees; the card's name is what a person picks by. */
function gpuName(renderer: string): string {
  const metal = /Metal Renderer: ([^,]+)/.exec(renderer);
  if (metal) return metal[1];
  const d3d = /^ANGLE \([^,]+, (.+?) (?:Direct3D|\(0x)/.exec(renderer);
  return d3d ? d3d[1] : renderer;
}

function languageName(tag: string, uiLang: string): string {
  try {
    const name = new Intl.DisplayNames([uiLang], { type: "language" }).of(tag);
    return name && name !== tag ? `${tag} — ${name}` : tag;
  } catch {
    return tag;
  }
}

/** Every IANA zone the WebView knows, for the timezone box's suggestions. */
const TIME_ZONES: string[] = (() => {
  try {
    return (Intl as unknown as { supportedValuesOf(k: string): string[] }).supportedValuesOf("timeZone");
  } catch {
    return [];
  }
})();

/** Drops the keys left undefined, so an untouched profile stores `{}`. */
function compact(o: MachineOverrides): MachineOverrides {
  return Object.fromEntries(Object.entries(o).filter(([, v]) => v !== undefined)) as MachineOverrides;
}

function pickWeighted(list: Persona[]): string | undefined {
  if (list.length === 0) return undefined;
  const total = list.reduce((sum, p) => sum + (p.weight || 0), 0);
  if (total <= 0) return list[0]?.id;
  let cursor = Math.random() * total;
  for (const p of list) {
    cursor -= p.weight || 0;
    if (cursor < 0) return p.id;
  }
  return list[list.length - 1]?.id;
}

export function ProfileDialog({
  projectId,
  editing,
  local,
  onClose,
  onSaved,
}: {
  /** Null when Profiles is unfiltered: a new profile then belongs to no
   *  project, which is a place it can live. It can be filed later. */
  projectId: string | null;
  editing: Profile | null;
  /** Whether the shell is on its own or connected to a team server. A new
   *  profile is born wherever the shell is, and a team profile has to have a
   *  proxy -- so the dialog has to know, to say so before the button. */
  local: boolean;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t, say } = useI18n();
  // The interface language, for naming languages in it.
  const lang = document.documentElement.lang || "en";
  const [tab, setTab] = useState<Tab>("General");
  const [personas, setPersonas] = useState<Persona[]>([]);
  // The picked machine is chosen at random from twenty-six, so it is usually
  // NOT the one on screen. Selecting a card nobody can see reads as selecting
  // nothing — the list showed six unhighlighted rows while the panel on the
  // right described an RTX 2060 further down.
  const selectedCard = useRef<HTMLButtonElement | null>(null);
  const [proxies, setProxies] = useState<LocalProxy[]>([]);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Two clicks, not one: the first shows what a new seed costs, the second
  // does it. A confirmation dialog on top of a dialog reads as an error.
  const [reseedArmed, setReseedArmed] = useState(false);
  const [reseeded, setReseeded] = useState<number | null>(null);
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
  /** Permission for THIS profile to open with none. Off unless it was already
   *  on: the refusal is the default and stays the default. */
  const [allowNoProxy, setAllowNoProxy] = useState(editing?.allow_no_proxy ?? false);
  // Empty means "follow the proxy's exit", which is what the agent resolves at
  // launch. Pre-filling Europe/Berlin made every profile ever created claim
  // Berlin — including the ones going out through São Paulo — and made the
  // follow-the-exit path unreachable, because the field was never empty.
  const [timezone, setTimezone] = useState(editing?.timezone ?? "");
  const [languages, setLanguages] = useState((editing?.languages ?? []).join(", "));
  // Machine fields pinned by hand. Each key absent means "as the persona has
  // it"; the whole is validated with the persona by the preview, the save and
  // the launch alike.
  const [overrides, setOverrides] = useState<MachineOverrides>(editing?.overrides ?? {});
  const setOv = (patch: Partial<MachineOverrides>) => setOverrides((o) => compact({ ...o, ...patch }));
  // Typed as text and parsed, so a half-typed "52.5" is not thrown away while
  // it is being written. Only a whole pair reaches the overrides.
  const [geoText, setGeoText] = useState(
    editing?.overrides?.geolocation
      ? `${editing.overrides.geolocation.latitude}, ${editing.overrides.geolocation.longitude}`
      : "",
  );
  const geoParsed = (() => {
    const m = /^\s*(-?\d+(?:\.\d+)?)\s*[,; ]\s*(-?\d+(?:\.\d+)?)\s*$/.exec(geoText);
    return m ? { latitude: Number(m[1]), longitude: Number(m[2]) } : null;
  })();
  const geoBad = geoText.trim() !== "" && geoParsed === null;
  useEffect(() => {
    setOv({ geolocation: geoParsed ?? undefined });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [geoText]);
  // A team profile's machine is fixed on the server; the picker is shown but
  // not live, rather than live and silently not saved.
  const machineLocked = editing?.origin === "team" || editing?.origin === "shared";
  // From the row. Both used to start empty and be saved empty: an edit that
  // touched only the name erased the notes and the start URLs.
  const [startUrls, setStartUrls] = useState((editing?.start_urls ?? []).join("\n"));
  const [notes, setNotes] = useState(editing?.notes ?? "");
  const [status, setStatus] = useState(editing?.status ?? "");
  // Domain lists the relay applies to this profile. Local profiles only — a
  // team profile's record lives on the server, which does not carry them.
  const [lists, setLists] = useState<DomainList[]>([]);
  const [blocklists, setBlocklists] = useState<string[]>(editing?.blocklists ?? []);

  useEffect(() => {
    void api.personas().then((p) => {
      setPersonas(p);
      setPersonaId((current) => current || pickWeighted(p) || "");
    });
    void api.proxies().then(setProxies);
    void api.blocklists().then(setLists).catch(() => setLists([]));
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
        overrides,
      })
      .then((p) => !cancelled && setPreview(p))
      .catch(() => !cancelled && setPreview(null));
    return () => {
      cancelled = true;
    };
  }, [personaId, timezone, languages, overrides, pxCheck?.timezone, editing?.fp_seed]);

  useEffect(() => {
    // `nearest` rather than `center`: it scrolls the list only when the card is
    // actually out of view, so opening the tab on an already-visible choice
    // does not jump.
    selectedCard.current?.scrollIntoView({ block: "nearest" });
  }, [personas.length, personaId, tab]);

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
        status: status.trim(),
        tags: splitList(tags),
        persona_id: personaId,
        // Zero means "assign one": the seed is generated once, on creation, and
        // never moves afterwards. Changing it would give a warmed account a
        // different fingerprint, which is the one thing it must never do.
        fp_seed: editing?.fp_seed ?? 0,
        // The id, not a copy of the proxy. Sending `{proxy: {id}}` was refused
        // with `missing field \`name\`` — serde wants the whole struct and
        // upsert_profile reads nothing from it but the id — so saving a profile
        // with a proxy from this dialog had never worked, and the error named a
        // field the person had already filled in on another tab.
        proxy_id: useProxyId || null,
        timezone: timezone.trim() || null,
        languages: languages.trim() ? splitList(languages) : null,
        overrides,
        start_urls: splitList(startUrls, "\n"),
        // Only ever true for a profile that has no proxy. Leaving a stale
        // permission on a profile that was later given one would be a switch
        // nobody can see, waiting for the day the proxy is removed again.
        allow_no_proxy: !useProxyId && allowNoProxy,
        blocklists,
        last_opened_at: null,
      },
      // Where to save it. An existing profile goes back where it came from --
      // with both worlds in one list, the shell's mode no longer says which.
      // A new one has no origin yet and is born wherever the shell is
      // connected, which is what `undefined` means to the command.
      editing?.origin);
      onSaved();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const pxComplete = pxHost.trim() !== "" && Number(pxPort) > 0;
  // A team profile goes out through a proxy, always: the server refuses one
  // without, because launching it would send a colleague's traffic from their
  // own address. Said here, under the fields, with the button off, rather than
  // as an error after Create -- which is where it was said until 21.09.2026,
  // in English, to somebody whose interface was in Russian.
  const needsProxy = editing ? editing.origin === "team" : !local;
  const hasProxy = proxyMode === "saved" ? proxyId !== "" : pxComplete;
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
          <div className={tab === "Device" ? "form fill" : "form"}>
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
                  <label htmlFor="p-status">{t("pd.status")}</label>
                  <div>
                    <input
                      id="p-status"
                      list="p-status-options"
                      value={status}
                      placeholder={t("pd.statusPlaceholder")}
                      onChange={(e) => setStatus(e.target.value)}
                    />
                    <datalist id="p-status-options">
                      {SUGGESTED.map((s) => (
                        <option key={s} value={s}>{t(`status.${s}`)}</option>
                      ))}
                    </datalist>
                    <p className="hint">{t("pd.statusHint")}</p>
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
                {lists.length > 0 && (
                  <div className="field">
                    <label>{t("pd.domainLists")}</label>
                    <div>
                      {lists.map((l) => (
                        <label key={l.name} className="row" style={{ marginBottom: "var(--s-1)" }}>
                          <input
                            type="checkbox"
                            style={{ width: 14, height: 14, accentColor: "var(--accent)" }}
                            checked={blocklists.includes(l.name)}
                            onChange={(e) =>
                              setBlocklists((cur) =>
                                e.target.checked ? [...cur, l.name] : cur.filter((n) => n !== l.name),
                              )
                            }
                          />
                          {l.name}
                          <span className="muted small">
                            {l.allow_only ? t("pd.listAllowOnly", { n: l.domains }) : t("pd.listBlocks", { n: l.domains })}
                          </span>
                        </label>
                      ))}
                      <p className="hint">{t("pd.domainListsHint")}</p>
                    </div>
                  </div>
                )}
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

            {tab === "Logins" && (
              <div className="field">
                <div />
                <div>
                  {editing?.id ? (
                    <Logins profileId={editing.id} />
                  ) : (
                    <p className="hint">{t("pd.loginsAfterSave")}</p>
                  )}
                </div>
              </div>
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
                      {!proxyId && !needsProxy && (
                        <div style={{ marginTop: "var(--s-1)" }}>
                          {/* The refusal used to be absolute, which made the
                              application useless for the cases with no account
                              to protect: reading documentation, testing a
                              fingerprint, filling a profile in before its proxy
                              has been bought. It is a per-profile permission
                              rather than a setting because a throwaway profile
                              and a warmed account must not share a switch. */}
                          <label className="row" style={{ alignItems: "flex-start" }}>
                            <input
                              type="checkbox"
                              style={{ width: 14, height: 14, accentColor: "var(--accent)", marginTop: 3 }}
                              checked={allowNoProxy}
                              onChange={(e) => setAllowNoProxy(e.target.checked)}
                            />
                            <span>{t("pd.allowNoProxy")}</span>
                          </label>
                          <p className={allowNoProxy ? "hint warn" : "hint"}>
                            {allowNoProxy ? t("pd.allowNoProxyOn") : t("pd.proxyRequired")}
                          </p>
                        </div>
                      )}
                      {!proxyId && needsProxy && <p className="hint">{t("pd.proxyRequired")}</p>}
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
                            onPaste={spreadPastedProxy((p) => {
                              setPxHost(p.host);
                              if (p.port) setPxPort(p.port);
                              if (p.kind) setPxKind(p.kind);
                              if (p.username !== undefined) setPxUser(p.username);
                              if (p.password !== undefined) setPxPass(p.password);
                            })}
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
                        {needsProxy && !pxComplete && (
                          <p className="hint">{t("pd.proxyRequired")}</p>
                        )}
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
                <div className="field grow">
                  <label>{t("pd.machine")}</label>
                  <div className="personaList">
                    {personas.map((p) => (
                      <button
                        key={p.id}
                        ref={p.id === personaId ? selectedCard : undefined}
                        className="personaCard"
                        aria-pressed={p.id === personaId}
                        disabled={machineLocked && p.id !== personaId}
                        onClick={() => {
                          // Another OS takes a different list of screens and
                          // GPUs; a pinned one from the old list would only be
                          // refused, so it goes with the machine it came from.
                          const was = personas.find((x) => x.id === personaId)?.os.split(" ")[0];
                          if (was && was !== p.os.split(" ")[0]) setOv({ gpu: undefined, screen: undefined });
                          setPersonaId(p.id);
                        }}
                      >
                        <div className="name">
                          {p.os} · {p.screen}
                        </div>
                        <div className="muted small sub">
                          <span className="gpu">{p.gpu}</span>
                          <span className="share">
                            {t("pd.share", { pct: (p.weight * 100).toFixed(1) })}
                            {p.source === "measured" ? ` · ${t("pd.measured")}` : ""}
                          </span>
                        </div>
                      </button>
                    ))}
                    <p className="hint">
                      {machineLocked ? t("pd.machineLockedTeam") : t("pd.machineHint")}
                    </p>
                  </div>
                </div>
                {editing && editing.origin !== "team" && (
                  <div className="field" style={{ marginTop: "var(--s-4)" }}>
                    <label>{t("pd.seed")}</label>
                    <div>
                      <p className="hint">{t("pd.seedHint")}</p>
                      {!reseedArmed ? (
                        <button className="ghost" disabled={busy || editing.running} title={editing.running ? t("row.closeFirst") : undefined}
                          onClick={() => setReseedArmed(true)}>
                          {t("pd.reseed")}
                        </button>
                      ) : (
                        <div className="verdict bad">
                          <div>{t("pd.reseedWarn")}</div>
                          <div className="row" style={{ marginTop: "var(--s-2)" }}>
                            <button className="danger" disabled={busy}
                              onClick={async () => {
                                setBusy(true);
                                setError(null);
                                try {
                                  const r = await api.reseedProfile(editing.id, editing.origin);
                                  setReseeded(r.fp_seed);
                                  setReseedArmed(false);
                                  // The preview reads the seed from `editing`; a
                                  // re-roll is a new noise stream, so re-fetch.
                                  editing.fp_seed = r.fp_seed;
                                } catch (e) {
                                  setError(say(e));
                                } finally {
                                  setBusy(false);
                                }
                              }}>
                              {t("pd.reseedConfirm")}
                            </button>
                            <button className="ghost" onClick={() => setReseedArmed(false)}>{t("ui.cancel")}</button>
                          </div>
                        </div>
                      )}
                      {reseeded !== null && <p className="hint">{t("pd.reseeded")}</p>}
                    </div>
                  </div>
                )}
              </>
            )}

            {tab === "Machine" && (
              <>
                <p className="hint" style={{ marginTop: 0 }}>{t("pd.msIntro")}</p>
                <div className="field">
                  <label htmlFor="p-lang">{t("pd.languages")}</label>
                  <div>
                    <input
                      id="p-lang"
                      value={languages}
                      placeholder={t("pd.followExit")}
                      onChange={(e) => setLanguages(e.target.value)}
                    />
                    <select
                      value=""
                      style={{ marginTop: "var(--s-2)" }}
                      onChange={(e) => e.target.value && setLanguages(e.target.value)}
                    >
                      <option value="">{t("pd.langPresets")}</option>
                      {LANGUAGE_SETS.map((l) => (
                        <option key={l} value={l}>{languageName(l.split(",")[0], lang)} · {l}</option>
                      ))}
                    </select>
                    <p className="hint">{t("pd.languagesHint")}</p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-ui">{t("pd.uiLocale")}</label>
                  <div>
                    <select
                      id="p-ui"
                      value={overrides.ui_locale ?? ""}
                      onChange={(e) => setOv({ ui_locale: e.target.value || undefined })}
                    >
                      <option value="">
                        {t("pd.asMachine", { v: preview && !overrides.ui_locale ? preview.ui_locale : "…" })}
                      </option>
                      {(preview?.options?.ui_locales ?? []).map((l) => (
                        <option key={l} value={l}>{languageName(l, lang)}</option>
                      ))}
                    </select>
                    <p className="hint">{t("pd.uiLocaleHint")}</p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-tz">{t("pd.timezone")}</label>
                  <div>
                    <input
                      id="p-tz"
                      list="p-tz-list"
                      value={timezone}
                      placeholder={t("pd.followExit")}
                      onChange={(e) => setTimezone(e.target.value)}
                    />
                    <datalist id="p-tz-list">
                      {TIME_ZONES.map((z) => <option key={z} value={z} />)}
                    </datalist>
                    <p className="hint">{t("pd.timezoneHint")}</p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-geo">{t("pd.geo")}</label>
                  <div>
                    <input
                      id="p-geo"
                      value={geoText}
                      placeholder={t("pd.followExit")}
                      aria-invalid={geoBad}
                      onChange={(e) => setGeoText(e.target.value)}
                    />
                    <p className={geoBad ? "hint error" : "hint"}>{geoBad ? t("pd.geoBad") : t("pd.geoHint")}</p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-screen">{t("pd.screen")}</label>
                  <div>
                    <div className="row">
                      <select
                        id="p-screen"
                        value={overrides.screen ? `${overrides.screen.width}x${overrides.screen.height}` : ""}
                        onChange={(e) => {
                          if (!e.target.value) return setOv({ screen: undefined });
                          const [w, h] = e.target.value.split("x").map(Number);
                          setOv({
                            screen: {
                              width: w,
                              height: h,
                              device_pixel_ratio:
                                overrides.screen?.device_pixel_ratio ?? preview?.base?.device_pixel_ratio ?? 1,
                            },
                          });
                        }}
                      >
                        <option value="">
                          {t("pd.asMachine", { v: preview?.base ? preview.base.screen.join("×") : "…" })}
                        </option>
                        {(preview?.options?.screens ?? []).map(([w, h]) => (
                          <option key={`${w}x${h}`} value={`${w}x${h}`}>{w}×{h}</option>
                        ))}
                      </select>
                      <select
                        aria-label={t("pd.dpr")}
                        title={t("pd.dpr")}
                        value={overrides.screen?.device_pixel_ratio ?? preview?.base?.device_pixel_ratio ?? 1}
                        onChange={(e) => {
                          const dpr = Number(e.target.value);
                          const [w, h] = overrides.screen
                            ? [overrides.screen.width, overrides.screen.height]
                            : preview?.base?.screen ?? [0, 0];
                          setOv({ screen: { width: w, height: h, device_pixel_ratio: dpr } });
                        }}
                      >
                        {(preview?.options?.device_pixel_ratios ?? [1]).map((d) => (
                          <option key={d} value={d}>{Math.round(d * 100)}%</option>
                        ))}
                      </select>
                    </div>
                    <p className="hint">{t("pd.screenHint")}</p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-cores">{t("pd.cores")}</label>
                  <div>
                    <select
                      id="p-cores"
                      value={overrides.cores ?? ""}
                      onChange={(e) => setOv({ cores: e.target.value ? Number(e.target.value) : undefined })}
                    >
                      <option value="">{t("pd.asMachine", { v: preview?.base?.cores ?? "…" })}</option>
                      {(preview?.options?.cores ?? []).map((c) => <option key={c} value={c}>{c}</option>)}
                    </select>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-mem">{t("pd.memory")}</label>
                  <div>
                    <select
                      id="p-mem"
                      value={overrides.memory_gb ?? ""}
                      onChange={(e) => setOv({ memory_gb: e.target.value ? Number(e.target.value) : undefined })}
                    >
                      <option value="">{t("pd.asMachine", { v: preview?.base?.memory_gb ?? "…" })}</option>
                      {(preview?.options?.memory_gb ?? []).map((m) => <option key={m} value={m}>{m}</option>)}
                    </select>
                    <p className="hint">{t("pd.memoryHint")}</p>
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="p-gpu">{t("pd.gpu")}</label>
                  <div>
                    <select
                      id="p-gpu"
                      value={overrides.gpu ?? ""}
                      onChange={(e) => setOv({ gpu: e.target.value || undefined })}
                    >
                      <option value="">{t("pd.asMachine", { v: preview?.base ? gpuName(preview.base.gpu) : "…" })}</option>
                      {(preview?.options?.gpus ?? []).map((g) => (
                        <option key={g.id} value={g.id}>{gpuName(g.renderer)}</option>
                      ))}
                    </select>
                    <p className="hint">{t("pd.gpuHint")}</p>
                  </div>
                </div>
                {Object.keys(overrides).length > 0 && (
                  <div className="field">
                    <label />
                    <div>
                      <button className="ghost" onClick={() => { setOverrides({}); setGeoText(""); }}>
                        {t("pd.resetOverrides")}
                      </button>
                    </div>
                  </div>
                )}
              </>
            )}

            {tab === "Advanced" && (
              <>
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
                  <dt>{t("pd.ovChrome")}</dt>
                  <dd>{preview.chrome_version}</dd>
                  <dt>{t("pd.ovClientHints")}</dt>
                  <dd className="small">{preview.client_hints}</dd>
                  <dt>{t("pd.ovScreen")}</dt>
                  <dd>
                    {preview.screen}
                    <span className="muted small">
                      {" "}{t("pd.ovScreenDetail", { avail: preview.avail, dpr: preview.device_pixel_ratio, depth: preview.color_depth })}
                    </span>
                  </dd>
                  <dt>{t("pd.ovGpu")}</dt>
                  <dd className="small">
                    {preview.gpu_renderer}
                    <div className="muted">{preview.gpu_vendor} · {t("pd.ovWebglExt", { n: preview.webgl_extensions })}</div>
                  </dd>
                  <dt>{t("pd.ovWebgpu")}</dt>
                  <dd>{preview.webgpu ?? t("pd.ovFollowsHost")}</dd>
                  <dt>{t("pd.ovCpuRam")}</dt>
                  <dd>
                    {t("pd.ovCores", { n: preview.hardware_concurrency, gb: preview.device_memory })}
                    {preview.js_heap_gb !== null && (
                      <span className="muted small"> · {t("pd.ovHeap", { gb: preview.js_heap_gb })}</span>
                    )}
                  </dd>
                  <dt>{t("pd.ovTouch")}</dt>
                  <dd>{preview.max_touch_points}</dd>
                  <dt>{t("pd.ovTimezone")}</dt>
                  <dd>
                    {timezone.trim()
                      ? preview.timezone
                      : t("pd.ovFollowsExit", {
                          tz: pxCheck?.timezone
                            ?? proxies.find((x) => x.id === proxyId)?.last_timezone
                            ?? editing?.proxy?.last_timezone
                            ?? "?",
                        })}
                  </dd>
                  <dt>{t("pd.ovLanguages")}</dt>
                  <dd>
                    {preview.languages.join(", ")}
                    <span className="muted small"> · {t("pd.ovUiLocale", { l: preview.ui_locale })}</span>
                  </dd>
                  <dt>{t("pd.ovFonts")}</dt>
                  <dd>{preview.fonts}</dd>
                  <dt>{t("pd.ovAudio")}</dt>
                  <dd>{t("pd.ovAudioRate", { hz: preview.audio_sample_rate })}</dd>
                  <dt>{t("pd.ovVoices")}</dt>
                  <dd>{preview.voices ?? t("pd.ovFollowsHost")}</dd>
                  <dt>{t("pd.ovMedia")}</dt>
                  <dd>{preview.media_devices ?? t("pd.ovFollowsHost")}</dd>
                  <dt>{t("pd.ovWebrtc")}</dt>
                  <dd className="small">{preview.webrtc}</dd>
                  <dt>{t("pd.ovGeo")}</dt>
                  <dd className="small">
                    {preview.geolocation
                      ? t("pd.ovGeoPinned", { lat: preview.geolocation.latitude, lng: preview.geolocation.longitude })
                      : t("pd.ovGeoFollows")}
                  </dd>
                  <dt>{t("pd.ovSource")}</dt>
                  <dd className="small">
                    {preview.persona_source === "measured" ? t("pd.measured") : t("pd.derived")}
                    {" · "}{t("pd.ovWeight", { pct: (preview.persona_weight * 100).toFixed(1) })}
                  </dd>
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
            disabled={busy || !personaId || problems.length > 0 || geoBad || (needsProxy && !hasProxy)}
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
