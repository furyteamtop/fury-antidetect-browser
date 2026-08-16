// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useState } from "react";
import { api, type Shell } from "../api";
import { languages, useI18n, type Language } from "../i18n";
import { type Theme, themes, useTheme } from "../theme";

const TABS = ["general", "team", "data", "about"] as const;
type Tab = (typeof TABS)[number];

/** Everything that is a preference rather than a property of a profile.
 *
 *  Tabbed rather than one long scroll. Four unrelated subjects stacked
 *  vertically means the answer to "where do I change the language" is "scroll
 *  and look", and the panel that grows fastest is the one nobody can navigate.
 *
 *  Deliberately short all the same. A settings screen that grows without
 *  resistance becomes the place decisions go to be avoided — each of these
 *  exists because leaving it out would force a choice on someone it does not
 *  fit. */
export function Settings({
  shell,
  hasProject,
  onExport,
  onImport,
  onChanged,
  onEnrol,
  onSignup,
  onClose,
}: {
  shell: Shell;
  hasProject: boolean;
  onExport: () => void;
  onImport: () => void;
  onChanged: (s: Shell) => void;
  /** Someone handed an invitation has no account to sign in with yet, and the
   *  code already carries the server address — so this is a way past the
   *  connect field, not through it. */
  onEnrol: () => void;
  onSignup: () => void;
  onClose: () => void;
}) {
  const [theme, setTheme] = useTheme();
  const { t, language, setLanguage } = useI18n();
  const [tab, setTab] = useState<Tab>("general");
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{t("set.title")}</h2>
        </div>

        <div className="tabs" role="tablist">
          {TABS.map((x) => (
            <button key={x} role="tab" aria-selected={tab === x} onClick={() => setTab(x)}>
              {t(`set.tab.${x}` as never)}
            </button>
          ))}
        </div>

        <div className="form settings" style={{ flex: 1, overflowY: "auto" }}>
          {tab === "general" && (
            <>
              <div className="settingsGroup">
                <h2>{t("set.appearance")}</h2>
                <p>{t("set.appearanceHint")}</p>
                <div className="segmented">
                  {themes.map((th) => (
                    <button
                      key={th}
                      aria-pressed={theme === th}
                      onClick={() => setTheme(th as Theme)}
                    >
                      {th === "system"
                        ? t("set.themeSystem")
                        : th === "dark"
                          ? t("set.themeDark")
                          : t("set.themeLight")}
                    </button>
                  ))}
                </div>
              </div>

              <div className="settingsGroup">
                <h2>{t("set.language")}</h2>
                <p>{t("set.languageHint")}</p>
                <div className="segmented">
                  {languages.map((l) => (
                    <button
                      key={l}
                      aria-pressed={language === l}
                      onClick={() => setLanguage(l as Language)}
                    >
                      {l === "system" ? t("set.langSystem") : l === "ru" ? "Русский" : "English"}
                    </button>
                  ))}
                </div>
              </div>
            </>
          )}

          {tab === "team" && (
            <div className="settingsGroup">
              <h2>{t("set.teamServer")}</h2>
              {shell.mode === "local" ? (
                <>
                  <p>{t("set.notConnected")}</p>
                  <p className="hint">{t("set.notConnectedHint")}</p>
                  {/* The address goes in here rather than behind a first-run
                      wall. Connecting is a decision made once a team exists,
                      which is usually long after the app was installed. */}
                  <div className="row" style={{ maxWidth: 420 }}>
                    <input
                      value={url}
                      placeholder={t("set.serverPlaceholder")}
                      spellCheck={false}
                      onChange={(e) => setUrl(e.target.value)}
                    />
                    <button
                      className="primary"
                      disabled={busy || !url.trim()}
                      onClick={async () => {
                        setBusy(true);
                        setError(null);
                        try {
                          onChanged(await api.setServer(url));
                          onClose();
                        } catch (e) {
                          setError((e as Error).message);
                        } finally {
                          setBusy(false);
                        }
                      }}
                    >
                      {busy ? t("srv.checking") : t("set.connect")}
                    </button>
                  </div>
                  {error && <p className="error">{error}</p>}
                  {/* Three ways in, and none of them should require leaving.
                      Signing up used to live only on the login screen, which is
                      the screen you see after you have already left local mode —
                      so making an account meant signing out of a session you did
                      not have, to reach a form that would have put you back
                      where you started. It is a settings decision, so it is in
                      settings.

                      Nothing else has to change for it to work: `signup` already
                      saves the server address and stores the session token, so
                      the account is created AND signed into in one step. */}
                  <div className="subActions">
                    <span className="lead">{t("set.orElse")}</span>
                    <button type="button" className="linky" onClick={onSignup}>
                      {t("signup.start")}
                    </button>
                    <button type="button" className="linky" onClick={onEnrol}>
                      {t("enrol.have")}
                    </button>
                  </div>
                  <SelfHosting />
                </>
              ) : (
                <>
                  <p className="mono">{shell.server_url}</p>
                  <label className="row" style={{ gap: 8, margin: "var(--s-3) 0" }}>
                    <input
                      type="checkbox"
                      style={{ width: 14, height: 14, accentColor: "var(--accent)" }}
                      checked={shell.remember_org_key}
                      onChange={async (e) => onChanged(await api.setRememberOrgKey(e.target.checked))}
                    />
                    <span>{t("set.rememberKey")}</span>
                  </label>
                  <p className="hint">{t("set.rememberKeyHint")}</p>
                  <button
                    onClick={async () => {
                      onChanged(await api.disconnectServer());
                      onClose();
                    }}
                  >
                    {t("set.disconnect")}
                  </button>
                </>
              )}
            </div>
          )}

          {tab === "data" && (
            <>
              {shell.mode === "local" && (
                <div className="settingsGroup">
                  <h2>{t("set.transfer")}</h2>
                  <p>{t("set.transferHint")}</p>
                  <div className="row">
                    <button
                      disabled={!hasProject}
                      onClick={() => {
                        onClose();
                        onExport();
                      }}
                    >
                      {t("nav.export")}
                    </button>
                    <button
                      onClick={() => {
                        onClose();
                        onImport();
                      }}
                    >
                      {t("nav.import")}
                    </button>
                  </div>
                </div>
              )}

              <div className="settingsGroup">
                <h2>{t("set.thisMachine")}</h2>
                <dl className="kv">
                  <dt>{t("set.machineName")}</dt>
                  <dd>{shell.machine_name}</dd>
                  <dt>{t("set.agent")}</dt>
                  <dd>{shell.agent_ready ? t("set.agentRunning") : t("set.agentStopped")}</dd>
                </dl>
                <p className="hint">{t("set.machineHint")}</p>
              </div>
            </>
          )}

          {tab === "about" && <About shell={shell} />}
        </div>

        <div className="modalFoot">
          <div className="spacer" />
          <button className="primary" onClick={onClose}>
            {t("set.done")}
          </button>
        </div>
      </div>
    </div>
  );
}

