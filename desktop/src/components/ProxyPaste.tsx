// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useState } from "react";
import { api } from "../api";
import { dictionary, useI18n } from "../i18n";

type Result = Awaited<ReturnType<typeof api.importProxies>>;

/** A supplier's block, pasted.
 *
 *  Two hundred proxies arrive as text, in whichever shape the supplier prefers,
 *  and typing them one at a time is not a thing anyone does. The parser lives in
 *  Rust so the launcher, the agent and the CLI all read a line the same way —
 *  three parsers would be two of them being separately wrong.
 *
 *  What this screen owes the operator is the READING it took. `host:port:user:pass`
 *  and `user:pass:host:port` produce rows that look identical and exits that are
 *  not, so every saved line shows which shape matched, and every refused line
 *  shows its number and what was wrong with it. A silent drop is how someone
 *  later finds profiles pointing at nothing. */
export function ProxyPaste({
  onClose,
  onSaved,
}: {
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useI18n();
  const dict = dictionary();
  const [text, setText] = useState("");
  const [prefix, setPrefix] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<Result | null>(null);

  const lines = text.split("\n").filter((l) => {
    const s = l.trim();
    return s !== "" && !s.startsWith("#") && !s.startsWith("//");
  }).length;

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" style={{ height: "auto", maxHeight: "88vh" }} role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{t("pp.title")}</h2>
        </div>

        <div className="form" style={{ paddingTop: "var(--s-5)", overflowY: "auto" }}>
          {!result && (
            <>
              <div className="field">
                <label htmlFor="pp-text">{t("pp.list")}</label>
                <div>
                  <textarea
                    id="pp-text"
                    value={text}
                    autoFocus
                    spellCheck={false}
                    rows={10}
                    style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 12 }}
                    placeholder={"1.2.3.4:8080:user:pass\nsocks5://user:pass@gw.provider.net:1080\n5.6.7.8:3128"}
                    onChange={(e) => setText(e.target.value)}
                  />
                  <p className="hint">{t("pp.formats")}</p>
                </div>
              </div>

              <div className="field">
                <label htmlFor="pp-prefix">{t("pp.prefix")}</label>
                <div>
                  <input
                    id="pp-prefix"
                    value={prefix}
                    placeholder={t("pp.prefixPlaceholder")}
                    onChange={(e) => setPrefix(e.target.value)}
                  />
                  <p className="hint">{t("pp.prefixHint")}</p>
                </div>
              </div>

              {/* Said before the click, not after: someone pasting a list from a
                  supplier reasonably expects the proxies to have been tried. */}
              <p className="hint">{t("pp.noCheck")}</p>
            </>
          )}

          {result && (
            <>
              <p>{t("pp.saved", { n: result.saved.length })}</p>
              {result.saved.length > 0 && (
                <table className="grid">
                  <thead>
                    <tr>
                      <th>{t("pp.line")}</th>
                      <th>{t("pp.address")}</th>
                      <th>{t("pp.readAs")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {result.saved.map((s) => (
                      <tr key={s.id}>
                        <td className="muted">{s.line}</td>
                        <td className="mono">
                          {s.host}:{s.port}
                        </td>
                        <td className="muted small">{t(`pp.shape.${s.shape}` as never)}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}

              {result.rejected.length > 0 && (
                <div className="settingsGroup">
                  <h2>{t("pp.rejected", { n: result.rejected.length })}</h2>
                  <ul className="hint" style={{ paddingLeft: "1.2em", lineHeight: 1.7 }}>
                    {result.rejected.map((r) => (
                      <li key={r.line}>
                        {/* The code is the parser's own name for the reason;
                            the English sentence it also sends is the fallback
                            for a reason nobody has translated yet. */}
                        {t("pp.lineN", { n: r.line })}:{" "}
                        {r.code && `pp.why.${r.code}` in dict
                          ? t(`pp.why.${r.code}` as never)
                          : r.error}
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </>
          )}

          {error && <p className="error">{error}</p>}
        </div>

        <div className="modalFoot">
          <div className="spacer" />
          {result ? (
            <button
              className="primary"
              onClick={() => {
                onSaved();
                onClose();
              }}
            >
              {t("ui.done")}
            </button>
          ) : (
            <>
              <button onClick={onClose}>{t("ui.cancel")}</button>
              <button
                className="primary"
                disabled={busy || lines === 0}
                onClick={async () => {
                  setBusy(true);
                  setError(null);
                  try {
                    setResult(await api.importProxies(text, prefix.trim()));
                  } catch (e) {
                    setError((e as Error).message);
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {busy ? t("pp.importing") : t("pp.import", { n: lines })}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
