import { api, type Shell } from "../api";
import { type Theme, themes, useTheme } from "../theme";

/** Everything that is a preference rather than a property of a profile.
 *
 *  Deliberately short. A settings screen that grows without resistance becomes
 *  the place decisions go to be avoided — each of these exists because leaving
 *  it out would force a choice on someone it does not fit. */
export function Settings({
  shell,
  onChanged,
  onClose,
}: {
  shell: Shell;
  onChanged: (s: Shell) => void;
  onClose: () => void;
}) {
  const [theme, setTheme] = useTheme();

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>Settings</h2>
        </div>

        <div className="form settings" style={{ flex: 1, overflowY: "auto" }}>
          <div className="settingsGroup">
            <h2>Appearance</h2>
            <p>
              Following the system is the default: an application that ignores the
              desktop's own setting is the one that glares at two in the morning.
            </p>
            <div className="segmented">
              {themes.map((t) => (
                <button
                  key={t}
                  aria-pressed={theme === t}
                  onClick={() => setTheme(t as Theme)}
                >
                  {t === "system" ? "System" : t === "dark" ? "Dark" : "Light"}
                </button>
              ))}
            </div>
          </div>

          <div className="settingsGroup">
            <h2>Team server</h2>
            {shell.mode === "local" ? (
              <>
                <p>
                  Not connected. Everything is on this machine: no account, no
                  database, nothing leaves. Connect a server when there is a team to
                  share projects with — see docs/13 for standing one up.
                </p>
                <p className="hint">
                  Connecting does not upload anything on its own. Profiles created
                  here stay here until bundle sync exists.
                </p>
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
                  Disconnect and work locally
                </button>
              </>
            )}
          </div>

          <div className="settingsGroup">
            <h2>This machine</h2>
            <dl className="kv">
              <dt>Name</dt>
              <dd>{shell.machine_name}</dd>
              <dt>Agent</dt>
              <dd>{shell.agent_ready ? "running" : "not running"}</dd>
            </dl>
            <p className="hint">
              The name is what colleagues see in the lock column when you have a
              profile open, so it is taken from the computer rather than invented.
            </p>
          </div>
        </div>

        <div className="modalFoot">
          <div className="spacer" />
          <button className="primary" onClick={onClose}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
