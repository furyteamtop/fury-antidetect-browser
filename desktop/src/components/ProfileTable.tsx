import type { Me, Profile } from "../api";

/** Every row's controls follow the permissions the SERVER resolved. Hiding a
 *  button is presentation, not protection — the server refuses regardless — but
 *  showing a control that will always fail is its own kind of lie. */
export function ProfileTable({
  profiles,
  me,
  busy,
  onLaunch,
  onRelease,
}: {
  profiles: Profile[];
  me: Me | null;
  busy: boolean;
  onLaunch: (p: Profile, force?: boolean) => void;
  onRelease: (p: Profile) => void;
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
          <th />
        </tr>
      </thead>
      <tbody>
        {profiles.map((p) => {
          const canLaunch = p.permissions.includes("launch");
          const canForce = p.permissions.includes("manage_access");
          const canReveal = p.permissions.includes("reveal_secrets");
          const locked = p.lock !== null;
          // A lock this same user already holds is not contention — it is the
          // profile they are working in. Offering to "take over" from yourself
          // reads as a bug, and worse, it hides the one control that matters
          // (letting go of it).
          const mine = locked && p.lock!.user_id === me?.user_id;

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
                  <span className="warn">No proxy</span>
                )}
              </td>
              <td>
                {!locked && <span className="free">Free</span>}
                {locked && mine && <span className="mineLock">Open here</span>}
                {locked && !mine && (
                  <span className="lock">
                    In use — {p.lock!.user_email} on {p.lock!.machine_name}
                  </span>
                )}
              </td>
              <td className="actions">
                {!locked && canLaunch && (
                  <button disabled={busy} onClick={() => onLaunch(p)}>
                    Open
                  </button>
                )}
                {mine && (
                  <button className="ghost" disabled={busy} onClick={() => onRelease(p)}>
                    Close
                  </button>
                )}
                {locked && !mine && canForce && (
                  <button className="danger" disabled={busy} onClick={() => onLaunch(p, true)}>
                    Take over
                  </button>
                )}
                {locked && !mine && !canForce && (
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
