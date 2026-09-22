// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { api, type DomainList, type Shell } from "../api";
import { SecondFactor } from "./SecondFactor";
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
/** The issue form that turns a saved capture into a pull request
 *  (.github/workflows/persona-issue.yml). Drop the file in; a robot does the rest. */
const PERSONA_FORM = "https://github.com/furyteamtop/fury-antidetect-browser/issues/new?template=persona.yml";

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
  // Prefilled with the server this machine used last. The first screen already
  // does this, and somebody working locally never reaches the first screen --
  // the window opens straight into the profile list, so THIS is where the way
  // back has to be. Reported exactly that way: "открываю лаунчер и он сразу
  // входит, кнопки войти в свой аккаунт нет".
  const [url, setUrl] = useState(shell.last_server ?? "");
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
                      {busy
                        ? t("srv.checking")
                        : shell.last_server && shell.last_email
                          ? t("set.signInAgain")
                          : t("set.connect")}
                    </button>
                  </div>
                  {shell.last_server && shell.last_email && (
                    <p className="hint">{t("srv.comeBack", { email: shell.last_email })}</p>
                  )}
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
                  {/* Two different exits, and only the second one was here.
                      Signing out ends the session and leaves the server
                      configured, so the next screen is the password form with
                      the address already in it -- which is what somebody means
                      by "let me back to where I type my account". It lived in
                      Users and nowhere else, which is a strange place to look
                      for it and was reported as missing.

                      Disconnecting is the bigger hammer: it forgets the server
                      as well, and the label says so. */}
                  <button
                    onClick={async () => {
                      await api.logout();
                      onChanged(await api.shell());
                      onClose();
                    }}
                  >
                    {t("set.signOut")}
                  </button>
                  <p className="hint">{t("set.signOutHint")}</p>
                  <button
                    className="ghost"
                    style={{ marginTop: "var(--s-3)" }}
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
          {tab === "team" && shell.mode !== "local" && shell.signed_in && <SecondFactor />}

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

              <DomainLists />

              <CaptureMachine />

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
          {shell.log_file && (
            <>
              <dt>{t("about.log")}</dt>
              <dd className="mono">{shell.log_file}</dd>
            </>
          )}
        </dl>
        {shell.log_file && <p className="hint">{t("about.logWhy")}</p>}

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
          <dt>{t("about.contact")}</dt>
          <dd>
            <a
              href="https://t.me/shapovalovbogdan"
              target="_blank"
              rel="noreferrer"
              onClick={(e) => {
                e.preventDefault();
                void api.openUrl("https://t.me/shapovalovbogdan");
              }}
            >
              @shapovalovbogdan
            </a>
          </dd>
          <dt>{t("about.source")}</dt>
          <dd>
            {/* target="_blank" alone does nothing here, and this link was proof
                of it: a Tauri window has no tab to open and no browser behind
                it, so the click was silence. Same handler as the release link
                above -- the href stays for the address on hover and for a
                right-click copy, and the click is handed to the system. */}
            <a
              href="https://github.com/furyteamtop/fury-antidetect-browser"
              target="_blank"
              rel="noreferrer"
              onClick={(e) => {
                e.preventDefault();
                void api.openUrl("https://github.com/furyteamtop/fury-antidetect-browser");
              }}
            >
              github.com/furyteamtop/fury-antidetect-browser
            </a>
          </dd>
        </dl>
      </div>
    </>
  );
}

/** The relay's domain lists: pasted text, saved under a name, chosen per
 *  profile in its editor.
 *
 *  This screen is the whole of the feature's interface, and it is dated
 *  12.09.2026 while blocklist.rs is dated 07.08.2026: the relay refused hosts
 *  for five weeks with no way for anyone to tell it which. The audit in
 *  docs/12 found the same shape three times in one day — extensions, disk
 *  usage, this — and the lesson recorded there applies: a feature the operator
 *  cannot reach is a feature the product does not have. */
