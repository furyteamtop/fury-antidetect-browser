import { useCallback, useEffect, useState } from "react";
import { useI18n } from "./i18n";
import { api, ApiError, type Me, type Profile, type Project, type Shell } from "./api";
import { Login } from "./components/Login";
import { ProfileDialog } from "./components/ProfileDialog";
import { useAsk } from "./components/Ask";
import { CommandPalette, type Command } from "./components/CommandPalette";
import { ProfileTable } from "./components/ProfileTable";
import { Proxies } from "./components/Proxies";
import { Trash } from "./components/Trash";
import { Team } from "./components/Team";
import { ServerSetup } from "./components/ServerSetup";
import { Enrol } from "./components/Enrol";
import { Settings } from "./components/Settings";
import { Sidebar, type View } from "./components/Sidebar";
import { useTheme } from "./theme";

export function App() {
  // The shell answers what the interface cannot know on its own: whether this
  // installation works alone or against a server, what this machine is called,
  // and whether there is a live session. In local mode there is no session and
  // no account — that is the point of it.
  const [shell, setShell] = useState<Shell | null>(null);
  // Redeeming an invitation is reachable from the sign-in screen and, unlike
  // it, works before this machine has a server configured — the code carries
  // the address. So it is its own state rather than a branch of `signed_in`.
  const [enrolling, setEnrolling] = useState(false);
  const [me, setMe] = useState<Me | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [active, setActive] = useState<Project | null>(null);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<Profile | null | undefined>(undefined);
  const [query, setQuery] = useState("");
  const [tag, setTag] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [view, setView] = useState<View>("profiles");
  const [openOnly, setOpenOnly] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Applied at the root before anything renders, so the first paint is already
  // the right theme rather than a flash of the wrong one.
  useTheme();
  const { t } = useI18n();
  const { ask, dialog: askDialog } = useAsk();

  useEffect(() => {
    void api.shell().then(setShell);
  }, []);

  // ⌘K, or Ctrl+K where there is no command key. Registered on the window so it
  // works from anywhere, including with focus in the search field.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key.toLowerCase() === "k" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        setPaletteOpen((open) => !open);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
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
      // The app opens on every profile, not on whichever project happened to
      // sort first. A project is a filter the operator chooses; choosing one
      // for them hides the rest of their work behind a click they did not know
      // they had to make. A project that disappears while selected drops the
      // filter rather than jumping to another one.
      setActive((current) => (current ? (list.find((p) => p.id === current.id) ?? null) : null));
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
    if (!ready) {
      setProfiles([]);
      return;
    }
    try {
      // Projects come along, because their counts change whenever a profile is
      // created, deleted or restored — including from another window or from
      // the agent directly. Refreshing only the rows left the sidebar showing a
      // number the table contradicted.
      // No project selected means every profile on this machine. That is the
      // Profiles view: the master list an operator actually works from, with a
      // project as a filter over it rather than the only way in.
      const [rows, list] = await Promise.all([
        api.profiles(active?.id),
        api.projects(),
      ]);
      setProfiles(rows);
      setProjects(list);
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

  if (enrolling) {
    return (
      <Enrol
        onDone={() => {
          setEnrolling(false);
          void api.shell().then(setShell);
        }}
        onCancel={() => setEnrolling(false)}
      />
    );
  }

  if (!local && !shell.signed_in) {
    return shell.server_url ? (
      <Login
        onSuccess={() => void api.shell().then(setShell)}
        onEnrol={() => setEnrolling(true)}
      />
    ) : (
      <ServerSetup onDone={setShell} />
    );
  }

  // Signed in, and holding nothing that can open a profile. The session came
  // back from the keychain; the organisation key never does, by design.
  if (!local && !shell.org_key_ready) {
    return (
      <Login
        onSuccess={() => void api.shell().then(setShell)}
        onEnrol={() => setEnrolling(true)}
        unlockFor={shell.last_email}
      />
    );
  }

  const onLaunch = async (profile: Profile, force = false) => {
    setBusy(true);
    setError(null);
    try {
      const res = await api.launch(profile.id, force);
      // Restrictions are the technical half of "an operator cannot take the
      // data home", and they are applied silently. Saying which ones landed is
      // the difference between a browser that behaves oddly and one whose
      // limits were explained.
      const applied = Object.entries(res.restrictions ?? {})
        .filter(([, on]) => on)
        .map(([k]) => k);
      if (applied.length > 0) {
        setNotice(t("app.launchRestricted", { list: applied.join(", ") }));
      }
      await refreshProfiles();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  // Tags in use. Groups were a second, weaker version of this — one label where
  // a tag list allows several — and two ways to sort the same profiles is one
  // too many. Projects are the unit that carries access, so nothing was lost.
  const tags = Array.from(new Set(profiles.flatMap((p) => p.tags))).sort();

  const shown = matching(profiles, query)
    .filter(
      (p) =>
        tag === "" ||
        (tag === "\u0000none" ? p.tags.length === 0 : p.tags.includes(tag)),
    )
    .filter((p) => !openOnly || p.running);

  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });

  const toggleAll = () =>
    setSelected((prev) =>
      // Everything visible, not everything that exists: a search is narrowing
      // on purpose, and selecting rows nobody can see is how bulk deletes go
      // wrong.
      prev.size === shown.length ? new Set() : new Set(shown.map((p) => p.id)),
    );

  const chosen = shown.filter((p) => selected.has(p.id));
  const openable = chosen.filter((p) => !p.running);
  const closable = chosen.filter((p) => p.running);

  /** One at a time, deliberately.
   *
   *  Each launch expands a profile directory, brings up a relay and starts a
   *  browser; ten of those at once turns a laptop into a space heater and makes
   *  every one of them slower. Sequential also means a failure stops the run
   *  instead of burying itself in nine others. */
  const openMany = async () => {
    setBusy(true);
    setError(t("bar.openingMany"));
    try {
      for (const p of openable) {
        await api.launch(p.id);
        await refreshProfiles();
      }
      setError(null);
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

  const commands: Command[] = [
    { id: "act:new", label: t("cmd.newProfile"), run: () => setEditing(null) },
    { id: "act:settings", label: t("cmd.settings"), run: () => setSettingsOpen(true) },
    { id: "act:trash", label: t("cmd.trash"), run: () => setView("trash") },
  ];


  // Lives here rather than in Settings because it needs the active project, the
  // ask() dialog and the notice bar — all of which belong to this screen.
  const exportProject = async () => {
          if (!active) return;
          const path = await ask({
            title: t("ex.exportTitle"),
            detail: t("ex.exportDetail"),
            placeholder: t("ex.path"),
            initial: `${active.name.replace(/[^\w.-]+/g, "-")}.fury`,
            confirmLabel: t("nav.export"),
          });
          if (!path) return;
          const pass = await ask({
            title: t("ex.passphrase"),
            detail: t("ex.exportDetail"),
            placeholder: t("ex.passphrase"),
            confirmLabel: t("nav.export"),
          });
          if (!pass) return;
          try {
            const r = await api.exportProject(active.id, path, pass);
            setNotice(t("ex.done", { kb: Math.round(r.bytes / 1024), path: r.path }));
          } catch (e) {
            setError((e as Error).message);
          }
  };

  const importProject = async () => {
          const path = await ask({
            title: t("ex.importTitle"),
            detail: t("ex.importDetail"),
            placeholder: t("ex.path"),
            confirmLabel: t("nav.import"),
          });
          if (!path) return;
          const pass = await ask({
            title: t("ex.passphrase"),
            placeholder: t("ex.passphrase"),
            confirmLabel: t("nav.import"),
          });
          if (!pass) return;
          try {
            const r = await api.importProject(path, pass);
            setNotice(t("ex.imported", { n: r.profiles }));
            await load();
          } catch (e) {
            setError((e as Error).message);
          }
  };

  return (
    <div className="app">
      {askDialog}
      <Sidebar
        projects={projects}
        active={active}
        shell={shell}
        me={me}
        onSelect={setActive}
        view={view}
        onView={setView}
        onNewProfile={() => setEditing(null)}
        onSettings={() => setSettingsOpen(true)}
        onRenameProject={async (p) => {
          const name = await ask({
            title: t("proj.rename"),
            placeholder: t("proj.newName"),
            initial: p.name,
            confirmLabel: t("ui.save"),
          });
          if (!name) return;
          await api.renameProject(p.id, name);
          await load();
          await refreshProfiles();
        }}
        onDeleteProject={async (p) => {
          const go = await ask({
            title: t("proj.delete"),
            // Naming the number is the difference between a warning someone
            // reads and one they click through.
            detail:
              p.profile_count > 0
                ? t("proj.confirmDelete", { name: p.name, n: p.profile_count })
                : t("proj.confirmDeleteEmpty", { name: p.name }),
            confirmLabel: t("ui.delete"),
            danger: true,
          });
          if (go === null) return;
          await api.deleteProject(p.id);
          setActive(null);
          await load();
          // The rows change too, not just the sidebar: every profile that was
          // in this project is now in none, and until this ran the table went
          // on naming a project that no longer exists.
          await refreshProfiles();
        }}
        onNewProject={async () => {
          const name = await ask({
            title: t("app.newProject"),
            placeholder: t("app.projectName"),
            confirmLabel: t("ui.create"),
          });
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
          <h1>
            {view === "trash"
              ? t("trash.title")
              : view === "proxies"
                ? t("nav.proxies")
                : view === "team"
                  ? t("nav.team")
                  : (active?.name ?? t("nav.profiles"))}
          </h1>
          {view === "profiles" && (
            <span className="muted">{t("app.profileCount", { n: profiles.length })}</span>
          )}
        </header>

        {local && !shell.agent_ready && (
          <div className="notice warnBar" role="status">
            {t("app.agentDown")}
          </div>
        )}

        {notice && (
          <div className="notice" role="status">
            {notice}
            <button className="ghost" onClick={() => setNotice(null)}>
              {t("app.dismiss")}
            </button>
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

        {local && view === "profiles" && (
          <div className="toolbar">
            {/* No "New profile" here. It is the first thing in the sidebar,
                permanently, and a second copy of the primary action a few
                pixels away is two places to look for one decision. */}
            <input
              className="search"
              placeholder={t("bar.search")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            {tags.length > 0 && (
              <select style={{ width: "auto" }} value={tag} onChange={(e) => setTag(e.target.value)}>
                <option value="">{t("bar.allTags")}</option>
                <option value={"\u0000none"}>{t("bar.untagged")}</option>
                {tags.map((x) => (
                  <option key={x} value={x}>
                    {x}
                  </option>
                ))}
              </select>
            )}
            <label className="row" style={{ gap: 6, whiteSpace: "nowrap" }}>
              <input
                type="checkbox"
                style={{ width: 14, height: 14, accentColor: "var(--accent)" }}
                checked={openOnly}
                onChange={(e) => setOpenOnly(e.target.checked)}
              />
              <span className="muted small">
                {t("bar.openOnly", { n: profiles.filter((p) => p.running).length })}
              </span>
            </label>
            <div className="spacer" />
            {chosen.length > 0 && (
              <div className="bulk">
                <span className="muted small">
                  {t("bar.selected", { n: chosen.length })}
                </span>
                {openable.length > 0 && (
                  <button disabled={busy} onClick={openMany}>
                    {t("bar.openSelected", { n: openable.length })}
                  </button>
                )}
                {closable.length > 0 && (
                  <button
                    className="ghost"
                    disabled={busy}
                    onClick={async () => {
                      setBusy(true);
                      for (const p of closable) await api.stop(p.id);
                      await refreshProfiles();
                      setBusy(false);
                    }}
                  >
                    {t("bar.closeSelected", { n: closable.length })}
                  </button>
                )}
                <button
                  className="ghost"
                  disabled={busy || closable.length > 0}
                  onClick={async () => {
                    const go = await ask({
                      title: t("bar.deleteSelected", { n: chosen.length }),
                      detail: t("bar.confirmDeleteMany", { n: chosen.length }),
                      confirmLabel: t("ui.delete"),
                      danger: true,
                    });
                    if (go === null) return;
                    setBusy(true);
                    for (const p of chosen) await api.deleteProfile(p.id);
                    setSelected(new Set());
                    await load();
                    await refreshProfiles();
                    setBusy(false);
                  }}
                >
                  {t("bar.deleteSelected", { n: chosen.length })}
                </button>
                {local && (
                  <select
                    style={{ width: "auto" }}
                    value=""
                    disabled={busy}
                    onChange={async (e) => {
                      const to = e.target.value;
                      if (to === "") return;
                      setBusy(true);
                      try {
                        // "\u0000none" rather than "" for out-of-every-project:
                        // an empty value is the placeholder, and the two must
                        // not be the same string or picking one does nothing.
                        await api.moveProfiles(
                          chosen.map((p) => p.id),
                          to === "\u0000none" ? null : to,
                        );
                        setSelected(new Set());
                        await load();
                        await refreshProfiles();
                      } catch (err) {
                        setError((err as Error).message);
                      } finally {
                        setBusy(false);
                      }
                    }}
                  >
                    <option value="">{t("bar.moveTo")}</option>
                    {projects.map((p) => (
                      <option key={p.id} value={p.id}>
                        {p.name}
                      </option>
                    ))}
                    <option value={"\u0000none"}>{t("bar.moveOut")}</option>
                  </select>
                )}
                <button className="ghost" onClick={() => setSelected(new Set())}>
                  {t("bar.clearSelection")}
                </button>
              </div>
            )}
            <button className="ghost" onClick={() => void refreshProfiles()}>
              {t("bar.refresh")}
            </button>
          </div>
        )}

        {/* No project selected is not the same as a project with no profiles,
            and saying the latter to someone who has been granted nothing sends
            them looking for a profile list that was never theirs. */}
        {view === "proxies" && <Proxies profiles={profiles} />}

        {view === "team" && <Team projects={projects} />}

        {view === "trash" && (
          <Trash
            onChanged={async () => {
              await load();
              await refreshProfiles();
            }}
          />
        )}

        {view === "profiles" && (
          <div className="tableWrap">
            <ProfileTable
              profiles={shown}
              showProject={active === null}
              selected={selected}
              onToggle={toggle}
              onToggleAll={toggleAll}
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
                      const go = await ask({
                        title: t("row.delete"),
                        detail: t("row.confirmDelete", { name: p.name }),
                        confirmLabel: t("ui.delete"),
                        danger: true,
                      });
                      if (go === null) return;
                      await api.deleteProfile(p.id);
                      await refreshProfiles();
                    }
                  : undefined
              }
            />
          </div>
        )}

        {paletteOpen && (
          <CommandPalette
            actions={[
              ...commands,
              ...profiles.map((p) => ({
                id: `run:${p.id}`,
                label: `${p.running ? t("cmd.closeProfile") : t("cmd.openProfile")} ${p.name}`,
                hint: p.proxy?.display,
                run: () => void (p.running ? onStop(p) : onLaunch(p)),
              })),
              ...projects.map((p) => ({
                id: `go:${p.id}`,
                label: p.name,
                hint: t("app.projects"),
                run: () => {
                  setActive(p);
                  setView("profiles");
                },
              })),
            ]}
            onClose={() => setPaletteOpen(false)}
          />
        )}

        {settingsOpen && (
          <Settings
            shell={shell}
            hasProject={active !== null}
            onExport={exportProject}
            onImport={importProject}
            onChanged={setShell}
            onEnrol={() => {
              setSettingsOpen(false);
              setEnrolling(true);
            }}
            onClose={() => setSettingsOpen(false)}
          />
        )}

        {editing !== undefined && (
          <ProfileDialog
            projectId={active?.id ?? null}
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
