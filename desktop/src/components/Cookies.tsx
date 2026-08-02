// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useState } from "react";
import { api, type Profile } from "../api";
import { useI18n } from "../i18n";

/** Cookies in and out of one profile.
 *
 *  This is how a warmed account arrives from somewhere else, and how it leaves.
 *
 *  Two things are said on the screen rather than discovered afterwards. The
 *  profile is opened and closed to do the exchange — the browser holds the
 *  encryption key, so nothing else can read or write the jar, and a person who
 *  sees a window appear should know why. And a cookie with no expiry is a
 *  session cookie, which Chromium never writes to disk, so it does not survive
 *  the profile closing again: importing forty and finding eleven is a bad
 *  surprise, and the count is reported instead. */
export function Cookies({
  profile,
  onClose,
}: {
  profile: Profile;
  onClose: () => void;
}) {
  const { t, say } = useI18n();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const parsed = (): unknown[] | null => {
    try {
      const value = JSON.parse(text);
      return Array.isArray(value) ? value : null;
    } catch {
      return null;
    }
  };
  const ready = text.trim() !== "" && parsed() !== null;

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "88vh" }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{t("ck.title", { name: profile.name })}</h2>
        </div>

        <div className="form" style={{ paddingTop: "var(--s-5)", overflowY: "auto" }}>
          <p className="hint">{t("ck.opens")}</p>

          <div className="field">
            <label htmlFor="ck-text">{t("ck.json")}</label>
            <div>
              <textarea
                id="ck-text"
                value={text}
                spellCheck={false}
                rows={12}
                style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 12 }}
                placeholder={'[{"name":"sid","value":"…","domain":".example.com","path":"/"}]'}
                onChange={(e) => {
                  setText(e.target.value);
                  setNote(null);
                }}
              />
              <p className="hint">{t("ck.formats")}</p>
              {text.trim() !== "" && parsed() === null && (
                <p className="error">{t("ck.notJson")}</p>
              )}
            </div>
          </div>

          {note && <p>{note}</p>}
          {error && <p className="error">{error}</p>}
        </div>

        <div className="modalFoot">
          <button
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              setError(null);
              setNote(null);
              try {
                const { cookies } = await api.exportCookies(profile.id);
                setText(JSON.stringify(cookies, null, 2));
                setNote(t("ck.exported", { n: cookies.length }));
              } catch (e) {
                setError(say(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            {busy ? t("ck.working") : t("ck.export")}
          </button>
          <div className="spacer" />
          <button onClick={onClose}>{t("ui.close")}</button>
          <button
            className="primary"
            disabled={busy || !ready}
            onClick={async () => {
              setBusy(true);
              setError(null);
              setNote(null);
              try {
                const r = await api.importCookies(profile.id, parsed() ?? []);
                setNote(
                  [
                    t("ck.imported", { n: r.imported }),
                    r.session_only > 0 ? t("ck.sessionOnly", { n: r.session_only }) : "",
                    r.skipped > 0 ? t("ck.skipped", { n: r.skipped }) : "",
                  ]
                    .filter(Boolean)
                    .join(" "),
                );
              } catch (e) {
                setError(say(e));
              } finally {
                setBusy(false);
              }
            }}
          >
            {busy ? t("ck.working") : t("ck.import")}
          </button>
        </div>
      </div>
    </div>
  );
}
