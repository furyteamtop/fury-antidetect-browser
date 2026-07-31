import { useCallback, useEffect, useState } from "react";
import { useI18n } from "./i18n";
import { api, ApiError, type Me, type Profile, type Project, type Shell } from "./api";
import { Login } from "./components/Login";
import { ProfileDialog } from "./components/ProfileDialog";
import { ProfileTable } from "./components/ProfileTable";
import { ProxyDialog } from "./components/ProxyDialog";
import { ServerSetup } from "./components/ServerSetup";
import { Settings } from "./components/Settings";
import { Sidebar } from "./components/Sidebar";
import { useTheme } from "./theme";

export function App() {
  // The shell answers what the interface cannot know on its own: whether this
  // installation works alone or against a server, what this machine is called,
  // and whether there is a live session. In local mode there is no session and
  // no account — that is the point of it.
  const [shell, setShell] = useState<Shell | null>(null);
  const [me, setMe] = useState<Me | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [active, setActive] = useState<Project | null>(null);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<Profile | null | undefined>(undefined);
  const [query, setQuery] = useState("");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [proxyOpen, setProxyOpen] = useState(false);
  // Applied at the root before anything renders, so the first paint is already
  // the right theme rather than a flash of the wrong one.
  useTheme();
  const { t } = useI18n();

  useEffect(() => {
    void api.shell().then(setShell);
  }, []);

  const local = shell?.mode === "local";
  // Local mode has nobody to sign in as. Team mode does, and until it happens
  // there is nothing to show.
  const ready = shell !== null && (local || shell.signed_in);

  const load = useCallback(async () => {
    setError(null);
    try {
      const list = await api.projects();
      setProjects(list);
      // Open straight into a project: the list of profiles is the application,
      // and any screen between launching and it is friction (docs/12).
      setActive((current) =>
        current ? (list.find((p) => p.id === current.id) ?? list[0] ?? null) : (list[0] ?? null),
      );
      // Identity only exists with a server.
      setMe(local ? null : await api.me());
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) void api.shell().then(setShell);
      else setError((e as Error).message);
    }
  }, [local]);

  useEffect(() => {
    if (ready) void load();
  }, [ready, load]);

  const refreshProfiles = useCallback(async () => {
    if (!active || !ready) {
      setProfiles([]);
      return;
    }
    try {
      setProfiles(await api.profiles(active.id));
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) void api.shell().then(setShell);
      else setError((e as Error).message);
    }
  }, [active, ready]);

  useEffect(() => {
    void refreshProfiles();
    // A browser can be closed from its own window, and in team mode a colleague
    // can take or release a lock. Either way the table has to notice without
    // being asked, or "is this open?" becomes a chat message — the problem the
    // column exists to remove.
    const timer = setInterval(() => void refreshProfiles(), 5_000);
    return () => clearInterval(timer);
  }, [refreshProfiles]);

  if (!shell) return <div className="splash">Fury</div>;

  if (!local && !shell.signed_in) {
    return shell.server_url ? (
      <Login onSuccess={() => void api.shell().then(setShell)} />
    ) : (
      <ServerSetup onDone={setShell} />
    );
  }

  const onLaunch = async (profile: Profile, force = false) => {
    setBusy(true);
    setError(null);
    try {
      const res = await api.launch(profile.id, force);
      if (!res.launched) {
        // Team mode: the agent cannot fetch a bundle from a server yet, so all
        // that happened was taking the lock. Saying "opening…" would leave the
        // operator waiting for a window that is not coming.
        const applied = Object.entries(res.restrictions ?? {})
          .filter(([, on]) => on)
          .map(([k]) => k);
        setError(
          `Lock taken; it lapses at ${
            res.expires_at ? new Date(res.expires_at).toLocaleTimeString() : "soon"
          } and nothing is renewing it yet. Profiles from a server cannot be launched ` +
            `yet — that needs bundle sync. Restrictions it would apply: ` +
            `${applied.length ? applied.join(", ") : "none"}.`,
        );
      }
      await refreshProfiles();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const onStop = async (profile: Profile) => {
    setBusy(true);
    try {
      await api.stop(profile.id);
      await refreshProfiles();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="app">
      <Sidebar
        projects={projects}
        active={active}
        shell={shell}
        me={me}
        onSelect={setActive}
        onSettings={() => setSettingsOpen(true)}
        onNewProject={async () => {
          const name = prompt(t("app.projectName"))?.trim();
          if (!name) return;
          await api.createProject(name);
          await load();
        }}
        onSignOut={async () => {
          await api.logout();
          setMe(null);
          setProjects([]);
          setActive(null);
          setShell(await api.shell());
        }}
      />
      <main className="main">
        <header className="head">
          <h1>{active?.name ?? t("app.noProjects")}</h1>
          <span className="muted">
            {active ? t("app.profileCount", { n: profiles.length }) : t("app.nothingYet")}
          </span>
        </header>

        {local && !shell.agent_ready && (
          <div className="notice warnBar" role="status">
            {t("app.agentDown")}
          </div>
        )}

        {error && (
          <div className="notice" role="status">
            {error}
            <button className="ghost" onClick={() => setError(null)}>
              {t("app.dismiss")}
            </button>
          </div>
        )}

        {active && local && (
          <div className="toolbar">
            <button className="primary" onClick={() => setEditing(null)}>
              {t("bar.newProfile")}
            </button>
            <input
              className="search"
              placeholder={t("bar.search")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <div className="spacer" />
            <button className="ghost" onClick={() => setProxyOpen(true)}>
              {t("bar.addProxy")}
            </button>
            <button className="ghost" onClick={() => void refreshProfiles()}>
              {t("bar.refresh")}
            </button>
          </div>
        )}

        {/* No project selected is not the same as a project with no profiles,
            and saying the latter to someone who has been granted nothing sends
            them looking for a profile list that was never theirs. */}
        {active && (
          <div className="tableWrap">
            <ProfileTable
              profiles={matching(profiles, query)}
              me={me}
              thisMachine={shell.machine_name}
              local={local}
              busy={busy}
              onLaunch={onLaunch}
              onStop={onStop}
              onEdit={local ? setEditing : undefined}
              onDelete={
                local
                  ? async (p) => {
                      if (!confirm(t("row.confirmDelete", { name: p.name }))) return;
                      await api.deleteProfile(p.id);
                      await refreshProfiles();
                    }
                  : undefined
              }
            />
          </div>
        )}

        {proxyOpen && (
          <ProxyDialog
            editing={null}
            onClose={() => setProxyOpen(false)}
            onSaved={() => setProxyOpen(false)}
          />
        )}

        {settingsOpen && (
          <Settings shell={shell} onChanged={setShell} onClose={() => setSettingsOpen(false)} />
        )}

        {editing !== undefined && active && (
          <ProfileDialog
            projectId={active.id}
            editing={editing}
            onClose={() => setEditing(undefined)}
            onSaved={async () => {
              setEditing(undefined);
              await load();
              await refreshProfiles();
            }}
          />
        )}
      </main>
    </div>
  );
}

/** Search across the three things someone actually looks a profile up by. */
function matching(profiles: Profile[], query: string): Profile[] {
  const q = query.trim().toLowerCase();
  if (!q) return profiles;
  return profiles.filter((p) =>
    [p.name, p.persona_id, p.proxy?.display ?? "", ...p.tags]
      .join(" ")
      .toLowerCase()
      .includes(q),
  );
}
