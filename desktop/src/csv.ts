// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

/** An account seller's line: `id|password|secret|cookies|…`. Three bars or more
 *  in the first line and it is one of these, whatever else the line holds: its
 *  cookies carry more semicolons than it has bars, so counting would choose
 *  wrong, and a quote inside them means nothing. One or two bars are a tag or a
 *  cell in an ordinary spreadsheet. */
export function isBarSeparated(text: string): boolean {
  const first = text.split(/\r?\n/).find((l) => l.trim() !== "") ?? "";
  return first.split("|").length >= 4;
}

/** A CSV reader that copes with what spreadsheets export: quoted fields with
 *  commas and newlines inside, doubled quotes, a comma or semicolon or tab as
 *  the separator (chosen by counting in the first line), CRLF. Bar-separated
 *  account lines are plain: one line, one row, no quoting. */
export function parseCsv(text: string): string[][] {
  if (isBarSeparated(text)) {
    return text
      .split(/\r?\n/)
      .filter((l) => l.trim() !== "")
      .map((l) => l.split("|").map((c) => c.trim()));
  }
  const first = text.split(/\r?\n/, 1)[0] ?? "";
  const sep = [",", ";", "\t"].map((s) => [s, first.split(s).length] as const).sort((a, b) => b[1] - a[1])[0][0];
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (c === '"') {
        if (text[i + 1] === '"') {
          cell += '"';
          i++;
        } else quoted = false;
      } else cell += c;
    } else if (c === '"') quoted = true;
    else if (c === sep) {
      row.push(cell);
      cell = "";
    } else if (c === "\n" || c === "\r") {
      if (c === "\r" && text[i + 1] === "\n") i++;
      row.push(cell);
      cell = "";
      if (row.some((x) => x.trim() !== "")) rows.push(row);
      row = [];
    } else cell += c;
  }
  row.push(cell);
  if (row.some((x) => x.trim() !== "")) rows.push(row);
  return rows;
}

/** The profile fields a CSV can carry, and the headings that mean them —
 *  English, Russian, and what the two most-copied competitors export. */
export type Field =
  | "name" | "proxy" | "tags" | "status" | "notes" | "start_urls" | "timezone" | "languages" | "persona"
  | "login" | "password" | "totp" | "cookies" | "token" | "email" | "email_password" | "backup_email";

/** The columns that make a row an account rather than only a profile. */
export const ACCOUNT_FIELDS: Field[] = ["login", "password", "totp", "cookies", "token", "email", "email_password", "backup_email"];

/** Never drawn in the preview: the dialog may be on a shared screen. */
export const SECRET_FIELDS: Field[] = ["password", "totp", "cookies", "token", "email_password"];

/** What a bar-separated line holds when it has no header, in the order account
 *  sellers use (tester, 04.11.2026). The preview lets a person move a column
 *  when theirs differs. */
export const ACCOUNT_LINE: Field[] = [...ACCOUNT_FIELDS, "proxy"];

const HEADINGS: Record<Field, string[]> = {
  name: ["name", "profile", "profile name", "title", "имя", "название", "профиль"],
  proxy: ["proxy", "proxy url", "прокси", "proxy_url"],
  tags: ["tags", "tag", "метки", "теги", "group", "группа"],
  status: ["status", "stage", "статус", "стадия"],
  notes: ["notes", "note", "comment", "remark", "заметки", "заметка", "комментарий", "примечание"],
  start_urls: ["start_urls", "start url", "start urls", "url", "urls", "site", "sites", "platform", "сайт", "сайты", "ссылки"],
  timezone: ["timezone", "time zone", "tz", "часовой пояс", "пояс"],
  languages: ["languages", "language", "lang", "языки", "язык"],
  persona: ["persona", "persona_id", "device", "персона", "устройство"],
  login: ["login", "id", "uid", "user", "username", "account", "account id", "логин", "аккаунт", "логин (id аккаунта)", "id аккаунта"],
  password: ["password", "pass", "пароль"],
  totp: ["2fa", "2fa secret", "2fa key", "totp", "otp", "secret", "секрет", "секретный ключ", "секретный ключ 2fa", "ключ 2fa"],
  cookies: ["cookies", "cookie", "куки"],
  token: ["token", "access token", "accesstoken", "токен"],
  email: ["email", "e-mail", "mail", "почта", "электронная почта"],
  email_password: ["email password", "e-mail password", "email pass", "mail password", "mail pass", "пароль от почты", "пароль почты"],
  backup_email: ["backup email", "recovery email", "reserve email", "резервная почта", "резервная электронная почта"],
};

