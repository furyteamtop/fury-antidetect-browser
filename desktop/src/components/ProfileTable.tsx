import type { Me, Profile } from "../api";

/** Every row's controls follow the permissions the SERVER resolved. Hiding a
 *  button is presentation, not protection — the server refuses regardless — but
 *  showing a control that will always fail is its own kind of lie.
 *
 *  In local mode there are no permissions to resolve, and the state that
 *  matters is different: not "who holds this" but "is it open right now". */
export function ProfileTable({
  profiles,
  me,
  thisMachine,
  local,
  busy,
  onLaunch,
  onStop,
}: {
  profiles: Profile[];
  me: Me | null;
  thisMachine: string;
  local: boolean;
  busy: boolean;
  onLaunch: (p: Profile, force?: boolean) => void;
  onStop: (p: Profile) => void;
}) {
  if (profiles.length === 0) {
    return <p className="empty pad">No profiles in this project yet.</p>;
  }

  return (
    <table className="grid">
      <thead>
        <tr>
          <th>Name</th>
          <th>Persona</th>
          <th>Proxy</th>
          <th>Status</th>
          <th>Last opened</th>
          <th />
        </tr>
      </thead>
      <tbody>
        {profiles.map((p) => {
          const canLaunch = p.permissions.includes("launch");
          const canForce = p.permissions.includes("manage_access");
          const canReveal = p.permissions.includes("reveal_secrets");
          const locked = p.lock !== null;
          // "Mine" means this user *on this machine*. Matching on the user
          // alone was wrong: the same person signed in on a laptop and a
          // desktop would see the laptop's live lock labelled "Open here", with
          // a Close button that worked — releasing a lock whose browser was
          // still running on the other machine.
          const mine =
            locked && p.lock!.user_id === me?.user_id && p.lock!.machine_name === thisMachine;
          const open = local ? p.running : mine;

          return (
            <tr key={p.id}>
              <td>
                <div className="name">{p.name}</div>
                {p.tags.length > 0 && (
                  <div className="tags">{p.tags.map((t) => <span key={t}>{t}</span>)}</div>
                )}
              </td>
              <td className="mono muted">{p.persona_id}</td>
              <td>
                {p.proxy ? (
                  <>
                    <div className="mono">{p.proxy.display}</div>
                    <div className="muted small">
                      {p.proxy.country ?? "?"}
                      {/* The server masks the host for anyone without
                          reveal_secrets; say so, or a masked value reads like a
                          bug rather than a boundary. */}
                      {!canReveal && " · masked"}
                    </div>
                  </>
                ) : (
                  // Not cosmetic: the agent refuses to launch without one,
                  // because everything the core does goes through the relay.
                  <span className="warn">No proxy</span>
                )}
              </td>
              <td>
                {open && <span className="mineLock">Open</span>}
                {!open && locked && (
                  <span className="lock">
                    In use — {p.lock!.user_email} on {p.lock!.machine_name}
                  </span>
                )}
                {!open && !locked && <span className="free">Idle</span>}
              </td>
              <td className="muted small">
                {/* docs/12: the metric that matters to someone running accounts
                    is which profiles have gone stale. A profile untouched for
                    two months behaves differently from a live one. */}
                {p.last_opened_at ? new Date(p.last_opened_at).toLocaleString() : "never"}
              </td>
              <td className="actions">
                {!open && !locked && canLaunch && (
                  <button disabled={busy} onClick={() => onLaunch(p)}>
                    Open
                  </button>
                )}
                {open && (
                  <button className="ghost" disabled={busy} onClick={() => onStop(p)}>
                    Close
                  </button>
                )}
                {!open && locked && canForce && (
                  <button className="danger" disabled={busy} onClick={() => onLaunch(p, true)}>
                    Take over
                  </button>
                )}
                {!open && locked && !canForce && (
                  <span className="muted small">Ask them to close it</span>
                )}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}