function DomainLists() {
  const { t, say } = useI18n();
  const [lists, setLists] = useState<DomainList[]>([]);
  const [name, setName] = useState("");
  const [text, setText] = useState("");
  const [isNew, setIsNew] = useState(true);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = async () => {
    try {
      setLists(await api.blocklists());
    } catch (e) {
      setError(say(e));
    }
  };
  useEffect(() => {
    void reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const pick = async (n: string) => {
    setNote(null);
    setError(null);
    if (n === "") {
      setIsNew(true);
      setName("");
      setText("");
      return;
    }
    setIsNew(false);
    setName(n);
    try {
      setText((await api.readBlocklist(n)).text);
    } catch (e) {
      setError(say(e));
    }
  };

  return (
    <div className="settingsGroup">
      <h2>{t("set.domainLists")}</h2>
      <p className="hint">{t("set.domainListsHint")}</p>
      <div className="field">
        <label htmlFor="dl-pick">{t("set.listName")}</label>
        <div>
          <select id="dl-pick" value={isNew ? "" : name} onChange={(e) => void pick(e.target.value)}>
            <option value="">{t("set.listNew")}</option>
            {lists.map((l) => (
              <option key={l.name} value={l.name}>
                {l.name} — {l.allow_only ? t("pd.listAllowOnly", { n: l.domains }) : t("pd.listBlocks", { n: l.domains })}
              </option>
            ))}
          </select>
          {isNew && (
            <>
              <input
                style={{ marginTop: "var(--s-2)" }}
                value={name}
                placeholder="ads"
                onChange={(e) => setName(e.target.value)}
              />
              <p className="hint">{t("set.listNameHint")}</p>
            </>
          )}
        </div>
      </div>
      <div className="field">
        <label htmlFor="dl-text">{t("set.listText")}</label>
        <div>
          <textarea
            id="dl-text"
            rows={8}
            value={text}
            spellCheck={false}
            style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 12 }}
            placeholder={"@allow-only\nfacebook.com\n||fbcdn.net^"}
            onChange={(e) => setText(e.target.value)}
          />
          <div style={{ display: "flex", gap: "var(--s-2)", marginTop: "var(--s-2)" }}>
            <button
              className="primary"
              disabled={busy || !name.trim() || !text.trim()}
              onClick={async () => {
                setBusy(true);
                setError(null);
                setNote(null);
                try {
                  const saved = await api.saveBlocklist(name.trim(), text);
                  setNote(
                    t("set.listSaved", {
                      n: saved.domains,
                      mode: saved.allow_only ? t("set.listModeAllow") : "",
                    }),
                  );
                  setIsNew(false);
                  await reload();
                } catch (e) {
                  setError(say(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              {t("set.listSave")}
            </button>
            {!isNew && (
              <button
                className="ghost danger"
                disabled={busy}
                onClick={async () => {
                  setBusy(true);
                  setError(null);
                  try {
                    await api.deleteBlocklist(name);
                    await pick("");
                    await reload();
                  } catch (e) {
                    setError(say(e));
                  } finally {
                    setBusy(false);
                  }
                }}
              >
                {t("set.listDelete")}
              </button>
            )}
          </div>
          {note && <p>{note}</p>}
          {error && <p className="error">{error}</p>}
        </div>
      </div>
    </div>
  );
}

/** This machine as a persona, for the catalogue.
 *
 *  The catalogue has 27 machines and two are measured; the rest are derived
 *  from those two. The machines that would widen the crowd belong to the
 *  people who installed the .dmg. This button runs the same capture the
 *  maintainers run — the installed Chrome at the detect-suite probe, in a
 *  throwaway profile — converts it, and shows the whole result before offering
 *  to write a file. It sends nothing: getting the file into the repository is
 *  a pull request, because README says "no telemetry" and means it.
 *
 *  The warning on screen is not boilerplate. A persona IS this machine's
 *  fingerprint — publishing it publishes how this computer looks to any site.
 *  For the laptop that runs live accounts that is a bad idea, and the screen
 *  says so before the button does anything. */
function CaptureMachine() {
  const { t, say } = useI18n();
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ persona: Record<string, unknown> & { id: string }; problems: string[]; browser: string } | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  return (
    <div className="settingsGroup">
      <h2>{t("cap.title")}</h2>
      <p className="hint">{t("cap.why")}</p>
      <div className="verdict bad" style={{ marginBottom: "var(--s-3)" }}>{t("cap.warning")}</div>
      <p className="hint">{t("cap.what")}</p>
      <div className="row">
        <button
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            setError(null);
            setSaved(null);
            setResult(null);
            try {
              setResult(await api.capturePersona());
            } catch (e) {
              setError(say(e));
            } finally {
              setBusy(false);
            }
          }}
        >
          {busy ? t("cap.running") : t("cap.run")}
        </button>
      </div>
      {error && <p className="error">{error}</p>}
      {result && (
        <div style={{ marginTop: "var(--s-3)" }}>
          <p>
            <strong>{result.persona.id}</strong>
            <span className="muted small"> · {result.browser}</span>
          </p>
          {result.problems.length === 0 ? (
            <div className="verdict good">{t("cap.consistent")}</div>
          ) : (
            <div className="verdict bad">
              <div>{t("cap.problems")}</div>
              {result.problems.map((x) => <div key={x}>{x}</div>)}
            </div>
          )}
          <textarea
            readOnly
            rows={12}
            spellCheck={false}
            value={JSON.stringify(result.persona, null, 2)}
            style={{ width: "100%", fontFamily: "var(--mono)", fontSize: 11, marginTop: "var(--s-2)" }}
          />
          <p className="hint">{t("cap.inFile")}</p>
          <div className="row" style={{ marginTop: "var(--s-2)" }}>
            <button
              className="primary"
              disabled={busy}
              onClick={async () => {
                try {
                  setSaved(await api.savePersonaFile(result.persona));
                } catch (e) {
                  setError(say(e));
                }
              }}
            >
              {t("cap.save")}
            </button>
          </div>
          {saved && (
            <p className="hint">
              {t("cap.saved", { path: saved })} {t("cap.next")}{" "}
              <a
                href={PERSONA_FORM}
                target="_blank"
                rel="noreferrer"
                onClick={(e) => {
                  e.preventDefault();
                  void api.openUrl(PERSONA_FORM);
                }}
              >
                {t("cap.openForm")}
              </a>
            </p>
          )}
        </div>
      )}
    </div>
  );
}
