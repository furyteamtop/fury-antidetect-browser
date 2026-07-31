import type { Me, Project, Shell } from "../api";

export function Sidebar({
  projects,
  active,
  shell,
  me,
  onSelect,
  onNewProject,
  onSignOut,
}: {
  projects: Project[];
  active: Project | null;
  shell: Shell;
  me: Me | null;
  onSelect: (p: Project) => void;
  onNewProject: () => void;
  onSignOut: () => void;
}) {
  const local = shell.mode === "local";
  return (
    <aside className="sidebar">
      <div className="brand">Fury</div>

      <nav>
        <div className="section">
          Projects
          {local && (
            <button className="linky" onClick={onNewProject} title="New project">
              +
            </button>
          )}
        </div>
        {projects.length === 0 && <div className="empty">Nothing shared with you</div>}
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
          {local ? "Working locally · no account" : shell.server_url}
        </div>
        <div className="muted small ellipsis" title={shell.machine_name}>
          {shell.machine_name}
          {!shell.native && " · browser dev"}
        </div>
        {/* Nothing to sign out of when there is no server. Offering it anyway
            would imply an account exists somewhere. */}
        {!local && (
          <button className="ghost" onClick={onSignOut}>
            Sign out
          </button>
        )}
      </div>
    </aside>
  );
}
