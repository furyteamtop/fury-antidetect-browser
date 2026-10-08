// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { api, type Assistants } from "../api";
import { useI18n } from "../i18n";

/** AI assistants: Fury as an MCP server, connected with one button.
 *
 *  In the sidebar rather than inside Settings, because the person who wants it
 *  is looking for it by name. MCP existed for weeks as a README paragraph and
 *  a Python script; a tester searched the application for it "like AdsPower
 *  has", found nothing, and said that is where people get lost (08.10.2026).
 *
 *  The server is `fury-agent mcp` (agent/src/mcp.rs); this screen writes the
 *  client configs (src-tauri/assistants.rs). Nothing here talks to a model:
 *  the assistant, its account and its key are the person's own. */
export function AssistantsView() {
  const { t, say } = useI18n();
  const [st, setSt] = useState<Assistants | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.assistants().then(setSt, (e) => setError(say(e)));
  }, []);

  const run = async (f: () => Promise<Assistants>, done: string) => {
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      setSt(await f());
      setNote(done);
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setNote(t("ai.copied"));
    } catch {
      setError(t("ai.copyFailed"));
    }
  };

  const name = (id: string) => (id === "claude_desktop" ? "Claude Desktop" : "Cursor");

  return (
    <div className="settings" style={{ overflowY: "auto", paddingBottom: "var(--s-6)" }}>
      <div className="settingsGroup">
        <p>{t("ai.what")}</p>
        <ul className="hint" style={{ margin: "0 0 var(--s-3)", paddingLeft: 18 }}>
          <li>{t("ai.example1")}</li>
          <li>{t("ai.example2")}</li>
          <li>{t("ai.example3")}</li>
        </ul>
        <p className="hint">{t("ai.private")}</p>
      </div>

      {note && <div className="notice" style={{ maxWidth: 620 }}>{note}</div>}
      {error && <div className="notice warnBar" style={{ maxWidth: 620 }}>{error}</div>}
      {st?.agent_problem === "not_installed" && (
        <div className="notice warnBar" style={{ maxWidth: 620 }}>{t("ai.moveToApplications")}</div>
      )}

      {st && (
        <>
          <div className="settingsGroup">
            <h2>{t("ai.connect")}</h2>
            <p>{t("ai.connectHint")}</p>
            {st.clients.map((c) => (
              <div key={c.id} className="row" style={{ alignItems: "center", marginBottom: "var(--s-2)" }}>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div>
                    <strong>{name(c.id)}</strong>{" "}
                    <span className="muted small">
                      {c.connected
                        ? t("ai.connected")
                        : c.stale
                          ? t("ai.stale")
                          : c.installed
                            ? t("ai.notConnected")
                            : t("ai.notFound")}
                    </span>
                  </div>
                  <div className="muted small mono" style={{ overflowWrap: "anywhere" }}>{c.config}</div>
                </div>
                {c.connected ? (
                  <button
                    className="ghost"
                    disabled={busy}
                    onClick={() => run(() => api.assistantsDisconnect(c.id), t("ai.disconnectedNote", { app: name(c.id) }))}
                  >
                    {t("ai.disconnect")}
                  </button>
                ) : (
                  <button
                    className="primary"
                    disabled={busy || st.agent_problem !== null}
                    onClick={() => run(() => api.assistantsConnect(c.id), t("ai.connectedNote", { app: name(c.id) }))}
                  >
                    {c.stale ? t("ai.reconnect") : t("ai.connectButton")}
                  </button>
                )}
              </div>
            ))}
          </div>

          {st.claude_code && (
            <div className="settingsGroup">
              <h2>Claude Code</h2>
              <p>{t("ai.claudeCodeHint")}</p>
              <div className="row" style={{ alignItems: "center", marginBottom: "var(--s-2)" }}>
                <code className="mono" style={{ flex: 1, minWidth: 0, overflowWrap: "anywhere" }}>{st.claude_code}</code>
                <button className="ghost" onClick={() => void copy(st.claude_code!)}>{t("ai.copy")}</button>
              </div>
              <div className="row" style={{ alignItems: "center" }}>
                <span className="muted small" style={{ flex: 1 }}>
                  {st.skill_installed ? t("ai.skillInstalled", { path: st.skill_path }) : t("ai.skillHint")}
                </span>
                <button
                  className="ghost"
                  disabled={busy}
                  onClick={() => run(() => api.assistantsInstallSkill(), t("ai.skillDone", { path: st.skill_path }))}
                >
                  {st.skill_installed ? t("ai.skillUpdate") : t("ai.skillInstall")}
                </button>
              </div>
            </div>
          )}

          {st.snippet && (
            <div className="settingsGroup">
              <h2>{t("ai.other")}</h2>
              <p>{t("ai.otherHint")}</p>
              <pre className="mono" style={{ whiteSpace: "pre-wrap", overflowWrap: "anywhere", margin: "0 0 var(--s-2)" }}>
                {st.snippet}
              </pre>
              <button className="ghost" onClick={() => void copy(st.snippet!)}>{t("ai.copy")}</button>
            </div>
          )}

          <div className="settingsGroup">
            <h2>{t("ai.can")}</h2>
            <p>{t("ai.canList")}</p>
            <p className="hint">{t("ai.cannot")}</p>
          </div>
        </>
      )}
    </div>
  );
}
