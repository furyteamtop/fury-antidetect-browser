import type { Project } from "../api";

export function Sidebar({
  projects,
  active,
  onSelect,
  onSignOut,
}: {
  projects: Project[];
  active: Project | null;
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
        {/* Quotas and connection state belong here — an operator has to know
            whether they are working online or from a stale cache (docs/12). */}
        <button className="ghost" onClick={onSignOut}>
          Sign out
        </button>
      </div>
    </aside>
  );
}