/// What to run to have a server of your own.
///
/// In the application rather than in a document, because the person who needs
/// it has an application and not a repository — "see docs/13-self-hosting.md"
/// was an instruction they could not follow.
function SelfHosting() {
  const { t, say } = useI18n();
  const [open, setOpen] = useState(false);
  const [dir, setDir] = useState("~/fury-server");
  const [saved, setSaved] = useState<{ path: string; files: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  // Step 2 used to say "from a clone of the repository", which somebody holding
  // an application does not have. The 410 KB the server is built from travels
  // inside this app, so the step is now "save them, then run this".
  const saveKit = async () => {
    setBusy(true);
    setErr(null);
    try {
      setSaved(await api.saveServerKit(dir));
    } catch (e) {
      setErr(say(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="guide">
      <button type="button" className="linky" onClick={() => setOpen(!open)}>
        <span aria-hidden="true" className="disclosure">{open ? "\u25be" : "\u25b8"}</span>{" "}
        {t("host.show")}
      </button>
      {open && (
        <div style={{ maxWidth: 620 }}>
          <p className="hint">{t("host.intro")}</p>
          <ol className="hint" style={{ paddingLeft: "1.2em", lineHeight: 1.7 }}>
            <li>{t("host.step1")}</li>
            <li>
              <p style={{ margin: 0 }}>{t("host.saveKitHint")}</p>
              <div className="row" style={{ maxWidth: 460, margin: "var(--s-2) 0" }}>
                <input
                  value={dir}
                  placeholder={t("host.saveKitPath")}
                  spellCheck={false}
                  onChange={(e) => setDir(e.target.value)}
                />
                <button type="button" disabled={busy || !dir.trim()} onClick={saveKit}>
                  {t("host.saveKit")}
                </button>
              </div>
              {err && <p className="error">{err}</p>}
              {saved && (
                <p style={{ margin: 0 }}>
                  {t("host.saveKitDone", { files: String(saved.files), path: saved.path })}
                </p>
              )}
              {t("host.step2")}
              <pre className="mono snippet">./deploy/push.sh root@ADDRESS ADDRESS-with-dashes.sslip.io</pre>
            </li>
            <li>
              {t("host.step3")}
              <pre className="mono snippet">fury-server invite --email you@example.com --org "My team"</pre>
            </li>
            <li>{t("host.step4")}</li>
          </ol>
          <p className="hint">{t("host.note")}</p>
        </div>
      )}
    </div>
  );
}

type Check = Awaited<ReturnType<typeof api.checkUpdate>>;

function About({ shell }: { shell: Shell }) {
  const { t } = useI18n();
  const [check, setCheck] = useState<Check | null>(null);
  const [busy, setBusy] = useState(false);

  return (
    <>
      <div className="settingsGroup">
        <h2>Fury</h2>
        <p>{t("about.what")}</p>
        <dl className="kv">
          <dt>{t("about.version")}</dt>
          <dd className="mono">{shell.version}</dd>
        </dl>

        <div className="row">
          <button
            disabled={busy}
            onClick={async () => {
              setBusy(true);
              try {
                setCheck(await api.checkUpdate());
              } finally {
                setBusy(false);
              }
            }}
          >
            {busy ? t("about.checking") : t("about.checkUpdates")}
          </button>
        </div>

        {check && (
          <p className={check.status === "available" ? "" : "hint"}>
            {check.status === "available" && (
              <>
                {t("about.available", { version: check.latest ?? "" })}{" "}
                {check.url && (
                  <a
                    href={check.url}
                    onClick={(e) => {
                      // The href stays for the address it shows on hover and
                      // for a right-click copy; the click is handled, because
                      // in a Tauri window the navigation itself goes nowhere.
                      e.preventDefault();
                      void api.openUrl(check.url!);
                    }}
                  >
                    {t("about.openRelease")}
                  </a>
                )}
              </>
            )}
            {check.status === "current" && t("about.upToDate")}
            {/* Nothing published yet is the honest state of a project before
                its first release, and saying "you are up to date" would be a
                claim the feed did not make. */}
            {check.status === "unpublished" && t("about.noReleases")}
            {check.status === "unreachable" && (check.message ?? t("about.unreachable"))}
          </p>
        )}

        {/* Said plainly rather than implied by a missing button: an application
            that could silently replace itself is exactly what this one must not
            be, and people running accounts deserve to know which it is. */}
        <p className="hint">{t("about.noAutoInstall")}</p>
      </div>

      <div className="settingsGroup">
        <h2>{t("about.licence")}</h2>
        <p>{t("about.licenceBody")}</p>
        <p className="hint">{t("about.licenceWhy")}</p>
      </div>

      <div className="settingsGroup">
        <h2>{t("about.author")}</h2>
        <dl className="kv">
          <dt>{t("about.madeBy")}</dt>
          <dd>Bogdan Shapovalov</dd>
          <dt>{t("about.source")}</dt>
          <dd>
            <a href="https://github.com/furyteamtop/fury-antidetect-browser" target="_blank" rel="noreferrer">
              github.com/furyteamtop/fury-antidetect-browser
            </a>
          </dd>
        </dl>
      </div>
    </>
  );
}
