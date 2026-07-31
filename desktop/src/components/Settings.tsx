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
                  <button type="button" className="linky" onClick={onEnrol}>
                    {t("enrol.have")}
                  </button>
                  <p className="hint">{t("set.howTo")}</p>
                </>
              ) : (
                <>
                  <p className="mono">{shell.server_url}</p>
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
                  <a href={check.url} target="_blank" rel="noreferrer">
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
          <dd>Богдан Шаповалов</dd>
          <dt>{t("about.source")}</dt>
          <dd>
            <a href="https://github.com/fury-browser/fury" target="_blank" rel="noreferrer">
              github.com/fury-browser/fury
            </a>
          </dd>
        </dl>
      </div>
    </>
  );
}
