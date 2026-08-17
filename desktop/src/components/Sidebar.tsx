// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useI18n } from "../i18n";
import { useState } from "react";
import type { Me, Project, Shell } from "../api";

export type View = "profiles" | "sharedWithMe" | "proxies" | "trash" | "users";

export function Sidebar({
  projects,
  active,
  shell,
  me,
  onSelect,
  onNewProject,
  onSendProject,
  onShareProject,
  onRenameProject,
  onDeleteProject,
  onNewProfile,
  view,
  onView,
  onSettings,
}: {
  projects: Project[];
  active: Project | null;
  shell: Shell;
  me: Me | null;
  onSelect: (p: Project | null) => void;
  onNewProject: (origin: "local" | "team") => void;
  /** Copy every profile in a local folder onto the server, into a project of
   *  the same name. */
  onSendProject: (p: Project) => void;
  /** Give every profile currently in a server project to somebody. */
  onShareProject: (p: Project) => void;
  onRenameProject: (p: Project) => void;
  onDeleteProject: (p: Project) => void;
  view: View;
  onView: (v: View) => void;
  onNewProfile: () => void;
  onSettings: () => void;
}) {
  const { t } = useI18n();
  const local = shell.mode === "local";
  const [menu, setMenu] = useState<string | null>(null);
  return (
    <aside className="sidebar">
      <div className="brand">Fury</div>

      <button className="primary newProfile" onClick={onNewProfile}>
        {t("bar.newProfile")}
      </button>

      <nav>
        {/* Sections first, projects under them: the sections are where an
            operator goes, the projects are what they filter by. */}
        {/* The same four everywhere. Working alone and working with a team
                are the same application doing the same job — one of them simply
                has other people in it — and an interface that rearranges itself
                around that makes the operator relearn where things are the day
                their team grows. */}
        {/* "Shared with me" only when there is a server to be shared from.
            On a machine working alone it would be a permanently empty room. */}
        {((local
          ? (["profiles", "proxies", "users", "trash"] as const)
          : (["profiles", "sharedWithMe", "proxies", "users", "trash"] as const)) as readonly View[]
        ).map((v) => (
              <button
                key={v}
                // Profiles is only "the current section" when no project is
                // filtering it. With one chosen, the highlight belongs on the
                // project — otherwise two rows claim to be where you are.
                className={
                  view === v && !(v === "profiles" && active) ? "nav active" : "nav"
                }
                onClick={() => {
                  // Choosing Profiles clears the filter. It is the master list:
                  // every profile on this machine, whatever project it is in.
                  if (v === "profiles") onSelect(null);
                  onView(v);
                }}
              >
                <span>{t(`nav.${v}` as never)}</span>
              </button>
            ))}
        {/* A heading per world, each with an ordinary +.
            The first attempt put two buttons under one heading and labelled the
            second "+тут", which meant nothing to anybody -- "что такое +тут?"
            was the whole of the feedback and it was right. A heading explains
            itself, and it also makes the badge on each row unnecessary: a
            folder under "On this machine" does not need to be told it is on
            this machine. */}
        {(local
          ? ([["local", "app.projects"]] as const)
          : ([
              ["team", "app.projectsTeam"],
              ["local", "app.projectsHere"],
            ] as const)
        ).map(([side, label]) => (
          <div key={side}>
            <div className="section" style={{ marginTop: "var(--s-3)" }}>
              {t(label as never)}
              <button
                className="linky"
                onClick={() => onNewProject(side)}
                title={t(side === "local" ? "app.newProjectHere" : "app.newProject")}
              >
                +
              </button>
            </div>
            {projects.filter((p) => p.origin === side).length === 0 && (
              <div className="empty">
                {side === "team" ? t("app.nothingShared") : t("app.noFoldersHere")}
              </div>
            )}
            {projects
              .filter((p) => p.origin === side)
              .map((p) => (
          <div key={p.id} className="projectRow">
            <button
              className={p.id === active?.id && view === "profiles" ? "nav active" : "nav"}
              onClick={() => {
                onSelect(p);
                onView("profiles");
              }}
              // Right-click is where renaming and deleting live: a row that
              // grows two buttons on hover moves the thing being aimed at.
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu(menu === p.id ? null : p.id);
              }}
            >
              <span className="ellipsis">{p.name}</span>
              <span className="count">{p.profile_count}</span>
            </button>
            {menu === p.id && (
              <div className="projectMenu">
                <button
                  className="ghost"
                  onClick={() => {
                    setMenu(null);
                    onRenameProject(p);
                  }}
                >
                  {t("proj.rename")}
                </button>
                {/* Whole-folder actions, because per profile does not scale.
                    Somebody with dozens of local folders is not going to open
                    each one, select everything and press two buttons; they are
                    going to ask why this is per profile, which is what
                    happened. */}
                {!local && p.origin === "local" && (
                  <button
                    className="ghost"
                    onClick={() => {
                      setMenu(null);
                      onSendProject(p);
                    }}
                  >
                    {t("proj.sendAll")}
                  </button>
                )}
                {!local && p.origin === "team" && (
                  <button
                    className="ghost"
                    onClick={() => {
                      setMenu(null);
                      onShareProject(p);
                    }}
                  >
                    {t("proj.shareAll")}
                  </button>
                )}
                <button
                  className="ghost"
                  onClick={() => {
                    setMenu(null);
                    onDeleteProject(p);
                  }}
                >
                  {t("proj.delete")}
                </button>
              </div>
            )}
          </div>
              ))}
          </div>
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
        <div className="footRow">
          <div className="muted small ellipsis" title={shell.machine_name}>
            {shell.machine_name}
            {!shell.native && ` · ${t("app.browserDev")}`}
          </div>
        {/* An icon, not a word: it sits under the machine name where a label
            competes with information, and a gear is the one glyph nobody has to
            learn. The accessible name carries the word instead. */}
        <button
          className="iconBtn"
          onClick={onSettings}
          title={t("app.settings")}
          aria-label={t("app.settings")}
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <circle cx="12" cy="12" r="3.2" stroke="currentColor" strokeWidth="1.6" />
            <path
              d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.6a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9v0a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </button>
        </div>
      </div>
    </aside>
  );
}
