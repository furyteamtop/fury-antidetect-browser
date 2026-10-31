// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useCallback, useEffect, useRef, useState } from "react";
import { useI18n, type Key } from "./i18n";
import { api, ApiError, type LocalProxy, type Me, type MirrorStatus, type Profile, type Project, type Shell, type UpdateProgress, type WarmProgress, type Farm } from "./api";
import { Login } from "./components/Login";
import { ProfileDialog } from "./components/ProfileDialog";
import { BulkProfiles } from "./components/BulkProfiles";
import { Cookies } from "./components/Cookies";
import { Extensions } from "./components/Extensions";
import { NetworkReport } from "./components/NetworkReport";
import { ExtensionsView } from "./components/ExtensionsView";
import { AssistantsView } from "./components/AssistantsView";
import { WarmDialog } from "./components/WarmDialog";
import { useAsk } from "./components/Ask";
import { CommandPalette, type Command } from "./components/CommandPalette";
import { ProfileTable, isOpenHere } from "./components/ProfileTable";
import { Proxies } from "./components/Proxies";
import { Trash } from "./components/Trash";
import { Users } from "./components/Users";
import { ServerSetup } from "./components/ServerSetup";
import { Enrol } from "./components/Enrol";
import { Signup } from "./components/Signup";
import { Settings } from "./components/Settings";
import { Sidebar, type View } from "./components/Sidebar";
import { ShareDialog } from "./components/ShareDialog";
import { CsvImport } from "./components/CsvImport";
import { AntidetectImport } from "./components/AntidetectImport";
import { IconButton } from "./components/Icon";
import { useTheme } from "./theme";
import { exitSharing } from "./consistency";
import { isSuggested, setStage as saveStage } from "./status";
import { Select } from "./components/Select";
import { OUR_SERVERS, versionNewer } from "./defaults";

/** What the browser build the agent asks for changes, for the bar that offers
 *  it. One sentence for every build told a Mac being offered 0.3.8 about a
 *  taskbar icon, canvas fixes and "about 150 MB" (13.10.2026). A build with
 *  no line of its own gets a plain one; the size is shown by the download. */
