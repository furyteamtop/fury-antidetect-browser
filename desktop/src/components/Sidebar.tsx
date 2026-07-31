import type { Me, Project, Shell } from "../api";

export function Sidebar({
  projects,
  active,
  shell,
  me,
  onSelect,
  onSignOut,
}: {
  projects: Project[];
  active: Project | null;
  shell: Shell;
  me: Me | null;
  onSelect: (p: Project) => void;
  onSignOut: () => void;
}) {
  return (
    <aside className="sidebar">
      <div className="brand">Fury</div>

      <nav>
        <div className="section">Projects</div>
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
          {shell.server_url ?? "no server"}
        </div>
        <div className="muted small ellipsis" title={shell.machine_name}>
          {shell.machine_name}
          {!shell.native && " · browser dev"}
        </div>
        <button className="ghost" onClick={onSignOut}>
          Sign out
        </button>
      </div>
    </aside>
  );
}
