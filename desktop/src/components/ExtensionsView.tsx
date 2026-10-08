// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useCallback, useEffect, useRef, useState } from "react";
import { api, type CatalogueEntry, type ExtensionEverywhere, type Profile, type StoreInstallResult } from "../api";
import { useI18n } from "../i18n";

/** Every extension on this machine, which profiles carry it, and a catalogue
 *  to install from.
 *
 *  The per-profile dialog answers "what does this profile have"; this section
 *  answers the question an operator with forty profiles actually asks — "which
 *  of them have the wallet, and which are missing it" — and lets a .crx go into
 *  many at once. Proposed 12.09.2026 after the AdsPower audit (docs/12, 5.30).
 *
 *  The catalogue (5.31) is shared/extensions/catalogue.json: a short list of
 *  Web Store ids. Installing one fetches the package through the proxy of each
 *  profile it goes into and checks the key derives the id (docs/12, decision
 *  B) — the agent does both; this file only asks. Anything not in the list is
 *  reachable by pasting its id or store link. */

/** What the target picker is open for. */
type Pick = { mode: "file" } | { mode: "store"; id: string; name: string };

const ID_IN_TEXT = /[a-p]{32}/;

export function ExtensionsView({ profiles }: { profiles: Profile[] }) {
  const { t, say, resolved } = useI18n();
  const [rows, setRows] = useState<ExtensionEverywhere[] | null>(null);
  const [catalogue, setCatalogue] = useState<CatalogueEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Which profiles the next install goes into. Starts as every one it can go into:
  // "install everywhere" is the common case, unticking is the exception.
  //
  // Team profiles too, for whoever may edit them -- the rule the extensions
  // button on a profile's row already follows. This screen used to take local
  // profiles only, so a tester whose one profile was a team profile saw every
  // button greyed and no reason, and was told to close profiles he had never
  // opened (08.10.2026).
  const eligible = profiles.filter((p) => p.origin === "local" || p.permissions.includes("edit_profile"));
  const isTeam = (id: string) => profiles.find((p) => p.id === id)?.origin === "team";
  const [targets, setTargets] = useState<Set<string>>(() => new Set(eligible.map((p) => p.id)));
  const [picking, setPicking] = useState<Pick | null>(null);
  const [byId, setById] = useState("");
  const file = useRef<HTMLInputElement>(null);

  const load = useCallback(async () => {
    try {
      setRows(await api.allExtensionsWith(profiles.filter((p) => p.origin === "team").map((p) => ({ id: p.id, name: p.name }))));
      setError(null);
    } catch (e) {
      setError(say(e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    void load();
    api.extensionCatalogue().then(setCatalogue, (e) => setError(say(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [load]);

  /** The agent's reasons are stable names; the sentence is ours. */
  const reason = (r: string) => {
    if (r === "proxy_missing") return t("exv.reason.proxy_missing");
    if (r.startsWith("id_mismatch:")) return t("exv.reason.id_mismatch");
    if (r.startsWith("fetch:")) return t("exv.reason.fetch", { detail: r.slice(6) });
    return r;
  };

  const report = (r: StoreInstallResult | { extension: { name: string } | null; installed: string[]; skipped: { id: string; reason: string }[] }, fallback: string) => {
    const skippedOpen = r.skipped.filter((s) => s.reason === "open").length;
    const others = r.skipped.filter((s) => s.reason !== "open");
    setNote(
      [
        t("exv.installed", { name: r.extension?.name ?? fallback, n: r.installed.length }),
        skippedOpen > 0 ? t("exv.skippedOpen", { n: skippedOpen }) : "",
        others.length > 0 ? others.map((s) => `${nameOf(s.id)}: ${reason(s.reason)}`).join("; ") : "",
        "routes" in r && r.installed.length > 0 ? t("exv.fetchedVia", { n: r.routes }) : "",
        r.installed.some(isTeam) ? t("exv.teamNote") : "",
      ]
        .filter(Boolean)
        .join(" "),
    );
  };

  const installFile = async (f: File) => {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const b64 = await new Promise<string>((resolve, reject) => {
        const r = new FileReader();
        r.onerror = () => reject(r.error);
        r.onload = () => resolve(String(r.result).split(",", 2)[1] ?? "");
        r.readAsDataURL(f);
      });
      report(await api.installExtensionMany([...targets], b64), f.name);
      setPicking(null);
      await load();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
      if (file.current) file.current.value = "";
    }
  };

  const installStore = async (id: string, name: string) => {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const mine = [...targets].filter((x) => !isTeam(x));
      const team = [...targets].filter(isTeam);
      const a = mine.length > 0 ? await api.installExtensionFromStore(mine, id) : null;
      const b = team.length > 0 ? await api.installExtensionFromStoreTeam(team, id) : null;
      report(
        {
          extension: a?.extension ?? b?.extension ?? null,
          installed: [...(a?.installed ?? []), ...(b?.installed ?? [])],
          skipped: [...(a?.skipped ?? []), ...(b?.skipped ?? [])],
          routes: (a?.routes ?? 0) + (b?.installed.length ?? 0),
        },
        name,
      );
      setPicking(null);
      await load();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const nameOf = (id: string) => profiles.find((p) => p.id === id)?.name ?? id;
  const carriers = (extId: string) => rows?.find((x) => x.id === extId)?.profiles.length ?? 0;
  const targetsWithoutProxy = [...targets].filter((id) => {
    const p = profiles.find((x) => x.id === id);
    return !p?.proxy_id && !p?.proxy;
  }).length;
  const pastedId = byId.match(ID_IN_TEXT)?.[0] ?? null;

  return (
    <>
      <div className="toolbar">
        <button
          className="primary"
          disabled={busy || eligible.length === 0}
          onClick={() => setPicking((v) => (v?.mode === "file" ? null : { mode: "file" }))}
        >
          {t("exv.add")}
        </button>
        <div className="spacer" />
        <button className="ghost" onClick={() => void load()}>
          {t("bar.refresh")}
        </button>
      </div>

      {picking && (
        <div className="notice" style={{ display: "block" }}>
          <p style={{ margin: "0 0 var(--s-2)" }}>{t("exv.pickTargets", { n: targets.size })}</p>
          <div style={{ display: "flex", flexWrap: "wrap", gap: "var(--s-2) var(--s-4)", marginBottom: "var(--s-3)" }}>
            {eligible.map((p) => (
              <label key={p.id} className="row">
                <input
                  type="checkbox"
                  style={{ width: 14, height: 14, accentColor: "var(--accent)" }}
                  checked={targets.has(p.id)}
                  onChange={(e) =>
                    setTargets((cur) => {
                      const next = new Set(cur);
                      if (e.target.checked) next.add(p.id);
                      else next.delete(p.id);
                      return next;
                    })
                  }
                />
                <span>{p.name}</span>
                {p.origin === "team" && <span className="muted small">{t("exv.team")}</span>}
                {p.running && <span className="muted small">{t("exv.open")}</span>}
              </label>
            ))}
          </div>
          <div className="row">
            <button className="linky" onClick={() => setTargets(new Set(eligible.map((p) => p.id)))}>{t("exv.all")}</button>
            <button className="linky" onClick={() => setTargets(new Set())}>{t("exv.clear")}</button>
            <div className="spacer" />
            {picking.mode === "file" ? (
              <>
                <input
                  ref={file}
                  type="file"
                  accept=".crx"
                  style={{ display: "none" }}
                  onChange={(e) => {
                    const f = e.target.files?.[0];
                    if (f) void installFile(f);
                  }}
                />
                <button className="primary" disabled={busy || targets.size === 0} onClick={() => file.current?.click()}>
                  {busy ? t("ck.working") : t("exv.chooseFile", { n: targets.size })}
                </button>
              </>
            ) : (
              <button
                className="primary"
                disabled={busy || targets.size === 0}
                onClick={() => void installStore(picking.id, picking.name)}
              >
                {busy ? t("ck.working") : t("exv.storeTarget", { name: picking.name, n: targets.size })}
              </button>
            )}
            <button className="ghost" disabled={busy} onClick={() => setPicking(null)}>{t("ui.cancel")}</button>
          </div>
          {picking.mode === "store" && targetsWithoutProxy > 0 && (
            <p className="hint warn">{t("exv.noProxyWarn", { n: targetsWithoutProxy })}</p>
          )}
          {picking.mode === "file" && <p className="hint">{t("ext.hint")}</p>}
        </div>
      )}

      {eligible.length === 0 && <div className="notice">{t("exv.noTargets")}</div>}
      {note && <div className="notice">{note}</div>}
      {error && <div className="notice warnBar">{error}</div>}

      <h2 className="sectionTitle">{t("exv.catalogue")}</h2>
      <p className="hint" style={{ maxWidth: 720 }}>{t("exv.catalogueHint")}</p>
      {catalogue.length > 0 && (
        <table className="grid" style={{ marginBottom: "var(--s-4)" }}>
          <tbody>
            {catalogue.map((c) => {
              const n = carriers(c.id);
              return (
                <tr key={c.id}>
                  <td style={{ minWidth: 180 }}>
                    <div className="name">{c.name}</div>
                    <div className="muted small">
                      {/* A Tauri window has no tab to open; the click goes to the system browser. */}
                      <a
                        href={c.homepage}
                        target="_blank"
                        rel="noreferrer"
                        onClick={(e) => {
                          e.preventDefault();
                          void api.openUrl(c.homepage);
                        }}
                      >
                        {c.homepage.replace(/^https:\/\//, "")}
                      </a>{" "}
                      · {c.licence}
                    </div>
                  </td>
                  <td className="muted" style={{ maxWidth: 420 }}>{resolved === "ru" ? c.summary.ru : c.summary.en}</td>
                  <td className="muted small" style={{ whiteSpace: "nowrap" }}>
                    {n > 0 ? t("exv.inN", { n, m: eligible.length }) : t("exv.notInstalled")}
                  </td>
                  <td className="actions">
                    <button
                      className="ghost"
                      disabled={busy || eligible.length === 0}
                      onClick={() => setPicking({ mode: "store", id: c.id, name: c.name })}
                    >
                      {t("exv.installInto")}
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
      <div className="row" style={{ marginBottom: "var(--s-5)", maxWidth: 720 }}>
        <span className="muted small" style={{ whiteSpace: "nowrap" }}>{t("exv.byId")}</span>
        <input
          value={byId}
          spellCheck={false}
          placeholder="https://chromewebstore.google.com/detail/…/hlkenndednhfkekhgcdicdfddnkalmdm"
          style={{ fontFamily: "var(--mono)", fontSize: 12 }}
          onChange={(e) => setById(e.target.value)}
        />
        <button
          className="ghost"
          disabled={busy || !pastedId || eligible.length === 0}
          onClick={() => pastedId && setPicking({ mode: "store", id: pastedId, name: pastedId.slice(0, 8) + "…" })}
        >
          {t("exv.byIdGo")}
        </button>
      </div>

      <h2 className="sectionTitle">{t("exv.extension")}</h2>
      <div className="tableWrap">
        {rows === null ? (
          <p className="empty pad">{t("ext.loading")}</p>
        ) : rows.length === 0 ? (
          <p className="empty pad">{t("exv.none")}</p>
        ) : (
          <table className="grid">
            <thead>
              <tr>
                <th>{t("exv.extension")}</th>
                <th>{t("exv.inProfiles")}</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {rows.map((x) => (
                <tr key={x.id}>
                  <td>
                    <div className="name">
                      {x.name} <span className="muted">{x.version}</span>
                    </div>
                    <div className="muted small mono">{x.id}</div>
                  </td>
                  <td>
                    <div className="tags">
                      {x.profiles.map((p) => (
                        <span key={p.id} title={p.version !== x.version ? t("exv.older", { v: p.version }) : undefined}>
                          {nameOf(p.id)}
                          {p.version !== x.version && ` · ${p.version}`}
                        </span>
                      ))}
                    </div>
                    {eligible.length > x.profiles.length && (
                      <div className="muted small" style={{ marginTop: "var(--s-1)" }}>
                        {t("exv.missingFrom", { n: eligible.length - x.profiles.length })}
                      </div>
                    )}
                  </td>
                  <td className="actions">
                    <button
                      className="ghost danger"
                      disabled={busy}
                      onClick={async () => {
                        setBusy(true);
                        setError(null);
                        try {
                          for (const p of x.profiles) {
                            await api.removeExtension(p.id, x.id);
                          }
                          setNote(t("exv.removed", { name: x.name, n: x.profiles.length }));
                          await load();
                        } catch (e) {
                          setError(say(e));
                        } finally {
                          setBusy(false);
                        }
                      }}
                    >
                      {t("exv.removeEverywhere")}
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </>
  );
}
