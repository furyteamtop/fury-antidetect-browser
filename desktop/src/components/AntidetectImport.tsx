// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useMemo, useState } from "react";
import { api, type LocalProxy, type Persona, type Vendor, type VendorProfile } from "../api";
import { useI18n } from "../i18n";

/** Profiles and their cookies out of another anti-detect browser, through
 *  its own API (agent/src/import_antidetect/ says what is read and why).
 *
 *  Each profile is made the way CsvImport makes one: its proxy through the
 *  paste parser, deduplicated by address, then `saveProfile`, then the cookies
 *  through the ordinary import, which opens the profile and closes it again.
 *  The fingerprint does not travel: the new profile gets a machine of the
 *  same OS from the catalogue, and the dialog says so before anything runs.
 *
 *  The token lives in this component's state and nowhere else. Local mode
 *  only, for the reason CsvImport gives. */

type Row = {
  name: string;
  ok: boolean;
  cookies?: number;
  notes: string[];
};

const OS_PREFIX: Record<string, string> = { win: "Windows", mac: "macOS", android: "Android" };

/** What each source needs from the person. `local` is the address its API
 *  answers on when the app is running; null for a cloud API. */
const VENDORS: { id: Vendor; name: string; token: "required" | "optional" | "none"; local: string | null }[] = [
  { id: "gologin", name: "GoLogin", token: "required", local: null },
  { id: "dolphin", name: "Dolphin Anty", token: "required", local: null },
  { id: "adspower", name: "AdsPower", token: "optional", local: "http://127.0.0.1:50325" },
  { id: "undetectable", name: "Undetectable", token: "none", local: "http://127.0.0.1:25325" },
  { id: "kameleo", name: "Kameleo", token: "none", local: "http://localhost:5050" },
  { id: "vision", name: "Vision", token: "required", local: null },
];