/** Map header cells to fields. Unknown headings are kept as null so the
 *  preview can say "ignored" beside them rather than silently dropping them. */
export function mapHeader(header: string[]): (Field | null)[] {
  return header.map((h) => {
    const k = h.trim().toLowerCase().replace(/[_-](?=\S)/g, " ");
    const raw = h.trim().toLowerCase();
    for (const f of Object.keys(HEADINGS) as Field[]) if (HEADINGS[f].includes(k) || HEADINGS[f].includes(raw)) return f;
    return null;
  });
}

/** Does the first row look like a header rather than data? */
export function looksLikeHeader(row: string[]): boolean {
  return mapHeader(row).filter(Boolean).length >= Math.max(1, Math.ceil(row.length / 2));
}

/** Split a multi-valued cell: tags, URLs, languages. */
export function splitMulti(s: string): string[] {
  return s.split(/[;|\n]|,\s*/).map((x) => x.trim()).filter(Boolean);
}

/** Cookies a page can read in a real session on the sites these lines are sold
 *  for; the rest are HttpOnly there, and a page can tell the difference. */
const READABLE_COOKIES = new Set(["c_user", "presence", "wd", "dpr", "locale", "m_pixel_ratio"]);

/** `https://www.facebook.com` as `.facebook.com`; null for what is no address. */
export function cookieDomain(site: string): string | null {
  try {
    const host = new URL(/^[a-z]+:\/\//i.test(site.trim()) ? site.trim() : `https://${site.trim()}`).hostname;
    return host ? `.${host.replace(/^www\./, "")}` : null;
  } catch {
    return null;
  }
}

/** A cookies cell in either shape it is sold in: `a=1;b=2`, as a browser sends
 *  it, or a JSON array. Null for anything else.
 *
 *  The string form says nothing but names and values. A year's expiry goes on
 *  because import closes the browser, and a cookie without one is a session
 *  cookie that would be gone by the time anybody opened the profile. */
export function parseCookieCell(cell: string, site: string, nowSeconds: number): unknown[] | null {
  const text = cell.trim();
  if (!text) return null;
  if (text.startsWith("[") || text.startsWith("{")) {
    try {
      const value = JSON.parse(text);
      if (Array.isArray(value)) return value;
      const inner = (value as { cookies?: unknown }).cookies;
      return Array.isArray(inner) ? inner : null;
    } catch {
      return null;
    }
  }
  const domain = cookieDomain(site);
  if (!domain) return null;
  const pairs = text
    .split(";")
    .map((p) => p.trim())
    .filter(Boolean)
    .map((p) => {
      const at = p.indexOf("=");
      return at > 0 ? [p.slice(0, at).trim(), p.slice(at + 1).trim()] : null;
    });
  if (pairs.length === 0 || pairs.some((p) => p === null)) return null;
  return (pairs as string[][]).map(([name, value]) => ({
    name,
    value,
    domain,
    path: "/",
    secure: true,
    httpOnly: !READABLE_COOKIES.has(name),
    expires: nowSeconds + 365 * 24 * 3600,
  }));
}

/** The address a login at this mail domain belongs to, for the logins list. */
export function emailSite(email: string): string {
  const at = email.lastIndexOf("@");
  return at >= 0 ? `https://${email.slice(at + 1).trim().toLowerCase()}` : "";
}
