import { useI18n } from "../i18n";
import type { Me, Project, Shell } from "../api";

export function Sidebar({
  projects,
  active,
  shell,
  me,
  onSelect,
  onNewProject,
  onSettings,
  onSignOut,
}: {
  projects: Project[];
  active: Project | null;
  shell: Shell;
  me: Me | null;
  onSelect: (p: Project) => void;
  onNewProject: () => void;
  onSettings: () => void;
  onSignOut: () => void;
}) {
  const { t } = useI18n();
  const local = shell.mode === "local";
  return (
    <aside className="sidebar">
      <div className="brand">Fury</div>

      <nav>
        <div className="section">
          {t("app.projects")}
          {local && (
            <button className="linky" onClick={onNewProject} title={t("app.newProject")}>
              +
            </button>
          )}
        </div>
        {projects.length === 0 && <div className="empty">{t("app.nothingShared")}</div>}
        {projects.map((p) => (
          <button
            key={p.id}
            className={p.id === active?.id ? "nav active" : "nav"}
            onClick={() => onSelect(p)}
          >
            <span>{p.name}</span>
            <span className="count">{p.profile_count}</span>
          </button>
        ))}
      </nav>

      <div className="foot">
        {/* docs/12 wants connection state permanently visible. Who and where
            matter for the same reason: on a machine that can reach two
            organisations, sending work to the wrong one is a mistake that only
            surfaces much later. */}
        {me && <div className="who ellipsis">{me.email}</div>}
        <div className="muted small ellipsis" title={shell.server_url ?? ""}>
          {local ? t("app.workingLocally") : shell.server_url}
        </div>
        <div className="muted small ellipsis" title={shell.machine_name}>
          {shell.machine_name}
          {!shell.native && ` · ${t("app.browserDev")}`}
        </div>
        <button className="ghost" onClick={onSettings}>
          {t("app.settings")}
        </button>
        {/* Nothing to sign out of when there is no server. Offering it anyway
            would imply an account exists somewhere. */}
        {!local && (
          <button className="ghost" onClick={onSignOut}>
            {t("app.signOut")}
          </button>
        )}
      </div>
    </aside>
  );
}
