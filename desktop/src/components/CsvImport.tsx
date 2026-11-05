// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useMemo, useRef, useState } from "react";
import { api, type LocalProxy } from "../api";
import { useI18n } from "../i18n";
import { hostOs } from "./ProfileDialog";
import {
  ACCOUNT_FIELDS,
  ACCOUNT_LINE,
  type Field,
  emailSite,
  isBarSeparated,
  looksLikeHeader,
  mapHeader,
  parseCookieCell,
  parseCsv,
  SECRET_FIELDS,
  splitMulti,
} from "../csv";

const ALL_FIELDS: Field[] = [
  "name", "proxy", "tags", "status", "notes", "start_urls", "timezone", "languages", "persona", ...ACCOUNT_FIELDS,
];

/** Profiles from a spreadsheet (docs/16 5.7). One row, one profile.
 *
 *  The column a proxy arrives in takes either the name of a proxy already
 *  here or a proxy line in any shape the paste dialog accepts — the same
 *  parser, so `host:port:user:pass` is read the same way in both places.
 *  Lines that are not yet proxies are created first, once each; two rows
 *  with the same line share one proxy, which is what the person meant.
 *
 *  A row without a persona is spread over the catalogue by how common each
 *  machine is, as the batch dialog does. Seeds are never in a CSV: each
 *  profile gets its own on creation, and a column for it would be a column
 *  for linking accounts.
 *
 *  A row can be an account too (`id|password|secret|cookies|token|email|…`, or
 *  the same as columns). What it logs in with goes where a login typed by hand
 *  goes, the profile's logins, and its cookies into the browser, not into
 *  notes: importing cookies opens the browser, which needs a proxy, so a row
 *  without one keeps them in its login and says so. Nothing is dropped.
 *
 *  Local mode only. A team profile's proxy carries sealed credentials on the
 *  server, and importing those is a different dialog with a different
 *  warning; this one says so rather than half-working. */