const CORE_WHAT: Record<string, Key> = {
  "0.4.14": "app.coreWhat0414",
  "0.4.12": "app.coreWhat0412",
  "0.3.8": "app.coreWhat038",
  "0.3.7": "app.coreWhat037",
};
function coreWhat(need: string): Key {
  return CORE_WHAT[need] ?? "app.coreWhatOther";
}

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
  // Held only between the click and the first poll that sees the download
  // running, so one button press cannot become two downloads.
  const [coreBusy, setCoreBusy] = useState(false);
  const [signingUp, setSigningUp] = useState(false);
  const [me, setMe] = useState<Me | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const [active, setActive] = useState<Project | null>(null);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  // For the row verdicts (consistency.ts): which personas exist, and which
  // exit each proxy was last seen coming out of. Both are cheap lists the
  // agent already keeps; refreshed with the rows.
  const [personaIds, setPersonaIds] = useState<Set<string> | null>(null);
  const [proxyList, setProxyList] = useState<LocalProxy[] | null>(null);
  const [shared, setShared] = useState<Profile[]>([]);
  /** The profiles a share dialog is open for, or null. */
  const [sharing, setSharing] = useState<Profile[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Profiles whose launch is in flight. Per profile, not the global `busy`:
  // a team launch can take minutes (it pulls the profile from the server
  // first), and holding every button in the list for that long read as the
  // application having frozen -- "the Open button does not press", 27.09.2026.
  const [launching, setLaunching] = useState<Set<string>>(new Set());
  // The release feed's answer, and the version the operator said "later" to.
  const [update, setUpdate] = useState<Awaited<ReturnType<typeof api.checkUpdate>> | null>(null);
  const [dismissedUpdate, setDismissedUpdate] = useState<string | null>(() => {
    try {
      return localStorage.getItem("fury.update.dismissed");
    } catch {
      return null;
    }
  });
  // The team server's version against the newest release, for the people
  // who can update it: owners and admins of a server that is not ours. The
  // tester who runs his own heard of server updates only when told
  // (14.10.2026). A server without `server_version` predates 0.4.2.
  const [dismissedServer, setDismissedServer] = useState<string | null>(() => {
    try {
      return localStorage.getItem("fury.serverUpdate.dismissed");
    } catch {
      return null;
    }
  });
  // An in-app install under way (src-tauri/update.rs), and why one stopped.
  const [installing, setInstalling] = useState<UpdateProgress | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);
  const installUpdate = async () => {
    if (!update) return;
    setInstallError(null);
    setInstalling({ stage: "checking", received: 0, total: null, error: null });
    const poll = setInterval(() => {
      api.updateProgress().then(setInstalling, () => {});
    }, 300);
    try {
      await api.installUpdate();
    } catch (e: any) {
      const code = e instanceof ApiError ? e.code : null;
      if (code === "no_signed_update") {
        // A release without a signed build: the link, as before 0.3.0.
        void api.openUrl(update.download ?? update.url ?? "");
      } else if (code === "profiles_open") {
        setInstallError(t("app.updateCloseProfiles"));
      } else {
        setInstallError(t("app.updateFailed", { message: e?.message ?? String(e) }));
      }
      setInstalling(null);
    } finally {
      clearInterval(poll);
    }
  };
  useEffect(() => {
    let off = false;
    try {
      off = localStorage.getItem("fury.update.auto") === "off";
    } catch {
      /* default: on */
    }
    if (off) return;
    const check = () => api.checkUpdate().then(setUpdate, () => {});
    void check();
    const every = setInterval(check, 6 * 60 * 60 * 1000);
    return () => clearInterval(every);
  }, []);
  const [editing, setEditing] = useState<Profile | null | undefined>(undefined);
  const [query, setQuery] = useState("");
  const [tag, setTag] = useState("");
  // The stage filter (5.5): "" is every stage, "\u0000none" is no stage.
  const [stage, setStage] = useState("");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  /** `undefined` closed, `null` making new ones, a profile means copying it. */
  const [bulk, setBulk] = useState<Profile | null | undefined>(undefined);
  const [csv, setCsv] = useState(false);
  const [gologin, setGologin] = useState(false);
  const [cookiesFor, setCookiesFor] = useState<Profile | null>(null);
  const [extFor, setExtFor] = useState<Profile | null>(null);
  const [netFor, setNetFor] = useState<Profile | null>(null);
  // The synchronised group, polled while it runs so the banner's counter and
  // membership stay true to the agent rather than to the click that started it.
  const [mirror, setMirror] = useState<MirrorStatus | null>(null);
  const [mirrorNote, setMirrorNote] = useState<string | null>(null);
  // Warm-ups in progress, polled while any runs.
  const [warmFor, setWarmFor] = useState<Profile[] | null>(null);
  const [warm, setWarm] = useState<WarmProgress[]>([]);
  useEffect(() => {
    if (!shell?.agent_ready) return;
    let stop = false;
    const tick = async () => {
      try {
        const s = await api.warmStatus();
        if (!stop) setWarm(s);
      } catch {
        if (!stop) setWarm([]);
      }
    };
    void tick();
    const h = setInterval(() => void tick(), 3000);
    return () => {
      stop = true;
      clearInterval(h);
    };
  }, [shell?.agent_ready]);
  // Farming plans (agent/src/farm.rs). The agent starts local sessions on
  // its own; a team one needs this window -- the lock and the keys are here --
  // so while it is open it claims the due ones and starts them the way the
  // Warm button does. It also sets the stage a finished profile asked for.
  const [farms, setFarms] = useState<{ now_ms: number; farms: Farm[] } | null>(null);
  const profilesNow = useRef<Profile[]>([]);
  profilesNow.current = profiles;
  useEffect(() => {
    if (!shell?.agent_ready) return;
    let stop = false;
    const tick = async () => {
      try {
        for (const d of await api.farmDue()) {
          let plan;
          try {
            plan = await api.farmClaim(d.farm_id, d.profile_id);
          } catch {
            continue; // no longer due: another window, or the hours ran out
          }
          try {
            const r = await api.warmStart([{ id: d.profile_id, origin: d.origin }], plan);
            const refused = r.refused.find((x) => x.id === d.profile_id);
            if (refused) await api.farmFail(d.farm_id, d.profile_id, say({ code: refused.code, message: refused.reason }));
          } catch (e) {
            await api.farmFail(d.farm_id, d.profile_id, say(e));
          }
        }
        const list = await api.farmList();
        for (const f of list.farms) {
          for (const [pid, tr] of Object.entries(f.tracks)) {
            if (!tr.stage_pending) continue;
            const p = profilesNow.current.find((x) => x.id === pid);
            if (!p) continue; // not in the list being shown; next time
            // Only over the stage the plan set: one the operator changed by
            // hand since ("banned") is theirs.
            if ((p.status ?? "") === "warming") {
              try {
                await saveStage(p, tr.stage_pending);
              } catch {
                continue;
              }
            }
            await api.farmStageApplied(f.id, pid);
          }
        }
        if (!stop) setFarms(list.farms.length > 0 ? list : null);
      } catch {
        if (!stop) setFarms(null);
      }
    };
    void tick();
    const h = setInterval(() => void tick(), 20_000);
    return () => {
      stop = true;
      clearInterval(h);
    };
  }, [shell?.agent_ready]);
  const refreshFarms = async () => {
    const list = await api.farmList();
    setFarms(list.farms.length > 0 ? list : null);
  };
  useEffect(() => {
    if (!shell?.agent_ready) return;
    let stop = false;
    const tick = async () => {
      try {
        const s = await api.mirrorStatus();
        if (!stop) setMirror(s.active ? s : null);
      } catch {
        if (!stop) setMirror(null);
      }
    };
    void tick();
    const h = setInterval(() => void tick(), 2000);
    return () => {
      stop = true;
      clearInterval(h);
    };
  }, [shell?.agent_ready]);
  const [view, setView] = useState<View>("profiles");
  const [openOnly, setOpenOnly] = useState(false);
  /** A line of information at the top of the window, and — when it is about a
   *  launch — which profile it is about.
   *
   *  One piece of state rather than two, so the sentence and the thing it
   *  describes cannot get out of step. `forProfile` is null for a notice with
   *  no lifetime: an export that finished, an import that landed. */
  const [notice, setNotice] = useState<{ text: string; forProfile: string | null } | null>(null);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  /** Which settings tab opens. "Connect a server" in Users means the team
   *  tab; it opened on General, one more click away from the only field that
   *  matters there, and a tester could not find where to sign in (14.10.2026). */
  const [settingsTab, setSettingsTab] = useState<"general" | "team">("general");
  // Applied at the root before anything renders, so the first paint is already
  // the right theme rather than a flash of the wrong one.
  useTheme();
  const { t, say } = useI18n();
  const { ask, dialog: askDialog } = useAsk();

  const [fatal, setFatal] = useState<string | null>(null);

  useEffect(() => {
    // Caught, because this is the one call with nothing behind it. Without a
    // catch a failure here left the splash on screen for ever — an application
    // that looks like it is still starting, and never will.
    api.shell().then(setShell, (e) => setFatal(say(e)));
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
  // Where a NEW profile is made: the open project's world, else the shell's.
  // With no project open on a connected shell the dialogs ask instead.
  const newOrigin: "local" | "team" =
    active?.origin === "local" || active?.origin === "team" ? active.origin : local ? "local" : "team";
  const canChooseOrigin = !local && !active;
  // The CSV and anti-detect imports file profiles on this computer.
  const importsHere = local || !active || active.origin === "local";
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
      else setError(say(e));
    }
  }, [local]);

  useEffect(() => {
    if (ready) void load();
  }, [ready, load]);

  // `scope` overrides which project's rows to load, for the one case the state
  // cannot answer: a caller that has just deleted the active project. React
  // applies setActive on the next render, so this callback still closes over
  // the old value and would ask the server for a project that no longer
  // exists — which answers 404, and 404 renders as "not found, or you no
  // longer have access" next to a delete that in fact worked.
  const refreshProfiles = useCallback(async (scope?: string | null) => {
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
      const [rows, list, current] = await Promise.all([
        api.profiles(
          scope === undefined ? active?.id : (scope ?? undefined),
          // Which side the chosen folder is on. A local folder's contents come
          // from the agent; asking the server about it returns "not found"
          // about a project it has never heard of.
          scope === undefined ? active?.origin : projects.find((p) => p.id === scope)?.origin,
        ),
        api.projects(),
        // The shell too, on every poll rather than only at startup.
        //
        // shell_state is what restarts the agent if it died, and it was called
        // once, on mount. So an agent that stopped while the app was open — a
        // crash, a machine waking from sleep — left the warning on screen for
        // ever and every persona list, launch and proxy call failing, with
        // nothing trying to fix it.
        api.shell(),
      ]);
      setProfiles(rows);
      setProjects(list);
      setShell(current);
      // Not awaited with the rows: a failure here costs the verdicts, not
      // the list.
      void api.personas().then((ps) => setPersonaIds(new Set(ps.map((x) => x.id))), () => {});
      void api.proxies().then(setProxyList, () => {});
      // A launch notice outlives its launch by exactly one poll.
      //
      // Here rather than in the Close button, because the browser is closed
      // from its own window at least as often as from ours, and a message that
      // only clears when you press the right button is the same bug with a
      // narrower door. A profile that has left the list entirely counts as
      // closed too.
      setNotice((n) => {
        if (n?.forProfile == null) return n;
        const row = rows.find((p) => p.id === n.forProfile);
        const stillOpen =
          row !== undefined &&
          isOpenHere(row, {
            local: current.mode === "local",
            userId: me?.user_id,
            machine: current.machine_name,
          });
        return stillOpen ? n : null;
      });
      // Borrowed profiles come from a different endpoint and a different
      // account's organisation, so they are fetched separately and their
      // failure is not allowed to empty the main list -- a server that answers
      // about your own profiles and stumbles on somebody else's grant should
      // cost you the borrowed ones only.
      if (current.mode !== "local") {
        try {
          setShared(await api.sharedWithMe());
        } catch {
          setShared([]);
        }
      } else {
        setShared([]);
      }
    } catch (e) {
      if (e instanceof ApiError && e.status === 401) void api.shell().then(setShell);
      else setError(say(e));
    }
  }, [active, ready, me?.user_id]);

  useEffect(() => {
    void refreshProfiles();
    // A browser can be closed from its own window, and in team mode a colleague
    // can take or release a lock. Either way the table has to notice without
    // being asked, or "is this open?" becomes a chat message — the problem the
    // column exists to remove.
    const timer = setInterval(() => void refreshProfiles(), 5_000);
    return () => clearInterval(timer);
  }, [refreshProfiles]);

  if (fatal) {
    return (
      <div className="splash">
        <div>Fury</div>
        <p className="error" style={{ maxWidth: 460, marginTop: "var(--s-4)" }}>
          {fatal}
        </p>
        <button className="ghost" onClick={() => window.location.reload()}>
          {t("app.retry")}
        </button>
      </div>
    );
  }

  if (!shell) return <div className="splash">Fury</div>;

  if (signingUp) {
    return (
      <Signup
        version={shell.version}
        onDone={() => {
          setSigningUp(false);
          void api.shell().then(setShell);
        }}
        onCancel={() => setSigningUp(false)}
        serverUrl={shell.server_url}
      />
    );
  }

  if (enrolling) {
    return (
      <Enrol
        version={shell.version}
        onDone={() => {
          setEnrolling(false);
          void api.shell().then(setShell);
        }}
        onCancel={() => setEnrolling(false)}
        serverUrl={shell.server_url}
      />
    );
  }

  if (!local && !shell.signed_in) {
    return shell.server_url ? (
      <Login
        version={shell.version}
        onSuccess={() => void api.shell().then(setShell)}
        onEnrol={() => setEnrolling(true)}
        onSignup={() => setSigningUp(true)}
        onLocal={() => void api.shell().then(setShell)}
        lastEmail={shell.last_email}
      />
    ) : (
      <ServerSetup
        version={shell.version}
        onDone={setShell}
        onSignup={() => setSigningUp(true)}
        onEnrol={() => setEnrolling(true)}
        onLocal={() => void api.shell().then(setShell)}
        lastServer={shell.last_server}
        lastEmail={shell.last_email}
      />
    );
  }

  // Signed in and holding nothing that can open a profile.
  //
  // With the key remembered — the default — this does not happen: it comes
  // back from the keychain with the session. It is reached by someone who
  // turned that off, and by a member who has not been handed the key yet, and
  // for the second the password is not the answer. Both are told which.
  if (!local && !shell.org_key_ready) {
    return (
      <Login
        version={shell.version}
        onSuccess={() => void api.shell().then(setShell)}
        onEnrol={() => setEnrolling(true)}
        onSignup={() => setSigningUp(true)}
        onLocal={() => void api.shell().then(setShell)}
        unlockFor={shell.last_email}
        awaitingKey={shell.awaiting_key}
      />
    );
  }

  const signOut = async () => {
    await api.logout();
    setMe(null);
    setProjects([]);
    setActive(null);
    setShell(await api.shell());
  };

  const onLaunch = async (profile: Profile, force = false) => {
    if (launching.has(profile.id)) return;
    setLaunching((prev) => new Set(prev).add(profile.id));
    setError(null);
    try {
      const res = await api.launch(profile.id, force, profile.origin);
      // Restrictions are the technical half of "an operator cannot take the
      // data home", and they are applied silently. Saying which ones landed is
      // the difference between a browser that behaves oddly and one whose
      // limits were explained.
      // Named in words, not in flags. This printed the field names off the
      // wire — "autofill_only, deny_cdp, lock_data_export, lock_devtools,
      // wipe_on_exit" — which is the server's vocabulary and nobody else's, and
      // in a bar that looks like the one errors use. It was read as five things
      // having gone wrong. They are one thing, decided on purpose: this account
      // was opened for somebody who may use it and may not carry it away.
      const applied = Object.entries(res.restrictions ?? {})
        .filter(([, on]) => on)
        .map(([k]) => t(restrictionKey(k)))
        .filter(Boolean);
      if (applied.length > 0) {
        setNotice({
          text: t("app.launchRestricted", { list: applied.join("; ") }),
          forProfile: profile.id,
        });
      }
      await refreshProfiles();
    } catch (e) {
      setError(say(e));
    } finally {
      setLaunching((prev) => {
        const next = new Set(prev);
        next.delete(profile.id);
        return next;
      });
    }
  };

  // Tags in use. Groups were a second, weaker version of this — one label where
  // a tag list allows several — and two ways to sort the same profiles is one
  // too many. Projects are the unit that carries access, so nothing was lost.
  const tags = Array.from(new Set(profiles.flatMap((p) => p.tags))).sort();
  // Stages in use, lower-cased so "Banned" and "banned" are one filter.
  const stages = Array.from(new Set(profiles.map((p) => (p.status ?? "").trim().toLowerCase()).filter(Boolean))).sort();

  const shown = matching(profiles, query)
    .filter(
      (p) =>
        tag === "" ||
        (tag === "\u0000none" ? p.tags.length === 0 : p.tags.includes(tag)),
    )
    .filter((p) => stage === "" || (stage === "\u0000none" ? !p.status : (p.status ?? "").trim().toLowerCase() === stage))
    .filter((p) => !openOnly || p.running);

  // Exit sharing over EVERY profile, not the filtered view: the question is
  // how many accounts a site sees from one address.
  const exitCounts = exitSharing(profiles, proxyList);

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
        await api.launch(p.id, false, p.origin);
        await refreshProfiles();
      }
      setError(null);
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const onStop = async (profile: Profile) => {
    setBusy(true);
    try {
      await api.stop(profile.id, profile.origin);
      await refreshProfiles();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  const commands: Command[] = [
    { id: "act:new", label: t("cmd.newProfile"), run: () => setEditing(null) },
    { id: "act:settings", label: t("cmd.settings"), run: () => { setSettingsTab("general"); setSettingsOpen(true); } },
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
            setNotice({
              text: t("ex.done", { kb: Math.round(r.bytes / 1024), path: r.path }),
              forProfile: null,
            });
          } catch (e) {
            setError(say(e));
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
            setNotice({ text: t("ex.imported", { n: r.profiles }), forProfile: null });
            await load();
          } catch (e) {
            setError(say(e));
          }
  };

  // Read once per render rather than reached for in three places in the JSX.
  const dl = shell?.core_download ?? null;

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
        onSettings={() => { setSettingsTab("general"); setSettingsOpen(true); }}
        onRenameProject={async (p) => {
          const name = await ask({
            title: t("proj.rename"),
            placeholder: t("proj.newName"),
            initial: p.name,
            confirmLabel: t("ui.save"),
          });
          if (!name) return;
          await api.renameProject(p.id, name, p.origin);
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
          try {
            await api.deleteProject(p.id, p.origin);
          } catch (e) {
            // Reported rather than swallowed. Without this the delete failed
            // in silence and the project simply stayed in the sidebar, which
            // is indistinguishable from a click that did not register.
            setError(say(e));
            return;
          }
          setActive(null);
          await load();
          // The rows change too, not just the sidebar: every profile that was
          // in this project is now in none, and until this ran the table went
          // on naming a project that no longer exists.
          //
          // `null` explicitly: see refreshProfiles.
          await refreshProfiles(null);
        }}
        /* Which side the new folder goes on is now the caller's to say, and
           the sidebar offers both when there is a server. Connected to one, the
           + used to make a project ON IT and there was no way to make a local
           folder at all -- so a local profile had nowhere to be filed, and
           "why is a local folder not being created" was the right question with
           the answer "it never was". */
        /* A whole local folder onto the server, in one action: make a project
           of the same name, then send everything in it. Per profile was the
           only way and does not survive somebody with dozens of folders. */
        onSendProject={async (p) => {
          const rows = await api.profiles(p.id, "local");
          if (rows.length === 0) return;
          const go = await ask({
            title: t("proj.sendAll"),
            detail: t("proj.sendAllConfirm", { n: String(rows.length), name: p.name }),
            confirmLabel: t("up.send"),
          });
          if (go === null) return;
          setBusy(true);
          try {
            const made = await api.createProjectIn(p.name, "team");
            const target = (made as { id: string }).id;
            // Counted, and the first failure named rather than swallowed.
            //
            // This was a bare loop that stopped on the first error and reported
            // it alone: the folder had been created by then, so the sidebar
            // showed a team project with nothing in it and a message about a
            // request, and the two did not obviously belong to each other. What
            // an operator needs to know is how many of the n arrived — and if
            // none did, that the empty folder on the server is the reason.
            let sent = 0;
            let failure: string | null = null;
            for (const row of rows) {
              try {
                await api.uploadProfile(row.id, target);
                sent++;
              } catch (e) {
                failure ??= `${row.name}: ${say(e)}`;
              }
            }
            setError(
              failure === null
                ? t("proj.sendAllDone", { n: String(sent), name: p.name })
                : t("proj.sendAllPartly", {
                    sent: String(sent),
                    n: String(rows.length),
                    name: p.name,
                    why: failure,
                  }),
            );
          } catch (e) {
            setError(say(e));
          } finally {
            setBusy(false);
            await load();
            await refreshProfiles();
          }
        }}
        /* And the other end: everything in a server project, to one person.
           The dialog says what it cannot do -- a profile added tomorrow is not
           covered, because each carries its own key. */
        onShareProject={async (p) => {
          const rows = await api.profiles(p.id, "team");
          if (rows.length === 0) return;
          setError(t("proj.shareAllNote", { n: String(rows.length) }));
          setSharing(rows);
        }}
        onNewProject={async (origin) => {
          const name = await ask({
            title: origin === "local" ? t("app.newProjectHere") : t("app.newProject"),
            placeholder: t("app.projectName"),
            confirmLabel: t("ui.create"),
          });
          if (!name) return;
          await api.createProjectIn(name, origin);
          await load();
        }}
      />
      <main className="main">
        <header className="head">
          <h1>
            {view === "trash"
              ? t("trash.title")
              : view === "proxies"
                ? t("nav.proxies")
                : view === "extensions"
                ? t("nav.extensions")
                : view === "assistants"
                ? t("nav.assistants")
                : view === "users"
                  ? t("nav.users")
                  : view === "sharedWithMe"
                    ? t("nav.sharedWithMe")
                    : (active?.name ?? t("nav.profiles"))}
          </h1>
          {view === "profiles" && (
            <span className="muted">{t("app.profileCount", { n: profiles.length })}</span>
          )}
          {view === "sharedWithMe" && (
            <span className="muted">{t("app.profileCount", { n: shared.length })}</span>
          )}
        </header>

        {!shell.agent_ready && (
          <div className="notice warnBar" role="status">
            {t("app.agentDown")}
          </div>
        )}

        {warm.length > 0 && (
          <div className="notice" role="status" style={{ display: "block" }}>
            {warm.map((w) => (
              <div key={w.profile_id} className="row" style={{ flexWrap: "wrap", marginBottom: "var(--s-1)" }}>
                <strong>{w.name}</strong>
                <span className="muted small">
                  {w.finished
                    ? w.error
                      ? t("warm.failed", { err: w.error })
                      : w.stopped
                        ? t("warm.stoppedAt", { done: w.done, total: w.total })
                        : t("warm.doneAll", { total: w.total })
                    : t("warm.progress", { done: w.done, total: w.total })}
                  {" · "}
                  {t("warm.cookies", { before: w.cookies_before, now: w.cookies_now })}
                </span>
                {w.current && <span className="mono small muted" style={{ maxWidth: 360, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{w.current}</span>}
                <div className="spacer" />
                {!w.finished && (
                  <button className="ghost" onClick={() => void api.warmStop(w.profile_id)}>{t("mir.stop")}</button>
                )}
              </div>
            ))}
            {warm.every((w) => w.finished) && (
              <button className="linky" onClick={async () => { await api.warmClear(); setWarm([]); }}>{t("warm.dismiss")}</button>
            )}
          </div>
        )}

        {farms && (
          <div className="notice" role="status" style={{ display: "block" }}>
            {farms.farms.map((f) => {
              const tracks = f.targets.map((x) => f.tracks[x.id]);
              const daysDone = Math.min(...tracks.map((tr) => tr?.days_done ?? 0));
              const finished = tracks.every((tr) => (tr?.days_done ?? 0) >= f.spec.days);
              const sites = tracks.reduce((n, tr) => n + (tr?.sites ?? 0), 0);
              const running = tracks.some((tr) => tr?.sessions.some((s) => s.started_ms && !s.ended));
              const next = Math.min(
                ...tracks.flatMap((tr) => (tr?.sessions ?? []).filter((s) => !s.started_ms && !s.ended).map((s) => s.at_ms)),
              );
              const errors = f.targets
                .map((x) => ({ name: x.name, err: f.tracks[x.id]?.last_error }))
                .filter((x) => x.err);
              const when = (ms: number) => {
                const d = new Date(ms);
                const hm = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
                return d.toDateString() === new Date().toDateString() ? hm : `${d.toLocaleDateString([], { day: "numeric", month: "short" })} ${hm}`;
              };
              return (
                <div key={f.id} style={{ marginBottom: "var(--s-1)" }}>
                  <div className="row" style={{ alignItems: "baseline" }}>
                    <strong style={{ flexShrink: 0 }}>{f.spec.name || t("farm.untitled")}</strong>
                    <span className="muted small" style={{ flex: 1, minWidth: 0 }}>
                      {t("farm.profiles", { n: f.targets.length })}
                      {" · "}
                      {finished ? t("farm.finished") : t("farm.dayOf", { done: daysDone, days: f.spec.days })}
                      {" · "}
                      {t("farm.sitesRead", { n: sites })}
                      {!finished && (f.paused
                        ? ` · ${t("farm.paused")}`
                        : running
                          ? ` · ${t("farm.running")}`
                          : Number.isFinite(next)
                            ? ` · ${t("farm.next", { at: when(next) })}`
                            : "")}
                    </span>
                    {!finished && (
                      <button className="ghost" onClick={async () => { await api.farmPause(f.id, !f.paused); await refreshFarms(); }}>
                        {f.paused ? t("farm.resume") : t("farm.pause")}
                      </button>
                    )}
                    <button
                      className="ghost"
                      onClick={async () => {
                        if (!finished && (await ask({ title: t("farm.deleteAsk"), detail: t("farm.deleteBody"), confirmLabel: t("farm.delete"), danger: true })) === null) return;
                        await api.farmDelete(f.id);
                        await refreshFarms();
                      }}
                    >
                      {finished ? t("warm.dismiss") : t("farm.delete")}
                    </button>
                  </div>
                  {errors.length > 0 && (
                    <div className="hint" title={errors.map((x) => `${x.name}: ${x.err}`).join("\n")}>
                      {t("farm.lastError", { name: errors[0].name, err: errors[0].err ?? "" })}
                      {errors.length > 1 ? ` (+${errors.length - 1})` : ""}
                    </div>
                  )}
                </div>
              );
            })}
            {farms.farms.some((f) => f.targets.some((x) => x.origin !== "local")) && (
              <div className="hint">{t("farm.teamNeedsWindow")}</div>
            )}
          </div>
        )}

        {(mirror || mirrorNote) && (
          <div className="notice" role="status" style={{ display: "block" }}>
            {mirror && (
              <div className="row" style={{ flexWrap: "wrap" }}>
                <strong>{t("mir.active", { n: mirror.members.length })}</strong>
                <span className="muted small">{mirror.members.map((m) => m.name).join(" · ")}</span>
                <span className="muted small">· {t("mir.mirrored", { n: mirror.mirrored })}</span>
                <div className="spacer" />
                <label className="row" style={{ gap: 6 }}>
                  <input
                    type="checkbox"
                    style={{ width: 14, height: 14, accentColor: "var(--accent)" }}
                    checked={mirror.typing}
                    onChange={async (e) => {
                      await api.mirrorTyping(e.target.checked);
                      setMirror({ ...mirror, typing: e.target.checked });
                    }}
                  />
                  <span>{t("mir.typing")}</span>
                </label>
                <button
                  className="ghost"
                  onClick={async () => {
                    await api.mirrorStop();
                    setMirror(null);
                    setMirrorNote(null);
                  }}
                >
                  {t("mir.stop")}
                </button>
              </div>
            )}
            {mirror && <div className="hint" style={{ marginTop: "var(--s-1)" }}>{t("mir.cost")}</div>}
            {mirrorNote && <div className="hint" style={{ marginTop: "var(--s-1)" }}>{mirrorNote}</div>}
          </div>
        )}

        {/* Shown before anything is attempted, not after Launch fails. The
            application is one download and the browser is another, and somebody
            who took only the first has an app that looks entirely finished
            until the moment they press the button. Saying so on arrival turns a
            confusing failure into a five-minute errand. */}
        {/* Two different situations, and they used to share one bar.
            A browser that is not installed blocks everything and needs a
            download. A FURY_CORE naming a deleted build blocks nothing any more
            — it is ignored — but it is somebody's leftover and will confuse
            them again next week, so it is worth saying without shouting. */}
        {/* A button rather than a command to paste. The browser is a second,
            134 MB download — that split is deliberate and stays — but asking
            somebody to open a terminal to finish installing a desktop
            application is asking most people to give up.

            Only ever on press. Nothing here polls for versions or downloads on
            a schedule: an anti-detect browser that phones a known host at a
            predictable time is a beacon, and this is the same request the user
            would have made by visiting the releases page. See
            agent/src/core_download.rs. */}
        {shell.agent_ready && !shell.core_ready && (
          <div className="notice warnBar" role="status">
            <div style={{ flex: 1 }}>
              <strong>
                {shell.core_outdated
                  ? t("app.coreOutdated", {
                      have: shell.core_outdated.have.split(".")[0],
                      need: String(shell.core_outdated.need),
                    })
                  : t("app.noCore")}
              </strong>
              <div className="muted" style={{ marginTop: "var(--s-1)" }}>
                {dl?.running
                  ? dl.total > 0
                    ? t("app.coreDownloading", {
                        done: Math.round(dl.downloaded / 1e6).toString(),
                        total: Math.round(dl.total / 1e6).toString(),
                      })
                    : t("app.coreStarting")
                  : (dl?.error ??
                    (shell.core_outdated ? t("app.coreOutdatedHow") : null) ??
                    shell.core_problem ??
                    t("app.noCoreHow"))}
              </div>
              {dl?.running && dl.total > 0 && (
                <progress
                  value={dl.downloaded}
                  max={dl.total}
                  style={{ width: "100%", marginTop: "var(--s-1)" }}
                />
              )}
            </div>
            <button
              disabled={dl?.running || coreBusy}
              onClick={async () => {
                setCoreBusy(true);
                try {
                  await api.downloadCore();
                } catch (e) {
                  // Said, not swallowed: a refusal here used to vanish and the
                  // button looked dead (09.10.2026).
                  setError(say(e));
                } finally {
                  // Cleared by the next poll seeing running: true. Held only
                  // for the gap between the click and that poll, so the button
                  // cannot be pressed twice into two downloads.
                  setTimeout(() => setCoreBusy(false), 3000);
                }
              }}
            >
              {dl?.running ? t("app.coreDownloadingShort") : t("app.coreDownload")}
            </button>
          </div>
        )}

        {/* A newer Fury. The check used to sit behind a button on the About
            screen, so nobody saw a release unless they went looking: the
            owner ran 0.2.10 for a week of releases and asked why the
            application never said (08.10.2026). Checked at start and every
            six hours; dismissed per version. From 0.3.0 the button installs a
            signed build and restarts; a release without one falls back to the
            download link. See src-tauri/update.rs. */}
        {update && update.status === "available" && update.latest !== dismissedUpdate && (
          <div className="notice" role="status">
            <div style={{ flex: 1 }}>
              <strong>{t("app.updateAvailable", { version: (update.latest ?? "").replace(/^v/, ""), current: update.current })}</strong>
              <div className="muted" style={{ marginTop: "var(--s-1)" }}>
                {installing
                  ? installing.stage === "downloading" && installing.total
                    ? t("app.updateStage.downloading", {
                        percent: Math.min(100, Math.round((installing.received / installing.total) * 100)),
                      })
                    : t(`app.updateStage.${installing.stage === "downloading" ? "downloadingNoSize" : installing.stage}`)
                  : installError ?? t("app.updateHow")}
              </div>
            </div>
            {update.url && (
              <button className="ghost" onClick={() => void api.openUrl(update.url!)}>
                {t("app.updateNotes")}
              </button>
            )}
            {installError && (update.download ?? update.url) && (
              <button className="ghost" onClick={() => void api.openUrl(update.download ?? update.url!)}>
                {t("app.updateDownload")}
              </button>
            )}
            <button className="primary" disabled={!!installing} onClick={() => void installUpdate()}>
              {t("app.updateInstall")}
            </button>
            <button
              className="ghost"
              disabled={!!installing}
              onClick={() => {
                try {
                  localStorage.setItem("fury.update.dismissed", update.latest ?? "");
                } catch {
                  /* a private window: dismissed for this session only */
                }
                setDismissedUpdate(update.latest);
              }}
            >
              {t("app.updateLater")}
            </button>
          </div>
        )}

        {/* The team server behind the newest release. Shown to owners and
            admins of a server we do not run, once per release; the steps are
            docs/14-team-server.md, "Обновление". */}
        {(() => {
          const latest = (update?.latest ?? update?.current ?? "").replace(/^v/, "");
          const host = (() => {
            try {
              return shell?.server_url ? new URL(shell.server_url).hostname : "";
            } catch {
              return "";
            }
          })();
          if (!me || !latest || !host || OUR_SERVERS.includes(host)) return null;
          if (me.role !== "owner" && me.role !== "admin") return null;
          if (me.server_version && !versionNewer(latest, me.server_version)) return null;
          // Without a version the server is older than 0.4.2; only worth
          // saying once a release that reports one exists.
          if (!me.server_version && !versionNewer(latest, "0.4.1")) return null;
          if (dismissedServer === latest) return null;
          return (
            <div className="notice" role="status">
              <div style={{ flex: 1 }}>
                <strong>
                  {me.server_version
                    ? t("app.serverUpdate", { host, version: me.server_version, latest })
                    : t("app.serverUpdateUnknown", { host, latest })}
                </strong>
                <div className="muted" style={{ marginTop: "var(--s-1)" }}>{t("app.serverUpdateHow", { latest })}</div>
              </div>
              <button
                className="ghost"
                onClick={() =>
                  void api.openUrl("https://github.com/furyteamtop/fury-antidetect-browser/blob/main/docs/14-team-server.md")
                }
              >
                {t("app.serverUpdateSteps")}
              </button>
              <button
                className="ghost"
                onClick={() => {
                  try {
                    localStorage.setItem("fury.serverUpdate.dismissed", latest);
                  } catch {
                    /* a private window: dismissed for this session only */
                  }
                  setDismissedServer(latest);
                }}
              >
                {t("app.updateLater")}
              </button>
            </div>
          );
        })()}

        {/* The right Chrome, an older build of it -- agent/src/main.rs,
            core_stale. Profiles keep launching; this only offers the build
            the application expects, and says what the old one lacks. A
            tester ran the 0.2.3 core for days with Chromium's icon and
            without the canvas fixes, because nothing said a newer one
            existed (02.10.2026). Same download as the bar above, on press. */}
        {shell.agent_ready && shell.core_ready && shell.core_stale && (
          <div className="notice" role="status">
            <div style={{ flex: 1 }}>
              <strong>
                {shell.core_stale.have
                  ? t("app.coreStale", { have: shell.core_stale.have, need: shell.core_stale.need })
                  : t("app.coreStaleUnknown", { need: shell.core_stale.need })}
              </strong>
              <div className="muted" style={{ marginTop: "var(--s-1)" }}>
                {dl?.running
                  ? dl.total > 0
                    ? t("app.coreDownloading", {
                        done: Math.round(dl.downloaded / 1e6).toString(),
                        total: Math.round(dl.total / 1e6).toString(),
                      })
                    : t("app.coreStarting")
                  : (dl?.error ?? t("app.coreStaleHow", { what: t(coreWhat(shell.core_stale.need)) }))}
              </div>
              {dl?.running && dl.total > 0 && (
                <progress
                  value={dl.downloaded}
                  max={dl.total}
                  style={{ width: "100%", marginTop: "var(--s-1)" }}
                />
              )}
            </div>
            <button
              disabled={dl?.running || coreBusy}
              onClick={async () => {
                setCoreBusy(true);
                try {
                  await api.downloadCore();
                } catch (e) {
                  setError(say(e));
                } finally {
                  setTimeout(() => setCoreBusy(false), 3000);
                }
              }}
            >
              {dl?.running ? t("app.coreDownloadingShort") : t("app.coreUpdate")}
            </button>
          </div>
        )}

        {shell.agent_ready && shell.core_ready && shell.core_problem && (
          <div className="notice" role="status">
            <div>
              <strong>{t("app.staleCoreVar")}</strong>
              <div className="muted" style={{ marginTop: "var(--s-1)" }}>
                {shell.core_problem}
              </div>
            </div>
          </div>
        )}

        {notice && (
          <div className="notice infoBar" role="status">
            {notice.text}
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

        {view === "profiles" && (
          <div className="toolbar">
            {/* No "New profile" here. It is the first thing in the sidebar,
                permanently, and a second copy of the primary action a few
                pixels away is two places to look for one decision.

                Making MANY is a different decision and does belong here: it is
                the one people reach for after the first few profiles, and it
                acts on the project this toolbar is already filtering. */}
            {/* The three ways to get many profiles at once, in one menu: three
                buttons of their own pushed the filters and Refresh onto a
                second line at an ordinary window width (14.10.2026).

                CSV and the other antidetect import into this computer only: a
                team profile's proxy carries sealed credentials on the server --
                see CsvImport. Offered whenever the profiles would land here:
                local mode, a local project, or no project on a connected shell.
                It used to follow the shell's mode alone, so a connected shell
                never showed it, even inside a local project (reported
                10.10.2026: "where is the import"). */}
            <Select
              style={{ width: "auto" }}
              value=""
              onChange={(e) => {
                const v = e.target.value;
                if (v === "many") setBulk(null);
                else if (v === "csv") setCsv(true);
                else if (v === "antidetect") setGologin(true);
              }}
            >
              <option value="">{t("bar.addMany")}</option>
              <option value="many">{t("bp.button")}</option>
              {importsHere && <option value="csv">{t("csv.button")}</option>}
              {importsHere && <option value="antidetect">{t("gl.button")}</option>}
            </Select>
            <input
              className="search"
              placeholder={t("bar.search")}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            {tags.length > 0 && (
              <Select style={{ width: "auto" }} value={tag} onChange={(e) => setTag(e.target.value)}>
                <option value="">{t("bar.allTags")}</option>
                <option value={"\u0000none"}>{t("bar.untagged")}</option>
                {tags.map((x) => (
                  <option key={x} value={x}>
                    {x}
                  </option>
                ))}
              </Select>
            )}
            {stages.length > 0 && (
              <Select style={{ width: "auto" }} value={stage} onChange={(e) => setStage(e.target.value)}>
                <option value="">{t("bar.allStages")}</option>
                <option value={"\u0000none"}>{t("bar.noStage")}</option>
                {stages.map((x) => (
                  <option key={x} value={x}>
                    {isSuggested(x) ? t(`status.${x}` as never) : x} ({profiles.filter((p) => (p.status ?? "").trim().toLowerCase() === x).length})
                  </option>
                ))}
              </Select>
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
            <IconButton
              icon="refresh"
              label={t("bar.refresh")}
              onClick={() => void refreshProfiles()}
            />
            {/* Its own line, always: it appears and disappears with the
                selection, and a block that reflows the filters above it every
                time a checkbox is ticked is the "visual mess" this replaced. */}
            {chosen.length > 0 && (
              <div className="bulk">
                {/* One kind of control, each saying what it does in words, in
                    the order a person reaches for them: what is selected, open
                    or close it, where it lives, who gets it, its proxy, the
                    rest, and Delete apart at the end. Until 14.10.2026 this was
                    text buttons, bare icons and two dropdowns side by side, and
                    the way to hand a profile from this machine to the team was
                    an unlabelled arrow that filed it into a project of its own
                    choosing ("I don't understand how to give my team access"). */}
                <span className="muted small">{t("bar.selected", { n: chosen.length })}</span>
                <button className="ghost" onClick={() => setSelected(new Set())}>
                  {t("bar.clearSelection")}
                </button>
                <span className="bulkGap" />
                {openable.length > 0 && (
                  <button disabled={busy} onClick={openMany}>
                    {t("bar.openSelected", { n: openable.length })}
                  </button>
                )}
                {closable.length > 0 && (
                  <button
                    disabled={busy}
                    onClick={async () => {
                      setBusy(true);
                      for (const p of closable) await api.stop(p.id, p.origin);
                      await refreshProfiles();
                      setBusy(false);
                    }}
                  >
                    {t("bar.closeSelected", { n: closable.length })}
                  </button>
                )}
                {/* Where the selection lives, both sides in one list. A folder
                    on the same side is a move, as before. A team project for
                    profiles from this machine is how the team gets them: they
                    go to the server, into that project, and everyone the
                    project is open to sees them -- the copy here goes to the
                    trash, so the profile is not listed twice. The other way
                    (server to this machine) does not exist, so a team selection
                    is offered team projects only. */}
                {(() => {
                  const side = chosen.every((p) => p.origin === "local")
                    ? "local"
                    : chosen.every((p) => p.origin === "team")
                      ? "team"
                      : null;
                  const here = projects.filter((p) => p.origin === "local");
                  const team = projects.filter((p) => p.origin === "team");
                  if (here.length + team.length === 0) return null;
                  const sendToTeam = async (target: string) => {
                    const project = projects.find((p) => p.id === target)?.name ?? "";
                    const go = await ask({
                      title: t("bar.moveTo"),
                      detail: t("up.moveConfirm", { n: String(chosen.length), project }),
                      confirmLabel: t("up.moveGo"),
                    });
                    if (go === null) return;
                    setBusy(true);
                    setError(null);
                    let moved = 0;
                    let proxyMoved = false;
                    try {
                      for (const p of chosen) {
                        const r = await api.uploadProfile(p.id, target);
                        proxyMoved = proxyMoved || r.proxy_moved;
                        // Only once it is on the server: a failed send leaves
                        // the profile where it was.
                        await api.deleteProfile(p.id, "local");
                        moved += 1;
                      }
                    } catch (e) {
                      setError(say(e));
                    } finally {
                      setSelected(new Set());
                      await load();
                      await refreshProfiles();
                      setBusy(false);
                    }
                    if (moved > 0) {
                      setNotice({
                        text: t("up.moved", { n: moved, project }) + (proxyMoved ? " " + t("up.proxyMoved") : ""),
                        forProfile: null,
                      });
                    }
                  };
                  return (
                    <Select
                      style={{ width: "auto" }}
                      value=""
                      disabled={busy || side === null}
                      title={side === null ? t("bar.moveMixed") : undefined}
                      onChange={async (e) => {
                        const to = e.target.value;
                        if (to === "") return;
                        if (side === "local" && team.some((p) => p.id === to)) {
                          await sendToTeam(to);
                          return;
                        }
                        setBusy(true);
                        try {
                          // "\u0000none" rather than "" for out-of-every-folder:
                          // an empty value is the placeholder, and the two must
                          // not be the same string or picking one does nothing.
                          await api.moveProfiles(
                            chosen.map((p) => p.id),
                            to === "\u0000none" ? null : to,
                            chosen[0].origin,
                          );
                          setSelected(new Set());
                          await load();
                          await refreshProfiles();
                        } catch (err) {
                          setError(say(err));
                        } finally {
                          setBusy(false);
                        }
                      }}
                    >
                      <option value="">{t("bar.moveTo")}</option>
                      {side === "local" && (
                        <optgroup label={t("app.projectsHere")}>
                          {here.map((p) => (
                            <option key={p.id} value={p.id}>
                              {p.name}
                            </option>
                          ))}
                          {/* Out of every folder is a local idea: on a server a
                              profile always belongs to a project, because the
                              project is what carries access to it. */}
                          <option value={"\u0000none"}>{t("bar.moveOut")}</option>
                        </optgroup>
                      )}
                      {team.length > 0 && (
                        <optgroup label={t("app.projectsTeam")}>
                          {team.map((p) => (
                            <option key={p.id} value={p.id}>
                              {p.name}
                            </option>
                          ))}
                        </optgroup>
                      )}
                    </Select>
                  );
                })()}
                {/* To one person, by address -- not the same thing as the
                    team. The hover says which is which. A profile from this
                    machine is copied to the server on the way, by the dialog. */}
                {!local && (
                  <button disabled={busy} title={t("bar.shareWhy")} onClick={() => setSharing(chosen)}>
                    {t("row.share")}
                  </button>
                )}
                {/* Proxy for the whole selection (5.7). Offered when every
                    chosen row is on the side the proxy list describes: the
                    list is this machine's in local mode and the server's
                    otherwise, and a local profile cannot point at a server
                    proxy. "Each its own" hands out proxies nobody uses yet,
                    one per profile — the answer to the household warning in
                    the proxy column. */}
                {(() => {
                  const side = local ? "local" : "team";
                  if (!proxyList || proxyList.length === 0 || !chosen.every((p) => p.origin === side)) return null;
                  const used = new Set(profiles.map((p) => p.proxy?.id).filter(Boolean));
                  const free = proxyList.filter((x) => !used.has(x.id));
                  const assign = async (pairs: [Profile, string | null][]) => {
                    setBusy(true);
                    setError(null);
                    let failed = 0;
                    for (const [p, id] of pairs) {
                      try {
                        // The agent takes the id and refuses a partial proxy
                        // object; the server path reads `proxy.id`. Each side
                        // gets the shape it reads.
                        await api.saveProfile(
                          local ? { ...p, proxy_id: id ?? "", proxy: null } : { ...p, proxy: id ? { id } : null },
                          p.origin,
                        );
                      } catch {
                        failed++;
                      }
                    }
                    setSelected(new Set());
                    await refreshProfiles();
                    setBusy(false);
                    setNotice({ text: failed > 0 ? t("bar.proxySetFailed", { n: pairs.length - failed, f: failed }) : t("bar.proxySet", { n: pairs.length }), forProfile: null });
                  };
                  return (
                    <Select
                      style={{ width: "auto" }}
                      value=""
                      disabled={busy}
                      onChange={(e) => {
                        const v = e.target.value;
                        if (v === "") return;
                        if (v === "\u0000each") {
                          void assign(chosen.map((p, i) => [p, free[i].id] as [Profile, string]));
                        } else if (v === "\u0000none") {
                          void assign(chosen.map((p) => [p, null] as [Profile, null]));
                        } else {
                          void assign(chosen.map((p) => [p, v] as [Profile, string]));
                        }
                      }}
                    >
                      <option value="">{t("bar.proxyTo")}</option>
                      <option value={"\u0000each"} disabled={free.length < chosen.length}>
                        {t("bar.proxyEach", { n: free.length })}
                      </option>
                      {proxyList.map((x) => (
                        <option key={x.id} value={x.id}>
                          {x.name} · {x.last_country ?? x.host}{used.has(x.id) ? ` · ${t("px.usedByN", { n: profiles.filter((p) => p.proxy?.id === x.id).length })}` : ""}
                        </option>
                      ))}
                      {/* A team profile must have a proxy: the server refuses one without. */}
                      {local && <option value={"\u0000none"}>{t("bar.proxyNone")}</option>}
                    </Select>
                  );
                })()}
                {/* A team profile is warmed the way it is opened: under its
                    lock, so only by someone who may open it. */}
                {chosen.length >= 1 && chosen.every((p) => p.permissions.includes("launch")) && (
                  <button disabled={busy} title={t("warm.why")} onClick={() => setWarmFor(chosen)}>
                    {t("warm.button", { n: chosen.length })}
                  </button>
                )}
                {chosen.length >= 2 && local && (
                  <button
                    disabled={busy}
                    title={t("mir.hint")}
                    onClick={async () => {
                      setMirrorNote(null);
                      try {
                        const r = await api.mirrorStart(chosen.map((p) => p.id), mirror?.typing ?? true);
                        setMirror(r.status.active ? r.status : null);
                        if (r.refused.length > 0) {
                          setMirrorNote(
                            r.refused
                              .map((x) => {
                                const name = profiles.find((p) => p.id === x.id)?.name ?? x.id;
                                return x.reason === "open_without_cdp" ? t("mir.refusedOpen", { name }) : `${name}: ${say({ code: x.code, message: x.reason })}`;
                              })
                              .join(" "),
                          );
                        }
                        await refreshProfiles();
                      } catch (e) {
                        setMirrorNote(say(e));
                      }
                    }}
                  >
                    {t("mir.start", { n: chosen.length })}
                  </button>
                )}
                {/* One profile, not many. Copying acts on a single source and
                    cookies belong to a single jar, so offering either for a
                    multi-selection would mean guessing which one was meant. */}
                {chosen.length === 1 && (
                  <>
                    <button disabled={busy} onClick={() => setBulk(chosen[0])}>
                      {t("bar.cloneOne")}
                    </button>
                    <button disabled={busy} onClick={() => setCookiesFor(chosen[0])}>
                      {t("bar.cookiesOne")}
                    </button>
                  </>
                )}
                <button
                  className="danger bulkDelete"
                  title={closable.length > 0 ? t("row.closeFirst") : undefined}
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
                    for (const p of chosen) await api.deleteProfile(p.id, p.origin);
                    setSelected(new Set());
                    await load();
                    await refreshProfiles();
                    setBusy(false);
                  }}
                >
                  {t("bar.deleteSelected", { n: chosen.length })}
                </button>
              </div>
            )}

          </div>
        )}

        {/* No project selected is not the same as a project with no profiles,
            and saying the latter to someone who has been granted nothing sends
            them looking for a profile list that was never theirs. */}
        {view === "proxies" && <Proxies profiles={profiles} local={local} myEmail={me?.email ?? null} />}
        {view === "extensions" && <ExtensionsView profiles={profiles} />}
        {view === "assistants" && <AssistantsView />}

        {view === "users" && (
          <Users
            projects={projects}
            local={local}
            serverUrl={shell.server_url}
            onSignOut={signOut}
            onConnect={() => { setSettingsTab("team"); setSettingsOpen(true); }}
          />
        )}

        {view === "trash" && (
          <Trash
            onChanged={async () => {
              await load();
              await refreshProfiles();
            }}
          />
        )}

        {/* Borrowed profiles, in a room of their own.
            Not merged into the main list, and deliberately: one of these can be
            taken back while you are looking at it, its name was chosen by
            somebody else, and two people can both have a "Shop DE". The column
            that shows a project shows who lent it instead. */}
        {view === "sharedWithMe" && (
          <div className="tableWrap">
            {shared.length === 0 ? (
              <p className="muted" style={{ padding: "var(--s-3)" }}>{t("shared.empty")}</p>
            ) : (
              <ProfileTable
                profiles={shared}
                showProject
                selected={new Set()}
                onToggle={() => {}}
                onToggleAll={() => {}}
                me={me}
                thisMachine={shell.machine_name}
                local={false}
                busy={busy}
                launching={launching}
                onLaunch={onLaunch}
                onStop={onStop}
              />
            )}
          </div>
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
              launching={launching}
              onLaunch={onLaunch}
              onStop={onStop}
              onEdit={setEditing}
              onExtensions={setExtFor}
              onShare={local ? undefined : (p) => setSharing([p])}
              onNetwork={setNetFor}
              onClone={(p) => setBulk(p)}
              personas={personaIds}
              sharing={exitCounts}
              /* Offered always, and gated per row rather than per mode.
                 It used to be handed over only in local mode, so connecting to
                 a server removed the delete button from EVERY row -- including
                 the local ones now shown beside the server's, which the server
                 has no say over at all. The table shows it when the row's own
                 permissions allow it, and a local row carries all of them. */
              onDelete={
                (async (p) => {
                      const go = await ask({
                        title: t("row.delete"),
                        detail: t("row.confirmDelete", { name: p.name }),
                        confirmLabel: t("ui.delete"),
                        danger: true,
                      });
                      if (go === null) return;
                      await api.deleteProfile(p.id, p.origin);
                      await refreshProfiles();
                })
              }
            />
          </div>
        )}

        {sharing && (
          <ShareDialog
            profiles={sharing}
            onClose={() => {
              setSharing(null);
              void refreshProfiles();
            }}
          />
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
            initialTab={settingsTab}
            hasProject={active !== null}
            onExport={exportProject}
            onImport={importProject}
            onChanged={setShell}
            onEnrol={() => {
              setSettingsOpen(false);
              setEnrolling(true);
            }}
            onSignup={() => {
              setSettingsOpen(false);
              setSigningUp(true);
            }}
            onClose={() => setSettingsOpen(false)}
          />
        )}

        {editing !== undefined && (
          <ProfileDialog
            projectId={active?.id ?? null}
            teamProjects={projects.filter((p) => p.origin === "team")}
            editing={editing}
            origin={newOrigin}
            canChoose={canChooseOrigin}
            onClose={() => setEditing(undefined)}
            onSaved={async () => {
              setEditing(undefined);
              await load();
              await refreshProfiles();
            }}
          />
        )}

        {bulk !== undefined && (
          <BulkProfiles
            cloneOf={bulk}
            projectId={active?.id ?? null}
            origin={newOrigin}
            canChoose={canChooseOrigin}
            onClose={() => setBulk(undefined)}
            onDone={async () => {
              setSelected(new Set());
              await load();
              await refreshProfiles();
            }}
          />
        )}
        {csv && (
          <CsvImport
            projectId={active?.origin === "local" ? active.id : null}
            onDone={() => void refreshProfiles()}
            onClose={() => setCsv(false)}
          />
        )}

        {gologin && (
          <AntidetectImport
            projectId={active?.origin === "local" ? active.id : null}
            onDone={() => void refreshProfiles()}
            onClose={() => setGologin(false)}
          />
        )}

        {cookiesFor && (
          <Cookies profile={cookiesFor} onClose={() => setCookiesFor(null)} />
        )}
        {warmFor && (
          <WarmDialog
            profiles={warmFor}
            onStarted={(note) => {
              setMirrorNote(note);
              void refreshProfiles();
            }}
            onClose={() => setWarmFor(null)}
          />
        )}
        {netFor && netFor.proxy && (
          <NetworkReport proxy={netFor.proxy} onClose={() => setNetFor(null)} />
        )}
        {extFor && (
          <Extensions
            profile={extFor}
            running={isOpenHere(extFor, { local, userId: me?.user_id, machine: shell.machine_name })}
            onClose={() => setExtFor(null)}
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

/** A restriction as a sentence, from the flag the server sends.
 *
 *  Spelled out rather than composed as `restrict.${k}`: the flags arrive as
 *  free text over the wire, and a key built from one would neither typecheck
 *  nor fail visibly — it would render the key, which is the thing this exists
 *  to stop. An unknown flag falls back to `restrict.other`, so a server that
 *  learns a new one says something true rather than something raw. */
function restrictionKey(
  flag: string,
): "restrict.autofillOnly" | "restrict.denyCdp" | "restrict.lockDataExport" | "restrict.lockDevtools" | "restrict.wipeOnExit" | "restrict.other" {
  switch (flag) {
    case "autofill_only":
      return "restrict.autofillOnly";
    case "deny_cdp":
      return "restrict.denyCdp";
    case "lock_data_export":
      return "restrict.lockDataExport";
    case "lock_devtools":
      return "restrict.lockDevtools";
    case "wipe_on_exit":
      return "restrict.wipeOnExit";
    default:
      return "restrict.other";
  }
}
