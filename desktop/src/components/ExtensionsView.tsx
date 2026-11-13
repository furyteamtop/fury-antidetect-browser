// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { IconButton } from "./Icon";
import { useCallback, useEffect, useState } from "react";
import { api, type CatalogueEntry, type ExtensionEverywhere, type ExtensionPackage, type KeySource, type Profile, type StoreInstallResult } from "../api";
import { useI18n } from "../i18n";
import { keyNote, PackageButtons, packageError, skipReason } from "./ExtensionPackage";

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

/** What the target picker is open for. `copy` takes the extension from one
 *  profile together with its storage, so the copies are signed in. */
type Pick =
  | { mode: "file" }
  | { mode: "store"; id: string; name: string }
  | { mode: "copy"; id: string; name: string; from: string };

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
  const reason = (r: string) => skipReason(t, r);

  const report = (
    r:
      | StoreInstallResult
      | { extension: { id: string; name: string } | null; installed: string[]; skipped: { id: string; reason: string }[]; key_source?: KeySource },
    fallback: string,
  ) => {
    const skippedOpen = r.skipped.filter((s) => s.reason === "open").length;
    const others = r.skipped.filter((s) => s.reason !== "open");
    setNote(
      [
        t("exv.installed", { name: r.extension?.name ?? fallback, n: r.installed.length }),
        skippedOpen > 0 ? t("exv.skippedOpen", { n: skippedOpen }) : "",
        others.length > 0 ? others.map((s) => `${nameOf(s.id)}: ${reason(s.reason)}`).join("; ") : "",
        "routes" in r && r.installed.length > 0 ? t("exv.fetchedVia", { n: r.routes }) : "",
        r.installed.some(isTeam) ? t("exv.teamNote") : "",
        "key_source" in r ? keyNote(t, r.key_source, r.extension?.name ?? fallback, r.extension?.id) : "",
      ]
        .filter(Boolean)
        .join(" "),
    );
  };

  const installPackage = async (pkg: ExtensionPackage, label: string) => {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      report(await api.installExtensionMany([...targets], pkg), label);
      setPicking(null);
      await load();
    } catch (e) {
      setError(packageError(t, say, e));
    } finally {
      setBusy(false);
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

  const copyWithState = async (p: Extract<Pick, { mode: "copy" }>) => {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const r = await api.copyExtensionWithState(p.from, p.id, [...targets].filter((x) => x !== p.from));
      const skippedOpen = r.skipped.filter((s) => s.reason === "open").length;
      const others = r.skipped.filter((s) => s.reason !== "open");
      setNote(
        [
          t("exv.copied", { name: r.extension?.name ?? p.name, n: r.installed.length }),
          skippedOpen > 0 ? t("exv.skippedOpen", { n: skippedOpen }) : "",
          others.length > 0 ? others.map((s) => `${nameOf(s.id)}: ${reason(s.reason)}`).join("; ") : "",
          r.installed.some(isTeam) ? t("exv.teamNote") : "",
        ]
          .filter(Boolean)
          .join(" "),
      );
      setPicking(null);
      await load();
    } catch (e) {
      const text = say(e);
      setError(
        text.includes("source_open")
          ? t("exv.err.sourceOpen", { name: nameOf(p.from) })
          : text.includes("not_installed")
            ? t("exv.err.notInSource", { name: nameOf(p.from) })
            : text,
      );
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
          {t("exv.addAny")}
        </button>
        <div className="spacer" />
        <IconButton icon="refresh" label={t("bar.refresh")} onClick={() => void load()} />
      </div>

      {picking && (
        <div className="notice" style={{ display: "block" }}>
          {picking.mode === "copy" && (
            <div className="row" style={{ marginBottom: "var(--s-2)" }}>
              <span>{t("exv.copyFrom")}</span>
              <select
                value={picking.from}
                onChange={(e) => setPicking({ ...picking, from: e.target.value })}
                style={{ width: "auto" }}
              >
                {(rows?.find((x) => x.id === picking.id)?.profiles ?? []).map((p) => (
                  <option key={p.id} value={p.id}>{nameOf(p.id)}</option>
                ))}
              </select>
            </div>
          )}
          <p style={{ margin: "0 0 var(--s-2)" }}>
            {t("exv.pickTargets", { n: picking.mode === "copy" ? [...targets].filter((x) => x !== picking.from).length : targets.size })}
          </p>
          <div style={{ display: "flex", flexWrap: "wrap", gap: "var(--s-2) var(--s-4)", marginBottom: "var(--s-3)" }}>
            {eligible.filter((p) => picking.mode !== "copy" || p.id !== picking.from).map((p) => (
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
              <PackageButtons
                primary
                disabled={busy || targets.size === 0}
                fileLabel={busy ? t("ck.working") : t("exv.chooseFileAny", { n: targets.size })}
                folderLabel={t("exv.chooseFolder", { n: targets.size })}
                onPicked={(pkg, label) => void installPackage(pkg, label)}
                onError={(e) => setError(packageError(t, say, e))}
              />
            ) : picking.mode === "store" ? (
              <button
                className="primary"
                disabled={busy || targets.size === 0}
                onClick={() => void installStore(picking.id, picking.name)}
              >
                {busy ? t("ck.working") : t("exv.storeTarget", { name: picking.name, n: targets.size })}
              </button>
            ) : (
              <button
                className="primary"
                disabled={busy || [...targets].filter((x) => x !== picking.from).length === 0}
                onClick={() => void copyWithState(picking)}
              >
                {busy
                  ? t("ck.working")
                  : t("exv.copyTarget", { name: picking.name, n: [...targets].filter((x) => x !== picking.from).length })}
              </button>
            )}
            <button className="ghost" disabled={busy} onClick={() => setPicking(null)}>{t("ui.cancel")}</button>
          </div>
          {picking.mode === "store" && targetsWithoutProxy > 0 && (
            <p className="hint warn">{t("exv.noProxyWarn", { n: targetsWithoutProxy })}</p>
          )}
          {picking.mode === "file" && <p className="hint">{t("ext.hintAny")}</p>}
          {picking.mode === "copy" && <p className="hint" style={{ maxWidth: 720 }}>{t("exv.copyHint")}</p>}
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
                      className="ghost"
                      disabled={busy || eligible.length < 2}
                      title={t("exv.copyWithLoginTitle")}
                      onClick={() => {
                        // From a profile that is not open, if there is one: an
                        // open source is refused, and starting there is a click
                        // the operator would only have to undo.
                        const from = x.profiles.find((p) => !profiles.find((q) => q.id === p.id)?.running) ?? x.profiles[0];
                        setPicking({ mode: "copy", id: x.id, name: x.name, from: from.id });
                      }}
                    >
                      {t("exv.copyWithLogin")}
                    </button>
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