export function CsvImport({
  projectId,
  onDone,
  onClose,
}: {
  projectId: string | null;
  onDone: (created: number) => void;
  onClose: () => void;
}) {
  const { t, say } = useI18n();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [result, setResult] = useState<{ created: number; failed: { row: number; error: string }[] } | null>(null);
  const [overrides, setOverrides] = useState<Record<number, Field | null>>({});
  const [site, setSite] = useState("https://www.facebook.com");
  const [proxyLines, setProxyLines] = useState("");
  const [error, setError] = useState<string | null>(null);
  const file = useRef<HTMLInputElement>(null);

  const parsed = useMemo(() => {
    const rows = parseCsv(text);
    if (rows.length === 0) return null;
    const hasHeader = looksLikeHeader(rows[0]);
    const bars = !hasHeader && isBarSeparated(text);
    const read: (Field | null)[] = hasHeader
      ? mapHeader(rows[0])
      : rows[0].map((_, i) => (bars ? ACCOUNT_LINE[i] ?? null : i === 0 ? "name" : i === 1 ? "proxy" : null));
    const fields = read.map((f, i) => (i in overrides ? overrides[i] : f));
    const data = hasHeader ? rows.slice(1) : rows;
    return { hasHeader, bars, fields, data, header: hasHeader ? rows[0] : null };
  }, [text, overrides]);

  const accounts = !!parsed && parsed.fields.some((f) => f && ACCOUNT_FIELDS.includes(f));
  const extraProxies = useMemo(() => proxyLines.split(/\r?\n/).map((l) => l.trim()).filter(Boolean), [proxyLines]);

  const get = (row: string[], f: Field) => {
    const i = parsed?.fields.indexOf(f) ?? -1;
    return i >= 0 ? (row[i] ?? "").trim() : "";
  };

  /** A row's logins and cookies, once its profile exists. A failure here is the
   *  account's, not the profile's: it comes back as words beside a profile that
   *  was made, and what could not go in its place stays in the login's notes. */
  const saveAccount = async (row: string[], profileId: string, proxyId: string | null): Promise<string[]> => {
    const warnings: string[] = [];
    const notes: string[] = [];
    const token = get(row, "token");
    if (token) notes.push(`Access token: ${token}`);

    const cookies = get(row, "cookies");
    if (cookies) {
      const list = parseCookieCell(cookies, site, Math.floor(Date.now() / 1000));
      if (!list) {
        notes.push(`Cookies: ${cookies}`);
        warnings.push(t("csv.cookiesUnread"));
      } else if (!proxyId) {
        notes.push(`Cookies: ${cookies}`);
        warnings.push(t("csv.cookiesNoProxy"));
      } else {
        try {
          await api.importCookies(profileId, list);
        } catch (e) {
          notes.push(`Cookies: ${cookies}`);
          warnings.push(t("csv.cookiesFailed", { error: say(e) }));
        }
      }
    }

    const login = get(row, "login");
    const password = get(row, "password");
    const totp = get(row, "totp");
    if (login || password || totp || notes.length > 0) {
      const credential = {
        id: "",
        profile_id: profileId,
        label: site.trim().replace(/^https?:\/\/(www\.)?/i, "").replace(/\/.*$/, ""),
        site: site.trim(),
        username: login || null,
        password: password || null,
        totp: totp || null,
        notes: notes.join("\n"),
      };
      try {
        await api.saveCredential(credential);
      } catch (e) {
        // A seed that does not parse must not cost the password beside it.
        await api.saveCredential({ ...credential, totp: null, notes: [...notes, `2FA: ${totp}`].join("\n") });
        warnings.push(t("csv.totpFailed", { error: say(e) }));
      }
    }

    const email = get(row, "email");
    if (email) {
      const backup = get(row, "backup_email");
      await api.saveCredential({
        id: "",
        profile_id: profileId,
        label: email.slice(email.lastIndexOf("@") + 1),
        site: emailSite(email),
        username: email,
        password: get(row, "email_password") || null,
        totp: null,
        notes: backup ? `Backup email: ${backup}` : "",
      });
    }
    return warnings;
  };

  const run = async () => {
    if (!parsed) return;
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const [proxies, personas] = await Promise.all([api.proxies(), api.personas()]);
      const byName = new Map<string, LocalProxy>(proxies.map((p) => [p.name.trim().toLowerCase(), p]));
      const byAddr = new Map<string, LocalProxy>(proxies.map((p) => [`${p.host}:${p.port}`, p]));

      // Proxy lines that are neither a known name nor a known address are
      // created first, once each, through the same parser the paste dialog
      // uses. The result maps line number back to the id.
      // One proxy per account, in order. Never handed round: two accounts behind
      // one exit is what gets both of them closed, so rows past the end of the
      // list go without one and say so.
      const proxyCell = (r: string[], i: number) => get(r, "proxy") || (accounts ? extraProxies[i] ?? "" : "");
      const wanted = [...new Set(parsed.data.map(proxyCell).filter(Boolean))];
      const unknown = wanted.filter((w) => !byName.has(w.toLowerCase()));
      const lineToId = new Map<string, string>();
      const lineError = new Map<string, string>();
      if (unknown.length > 0) {
        const imported = await api.importProxies(unknown.join("\n"), t("csv.proxyPrefix"), "local");
        for (const s of imported.saved) {
          const line = unknown[s.line - 1];
          // The same address under a different spelling is the proxy already
          // here: use it, and drop the copy the import just made.
          const existing = byAddr.get(`${s.host}:${s.port}`);
          if (existing) await api.deleteProxy(s.id, "local").catch(() => {});
          lineToId.set(line, existing?.id ?? s.id);
        }
        for (const r of imported.rejected) lineError.set(unknown[r.line - 1], r.error);
      }
      const proxyIdFor = (cell: string): { id: string | null; error?: string } => {
        if (!cell) return { id: null };
        const named = byName.get(cell.toLowerCase());
        if (named) return { id: named.id };
        if (lineToId.has(cell)) return { id: lineToId.get(cell)! };
        return { id: null, error: lineError.get(cell) ?? t("csv.badProxy") };
      };

      // Weighted pick over the catalogue for rows that name no persona —
      // the same spread the batch dialog uses, done here because the rows
      // differ from one another and the agent's batch call takes one template.
      //
      // Of this computer's OS, as the batch dialog's are: a machine of another
      // OS is not hidden by the patches (agent: pick_weighted_on). This drew from
      // the whole catalogue until 0.5.0, and with phones in it would have made a
      // phone of a spreadsheet row.
      const known = new Set(personas.map((p) => p.id));
      const host = hostOs();
      const same = personas.filter((p) => p.os === host);
      const pool = same.length > 0 ? same : personas;
      const total = pool.reduce((s, p) => s + p.weight, 0);
      const pick = (): string => {
        let r = Math.random() * total;
        for (const p of pool) {
          r -= p.weight;
          if (r <= 0) return p.id;
        }
        return pool[pool.length - 1]?.id ?? "";
      };

      let created = 0;
      const failed: { row: number; error: string }[] = [];
      setProgress({ done: 0, total: parsed.data.length });
      for (let i = 0; i < parsed.data.length; i++) {
        const row = parsed.data[i];
        const rowNo = i + (parsed.hasHeader ? 2 : 1);
        try {
          const px = proxyIdFor(proxyCell(row, i));
          if (px.error) throw new Error(px.error);
          const personaCell = get(row, "persona");
          const persona_id = personaCell && known.has(personaCell) ? personaCell : pick();
          const languages = splitMulti(get(row, "languages"));
          const login = get(row, "login");
          const startUrls = splitMulti(get(row, "start_urls"));
          const saved = await api.saveProfile({
            id: "",
            project_id: projectId,
            name: get(row, "name") || login || t("csv.unnamed", { n: rowNo }),
            notes: get(row, "notes"),
            status: get(row, "status"),
            tags: splitMulti(get(row, "tags")),
            persona_id,
            fp_seed: 0,
            proxy_id: px.id,
            timezone: get(row, "timezone") || null,
            languages: languages.length > 0 ? languages : null,
            start_urls: startUrls.length > 0 || !accounts ? startUrls : [site.trim()].filter(Boolean),
            blocklists: [],
          }, "local");
          created++;
          if (accounts) {
            for (const warning of await saveAccount(row, saved.id, px.id)) failed.push({ row: rowNo, error: warning });
          }
        } catch (e) {
          failed.push({ row: rowNo, error: say(e) });
        }
        setProgress({ done: i + 1, total: parsed.data.length });
      }
      setResult({ created, failed });
      onDone(created);
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const recognised = parsed?.fields.filter(Boolean).length ?? 0;

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && !busy && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "90vh", width: 760 }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{t("csv.title")}</h2>
        </div>
        <div className="form" style={{ paddingTop: "var(--s-5)", overflowY: "auto" }}>
          <p className="hint">{t("csv.why")}</p>
          <p className="hint mono small" style={{ whiteSpace: "pre-wrap" }}>{t("csv.columns")}</p>
          <div className="field">
            <label htmlFor="csv-text">{t("csv.paste")}</label>
            <div>
              <textarea
                id="csv-text"
                rows={8}
                value={text}
                spellCheck={false}
                style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 12 }}
                placeholder={"name,proxy,tags,status\nShop 1,http://user:pass@1.2.3.4:8080,de;shop,warming\nShop 2,Residential DE,de,new"}
                onChange={(e) => {
                  setText(e.target.value);
                  setOverrides({});
                }}
              />
              <div className="row" style={{ marginTop: "var(--s-2)" }}>
                <input
                  ref={file}
                  type="file"
                  accept=".csv,.tsv,.txt,text/csv"
                  style={{ display: "none" }}
                  onChange={(e) => {
                    const f = e.target.files?.[0];
                    if (f) {
                      setOverrides({});
                      void f.text().then(setText);
                    }
                  }}
                />
                <button className="ghost" disabled={busy} onClick={() => file.current?.click()}>{t("csv.chooseFile")}</button>
              </div>
            </div>
          </div>
          {parsed && (
            <div className="field">
              <label>{t("csv.preview")}</label>
              <div>
                <p className="hint">
                  {parsed.hasHeader
                    ? t("csv.headerFound", { n: recognised, m: parsed.fields.length })
                    : parsed.bars
                      ? t("csv.barsFound")
                      : t("csv.noHeader")}
                  {" · "}
                  {t("csv.rows", { n: parsed.data.length })}
                </p>
                <div className="tableWrap" style={{ maxHeight: 220 }}>
                  <table className="grid">
                    <thead>
                      <tr>
                        {parsed.fields.map((f, i) => (
                          <th key={i} className={f ? undefined : "dim"}>
                            <select
                              aria-label={parsed.header?.[i] ?? t("csv.column", { n: i + 1 })}
                              value={f ?? ""}
                              onChange={(e) => setOverrides({ ...overrides, [i]: (e.target.value || null) as Field | null })}
                            >
                              <option value="">{t("csv.ignored")}</option>
                              {ALL_FIELDS.map((x) => (
                                <option key={x} value={x}>{t(`csv.f.${x}` as never)}</option>
                              ))}
                            </select>
                            {parsed.header && <div className="muted small mono">{parsed.header[i]}</div>}
                          </th>
                        ))}
                      </tr>
                    </thead>
                    <tbody>
                      {parsed.data.slice(0, 5).map((r, i) => (
                        <tr key={i}>
                          {parsed.fields.map((f, j) => (
                            <td key={j} className="small" style={{ maxWidth: 200, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                              {f && SECRET_FIELDS.includes(f) && r[j] ? "••••••" : r[j]}
                            </td>
                          ))}
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            </div>
          )}
          {accounts && (
            <>
              <div className="field">
                <label htmlFor="csv-site">{t("csv.site")}</label>
                <div>
                  <input id="csv-site" value={site} spellCheck={false} onChange={(e) => setSite(e.target.value)} />
                  <p className="hint">{t("csv.siteHint")}</p>
                </div>
              </div>
              {!parsed?.fields.includes("proxy") && (
                <div className="field">
                  <label htmlFor="csv-proxies">{t("csv.proxies")}</label>
                  <div>
                    <textarea
                      id="csv-proxies"
                      rows={4}
                      value={proxyLines}
                      spellCheck={false}
                      style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 12 }}
                      placeholder={"host:port:user:pass"}
                      onChange={(e) => setProxyLines(e.target.value)}
                    />
                    <p className="hint">
                      {t("csv.proxiesHint")}
                      {extraProxies.length < (parsed?.data.length ?? 0) && (
                        <> {t("csv.proxiesShort", { n: extraProxies.length, m: parsed?.data.length ?? 0 })}</>
                      )}
                    </p>
                  </div>
                </div>
              )}
            </>
          )}
          {progress && busy && <p className="hint">{t("csv.progress", { done: progress.done, total: progress.total })}</p>}
          {result && (
            <div className="notice" style={{ display: "block" }}>
              {t("csv.done", { n: result.created })}
              {result.failed.length > 0 && (
                <ul className="small" style={{ margin: "var(--s-2) 0 0", paddingLeft: 18 }}>
                  {result.failed.slice(0, 20).map((f) => (
                    <li key={f.row}>{t("csv.rowFailed", { row: f.row, error: f.error })}</li>
                  ))}
                  {result.failed.length > 20 && <li>…{result.failed.length - 20}</li>}
                </ul>
              )}
            </div>
          )}
          {error && <p className="error">{error}</p>}
        </div>
        <div className="modalFoot">
          <span className="muted small">{t("csv.seeds")}</span>
          <div className="spacer" />
          <button className="ghost" disabled={busy} onClick={onClose}>{result ? t("ui.close") : t("ui.cancel")}</button>
          {!result && (
            <button className="primary" disabled={busy || !parsed || parsed.data.length === 0} onClick={() => void run()}>
              {busy ? t("bp.working") : t("csv.go", { n: parsed?.data.length ?? 0 })}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
