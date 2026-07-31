import { useCallback, useEffect, useState } from "react";
import { api, ApiError, storedToken, type Me, type Profile, type Project } from "./api";
import { Login } from "./components/Login";
import { ProfileTable } from "./components/ProfileTable";
import { Sidebar } from "./components/Sidebar";

export function App() {
  const [authed, setAuthed] = useState(() => storedToken() !== null);
  const [me, setMe] = useState<Me | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [active, setActive] = useState<Project | null>(null);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    setError(null);
    try {
      // Identity and projects together: the table cannot tell "your lock" from
      // "someone else's" until it knows which user this session belongs to.
      const [who, list] = await Promise.all([api.me(), api.projects()]);
      setMe(who);
      setProjects(list);
      // Open straight into a project: the list of profiles is the application,
      // and any screen between launching and it is friction (docs/12).
      setActive((current) =>
        current ? (list.find((p) => p.id === current.id) ?? list[0] ?? null) : (list[0] ?? null),
      );
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAuthed(false);
      else setError(String((e as Error).message));
    }
  }, []);

  useEffect(() => {
    if (authed) void load();
  }, [authed, load]);

  const refreshProfiles = useCallback(async () => {
    if (!active) {
      setProfiles([]);
      return;
    }
    try {
      setProfiles(await api.profiles(active.id));
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) setAuthed(false);
      else setError(String((e as Error).message));
    }
  }, [active]);

  useEffect(() => {
    void refreshProfiles();
    // Lock state changes when a colleague opens or closes a profile, so the
    // list has to keep up on its own — otherwise "who has this open?" becomes a
    // chat message, which is the problem the lock column exists to remove.
    const timer = setInterval(() => void refreshProfiles(), 10_000);
    return () => clearInterval(timer);
  }, [refreshProfiles]);

  if (!authed) {
    return <Login onSuccess={() => setAuthed(true)} />;
  }

  const onLaunch = async (profile: Profile, force = false) => {
    setBusy(true);
    setError(null);
    try {
      const res = await api.lock(profile.id, force);
      const applied = Object.entries(res.restrictions)
        .filter(([, on]) => on)
        .map(([k]) => k);
      // The agent is not wired up yet, so say exactly that rather than
      // pretending a browser opened.
      setError(
        `Lock held until ${new Date(res.expires_at).toLocaleTimeString()}. ` +
          `The local agent is not connected yet, so nothing was launched. ` +
          `Restrictions it would apply: ${applied.length ? applied.join(", ") : "none"}.`,
      );
      await refreshProfiles();
    } catch (e) {
      setError(String((e as Error).message));
    } finally {
      setBusy(false);
    }
  };

  const onRelease = async (profile: Profile) => {
    setBusy(true);
    try {
      await api.unlock(profile.id);
      await refreshProfiles();
    } catch (e) {
      setError(String((e as Error).message));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="app">
      <Sidebar
        projects={projects}
        active={active}
        onSelect={setActive}
        onSignOut={async () => {
          await api.logout();
          setMe(null);
          setAuthed(false);
        }}
      />
      <main className="main">
        <header className="head">
          <h1>{active?.name ?? "No projects"}</h1>
          <span className="muted">
            {active ? `${profiles.length} profiles` : "Ask an admin for access"}
          </span>
        </header>

        {error && (
          <div className="notice" role="status">
            {error}
            <button className="ghost" onClick={() => setError(null)}>
              Dismiss
            </button>
          </div>
        )}

        {/* No project selected is not the same as a project with no profiles,
            and saying the latter to someone who has been granted nothing sends
            them looking for a profile list that was never theirs. */}
        {active && (
          <ProfileTable
            profiles={profiles}
            me={me}
            busy={busy}
            onLaunch={onLaunch}
            onRelease={onRelease}
          />
        )}
      </main>
    </div>
  );
}