export function AntidetectImport({
  projectId,
  onDone,
  onClose,
}: {
  projectId: string | null;
  onDone: (created: number) => void;
  onClose: () => void;
}) {
  const { t, say } = useI18n();
  const [vendor, setVendor] = useState<Vendor>("gologin");
  const [token, setToken] = useState("");
  const [base, setBase] = useState("");
  const [list, setList] = useState<VendorProfile[] | null>(null);
  const spec = VENDORS.find((v) => v.id === vendor)!;
  const baseArg = spec.local && base.trim() && base.trim() !== spec.local ? base.trim() : null;
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState("");
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [rows, setRows] = useState<Row[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!list) return [];
    if (!q) return list;
    return list.filter((p) => p.name.toLowerCase().includes(q) || p.tags.some((x) => x.toLowerCase().includes(q)));
  }, [list, filter]);

  const load = async () => {
    setBusy(true);
    setError(null);
    try {
      const got = await api.vendorList(vendor, token.trim(), baseArg);
      setList(got);
      setPicked(new Set(got.map((p) => p.id)));
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const run = async () => {
    if (!list) return;
    const todo = list.filter((p) => picked.has(p.id));
    setBusy(true);
    setError(null);
    setRows(null);
    try {
      const [proxies, personas] = await Promise.all([api.proxies(), api.personas()]);
      const byAddr = new Map<string, string>(proxies.map((p: LocalProxy) => [`${p.host}:${p.port}`, p.id]));

      // The same weighted spread as the batch dialog, narrowed to the OS the
      // GoLogin profile claimed: that is the part an account remembers.
      const pick = (os: string): string => {
        const prefix = OS_PREFIX[os];
        const pool: Persona[] = (prefix && personas.filter((p) => p.os.startsWith(prefix))) || [];
        const from = pool.length > 0 ? pool : personas;
        let r = Math.random() * from.reduce((s, p) => s + p.weight, 0);
        for (const p of from) {
          r -= p.weight;
          if (r <= 0) return p.id;
        }
        return from[from.length - 1]?.id ?? "";
      };

      const out: Row[] = [];
      let created = 0;
      setProgress({ done: 0, total: todo.length });
      for (let i = 0; i < todo.length; i++) {
        const g = todo[i];
        const row: Row = { name: g.name || g.id, ok: false, notes: [] };
        try {
          const d = await api.vendorProfile(vendor, token.trim(), baseArg, g);

          let proxy_id: string | null = null;
          if (d.proxy_line) {
            const imported = await api.importProxies(d.proxy_line, spec.name, "local");
            const s = imported.saved[0];
            if (s) {
              const key = `${s.host}:${s.port}`;
              const existing = byAddr.get(key);
              if (existing && existing !== s.id) {
                await api.deleteProxy(s.id, "local").catch(() => {});
                proxy_id = existing;
              } else {
                byAddr.set(key, s.id);
                proxy_id = s.id;
              }
            } else {
              row.notes.push(t("gl.proxyRefused", { error: imported.rejected[0]?.error ?? "" }));
            }
          } else if (d.proxy_note) {
            row.notes.push(d.proxy_note);
          }
          // Said whenever the catalogue has no device of that OS, Android
          // included until a phone has been captured (docs/18).
          const prefix = OS_PREFIX[g.os];
          if (g.os && !(prefix && personas.some((p) => p.os.startsWith(prefix)))) {
            row.notes.push(t("gl.osSubstituted", { os: g.os }));
          }

          const saved = await api.saveProfile({
            id: "",
            project_id: projectId,
            name: g.name || g.id,
            notes: g.notes,
            status: "",
            tags: g.tags,
            persona_id: pick(g.os),
            fp_seed: 0,
            proxy_id,
            timezone: null,
            languages: null,
            start_urls: d.start_url ? [d.start_url] : [],
            blocklists: [],
          }, "local");
          created++;
          row.ok = true;

          if (d.cookie_note) row.notes.push(d.cookie_note);
          if (d.cookies.length > 0) {
            try {
              const r = await api.importCookies(saved.id, d.cookies);
              row.cookies = r.imported;
            } catch (e) {
              row.notes.push(t("gl.cookiesFailed", { error: say(e) }));
            }
          } else {
            row.cookies = 0;
          }
        } catch (e) {
          row.notes.push(say(e));
        }
        out.push(row);
        setRows([...out]);
        setProgress({ done: i + 1, total: todo.length });
      }
      onDone(created);
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const toggle = (id: string) =>
    setPicked((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });
  const allShown = shown.length > 0 && shown.every((p) => picked.has(p.id));

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "90vh", width: 760 }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{t("gl.title", { vendor: spec.name })}</h2>
        </div>
        <div className="form" style={{ paddingTop: "var(--s-5)", overflowY: "auto" }}>
          <p className="hint">{t("gl.why")}</p>
          <p className="hint">{t("gl.notCarried")}</p>

          {!list && (
            <>
              <div className="field">
                <label htmlFor="gl-vendor">{t("gl.source")}</label>
                <div>
                  <select
                    id="gl-vendor"
                    value={vendor}
                    onChange={(e) => {
                      setVendor(e.target.value as Vendor);
                      setToken("");
                      setBase("");
                      setError(null);
                    }}
                  >
                    {VENDORS.map((v) => (
                      <option key={v.id} value={v.id}>{v.name}</option>
                    ))}
                  </select>
                  <p className="hint small">{t(`gl.how.${vendor}` as never)}</p>
                </div>
              </div>
              {spec.token !== "none" && (
                <div className="field">
                  <label htmlFor="gl-token">{spec.token === "optional" ? t("gl.tokenOptional") : t("gl.token")}</label>
                  <div>
                    <input
                      id="gl-token"
                      type="password"
                      autoComplete="off"
                      spellCheck={false}
                      value={token}
                      onChange={(e) => setToken(e.target.value)}
                      onKeyDown={(e) => e.key === "Enter" && !busy && (token.trim() || spec.token !== "required") && void load()}
                    />
                    <p className="hint small">{t("gl.tokenKept")}</p>
                  </div>
                </div>
              )}
              {spec.local && (
                <div className="field">
                  <label htmlFor="gl-base">{t("gl.base")}</label>
                  <div>
                    <input
                      id="gl-base"
                      spellCheck={false}
                      placeholder={spec.local}
                      value={base}
                      onChange={(e) => setBase(e.target.value)}
                    />
                    <p className="hint small">{t("gl.baseHint", { vendor: spec.name })}</p>
                  </div>
                </div>
              )}
            </>
          )}

          {list && !rows && (
            <div className="field">
              <label>{t("gl.found", { n: list.length, vendor: spec.name })}</label>
              <div>
                <input
                  className="search"
                  placeholder={t("bar.search")}
                  value={filter}
                  onChange={(e) => setFilter(e.target.value)}
                />
                <div className="tableWrap" style={{ maxHeight: 320, marginTop: "var(--s-2)" }}>
                  <table className="grid">
                    <thead>
                      <tr>
                        <th style={{ width: 28 }}>
                          <input
                            type="checkbox"
                            checked={allShown}
                            onChange={() =>
                              setPicked((s) => {
                                const n = new Set(s);
                                for (const p of shown) {
                                  if (allShown) n.delete(p.id);
                                  else n.add(p.id);
                                }
                                return n;
                              })
                            }
                          />
                        </th>
                        <th>{t("csv.f.name")}</th>
                        <th>{t("gl.os")}</th>
                        <th>{t("csv.f.proxy")}</th>
                        <th>{t("csv.f.tags")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {shown.map((p) => (
                        <tr key={p.id} onClick={() => toggle(p.id)} style={{ cursor: "pointer" }}>
                          <td>
                            <input type="checkbox" checked={picked.has(p.id)} onChange={() => toggle(p.id)} onClick={(e) => e.stopPropagation()} />
                          </td>
                          <td className="small">{p.name || p.id}</td>
                          <td className="small mono">{p.os}</td>
                          <td className="small mono">{p.proxy ?? "—"}</td>
                          <td className="small">{p.tags.join(", ")}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            </div>
          )}

          {progress && busy && <p className="hint">{t("csv.progress", { done: progress.done, total: progress.total })}</p>}
          {rows && (
            <div className="notice" style={{ display: "block" }}>
              {!busy && t("gl.done", { n: rows.filter((r) => r.ok).length, m: rows.length })}
              <ul className="small" style={{ margin: "var(--s-2) 0 0", paddingLeft: 18 }}>
                {rows.map((r, i) => (
                  <li key={i}>
                    <b>{r.name}</b>
                    {": "}
                    {r.ok ? t("gl.rowOk", { n: r.cookies ?? 0 }) : t("gl.rowFailed")}
                    {r.notes.length > 0 && <span className="muted"> · {r.notes.join(" · ")}</span>}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {error && <p className="error">{error}</p>}
        </div>
        <div className="modalFoot">
          <span className="muted small">{t("gl.slow")}</span>
          <div className="spacer" />
          <button className="ghost" disabled={busy} onClick={onClose}>{rows && !busy ? t("ui.close") : t("ui.cancel")}</button>
          {!list && (
            <button className="primary" disabled={busy || (spec.token === "required" && !token.trim())} onClick={() => void load()}>
              {busy ? t("bp.working") : t("gl.load")}
            </button>
          )}
          {list && !rows && (
            <button className="primary" disabled={busy || picked.size === 0} onClick={() => void run()}>
              {t("gl.go", { n: picked.size })}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
