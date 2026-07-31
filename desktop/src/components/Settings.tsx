import { useState } from "react";
import { api, type Shell } from "../api";
import { languages, useI18n, type Language } from "../i18n";
import { type Theme, themes, useTheme } from "../theme";

/** Everything that is a preference rather than a property of a profile.
 *
 *  Deliberately short. A settings screen that grows without resistance becomes
 *  the place decisions go to be avoided — each of these exists because leaving
 *  it out would force a choice on someone it does not fit. */
export function Settings({
  shell,
  hasProject,
  onExport,
  onImport,
  onChanged,
  onClose,
}: {
  shell: Shell;
  hasProject: boolean;
  onExport: () => void;
  onImport: () => void;
  onChanged: (s: Shell) => void;
  onClose: () => void;
}) {
  const [theme, setTheme] = useTheme();
  const { t, language, setLanguage } = useI18n();
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>{t("set.title")}</h2>
        </div>

        <div className="form settings" style={{ flex: 1, overflowY: "auto" }}>
          <div className="settingsGroup">
            <h2>{t("set.appearance")}</h2>
            <p>
              {t("set.appearanceHint")}
            </p>
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

          <div className="settingsGroup">
            <h2>{t("set.teamServer")}</h2>
            {shell.mode === "local" ? (
              <>
                <p>
                  {t("set.notConnected")}
                </p>
                <p className="hint">{t("set.notConnectedHint")}</p>
                {/* The address goes in here rather than behind a first-run wall.
                    Connecting is a decision made once a team exists, which is
                    usually long after the app was installed. */}
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
            <p className="hint">
              {t("set.machineHint")}
            </p>
          </div>
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
