// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useCallback, useEffect, useState } from "react";
import { api, type Perm, type Project } from "../api";
import { useI18n } from "../i18n";
import { Audit } from "./Audit";
import { useAsk } from "./Ask";

type Members = Awaited<ReturnType<typeof api.orgMembers>>;
type Grants = Awaited<ReturnType<typeof api.grants>>;

const ROLES = ["admin", "manager", "member"] as const;

/** What being let into a project means: see it, open the profiles in it, and
 *  edit them. Not `reveal_secrets`, not `manage_access`, not deletion — those
 *  stay a deliberate act on a row, which is the whole argument for the
 *  permission set existing. The server caps this by the recipient's role
 *  anyway (`role_ceiling`), so a generous list here cannot promote anyone. */
const MEMBER_PERMS = ["view", "launch", "edit_profile"] as Perm[];

/** The team, and who can reach what.
 *
 *  Two things happen here that happen nowhere else, and both are easy to get
 *  wrong by omission:
 *
 *  A member who has enrolled does not yet hold the organisation key. They can
 *  sign in, see the shape of the team, and decrypt nothing. That is a real
 *  state, not an error, and it is invisible unless this screen says so — so it
 *  is the first thing each row reports.
 *
 *  Handing the key over happens on this machine. The key is sealed to their
 *  published public key in Rust and only the result is sent. Nobody, including
 *  whoever runs the server, can do it on their behalf. */
export function Users({
  projects,
  local,
  onSignOut,
  onConnect,
}: {
  projects: Project[];
  /** Working alone. The tab is here all the same — it is where an account
   *  lives, and hiding it until one exists means the way to get one is a
   *  setting nobody opens. */
  local: boolean;
  onSignOut: () => void;
  onConnect: () => void;
}) {
  const { t, say } = useI18n();
  const { ask, dialog } = useAsk();
  const [team, setTeam] = useState<Members | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<string>("member");
  const [code, setCode] = useState<{ code: string; email: string } | null>(null);

  /** The server's folders, and only those.
   *
   *  Access is a thing the server keeps: a row in project_grants, checked by
   *  the guard on every call. A folder on this machine has no such row and no
   *  such id — the server has never heard of it — so asking to grant access to
   *  one is asking about a project that does not exist, and the server answers
   *  the way it answers every invisible project: 404, rendered here as
   *  "Not found, or you no longer have access."
   *
   *  Which is exactly what an owner saw, because this picker was fed the whole
   *  list. A fresh organisation has no folders on the server yet, so the only
   *  entry in it was "My profiles" from this machine, it was selected by
   *  default, and the one button on the screen answered with a permission
   *  error about a folder sitting in front of them. */
  const teamProjects = projects.filter((p) => p.origin === "team");
  const [project, setProject] = useState<string>(teamProjects[0]?.id ?? "");
  const [grants, setGrants] = useState<Grants | null>(null);

  // The list arrives after the first render and changes while the screen is
  // open — a folder created on the server, or the last one deleted. A selection
  // that is no longer in it would keep asking about a project nobody can see.
  useEffect(() => {
    if (!teamProjects.some((p) => p.id === project)) {
      setProject(teamProjects[0]?.id ?? "");
    }
  }, [projects]); // eslint-disable-line react-hooks/exhaustive-deps

  const load = useCallback(async () => {
    try {
      setTeam(await api.orgMembers());
    } catch (e) {
      setError(say(e));
    }
  }, []);

  const loadGrants = useCallback(async () => {
    if (!project) return;
    try {
      setGrants(await api.grants(project));
    } catch (e) {
      // Not fatal: a manager may reach the team screen without being allowed to
      // see who else can open a given project.
      setGrants(null);
    }
  }, [project]);

  useEffect(() => {
    void load();
  }, [load]);
  useEffect(() => {
    void loadGrants();
  }, [loadGrants]);

  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
      await load();
      await loadGrants();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  if (local) {
    return (
      <div className="teamPane">
        {/* The sequence, stated.
          This screen used to show a form and not a path: inviting is one of
          four steps, and the other three appear as buttons on a member's row —
          so an owner who is still the only member never saw them and could not
          tell what happens after they send a code. */}
      <ol className="steps">
        <li>{t("team.how1")}</li>
        <li>{t("team.how2")}</li>
        <li>{t("team.how3")}</li>
        <li>{t("team.how4")}</li>
      </ol>

      <h2 className="sectionTitle">{t("team.people")}</h2>
        <p className="hint" style={{ maxWidth: 620 }}>
          {t("team.aloneHere")}
        </p>
        <button className="primary" onClick={onConnect}>
          {t("team.connectToWork")}
        </button>
      </div>
    );
  }

  if (!team) {
    return <p className="empty pad">{error ?? t("team.loading")}</p>;
  }

  const granted = new Set(grants?.granted.map((g) => g.user_id) ?? []);
  const implicit = new Set(grants?.implicit.map((g) => g.user_id) ?? []);

  return (
    <div className="teamPane">
      {dialog}
      {error && <p className="error">{error}</p>}

      {/* The sequence, stated.
          This screen used to show a form and not a path: inviting is one of
          four steps, and the other three appear as buttons on a member's row —
          so an owner who is still the only member never saw them and could not
          tell what happens after they send a code. */}
      <ol className="steps">
        <li>{t("team.how1")}</li>
        <li>{t("team.how2")}</li>
        <li>{t("team.how3")}</li>
        <li>{t("team.how4")}</li>
      </ol>

      <h2 className="sectionTitle">{t("team.people")}</h2>
      <table className="grid">
        <thead>
          <tr>
            <th>{t("team.member")}</th>
            <th>{t("team.role")}</th>
            <th>{t("team.key")}</th>
            {project && <th>{t("team.access", { project: teamProjects.find((p) => p.id === project)?.name ?? "" })}</th>}
            <th />
          </tr>
        </thead>
        <tbody>
          {team.members.map((m) => (
            <tr key={m.user_id}>
              <td>
                <div className="name">{m.email}</div>
                {m.is_you && <div className="muted small">{t("team.you")}</div>}
              </td>
              <td className="muted">{t(roleKey(m.role))}</td>
              <td>
                {m.has_key ? (
                  <span className="state free">{t("team.hasKey")}</span>
                ) : (
                  <span className="state lock">{t("team.waitingForKey")}</span>
                )}
              </td>
              {project && (
                <td className="muted small">
                  {implicit.has(m.user_id)
                    ? t("team.everything")
                    : granted.has(m.user_id)
                      ? (grants?.granted.find((g) => g.user_id === m.user_id)?.permissions ?? []).join(", ")
                      : t("team.noAccess")}
                </td>
              )}
              <td className="actions">
                <div>
                  {/* One button for letting somebody in, and it does both halves.
                      Handing the key over and granting access were two buttons
                      on the same row, pressed one after the other, every time —
                      because a member who can decrypt and cannot reach a single
                      folder is nobody's intention. The second press was a step
                      the interface asked for and the work never needed.

                      Not the same operation underneath, and that is worth
                      knowing: the key is sealed to their public key HERE, on
                      this machine, because the server cannot do it. The grants
                      are ordinary server calls. What is shared is the moment an
                      owner decides this person is in. */}
                  {!m.has_key && (
                    <button
                      disabled={busy}
                      onClick={() =>
                        run(async () => {
                          await api.handOverKey(m.user_id, m.public_key);
                          // Every folder the team has, not just the one the
                          // picker happens to show. Access to one project out
                          // of six, chosen by whatever was selected above, is
                          // not what "let them in" means.
                          //
                          // Sequential rather than concurrent: each is audited,
                          // and a half-applied burst leaves an owner reading a
                          // failure with no way to tell which ones landed.
                          for (const p of teamProjects) {
                            await api.grantAccess(p.id, m.user_id, MEMBER_PERMS);
                          }
                        })
                      }
                    >
                      {teamProjects.length > 0 ? t("team.letIn") : t("team.giveKey")}
                    </button>
                  )}
                  {project && m.has_key && !implicit.has(m.user_id) && !granted.has(m.user_id) && (
                    <button
                      className="ghost"
                      disabled={busy}
                      onClick={() => run(() => api.grantAccess(project, m.user_id, MEMBER_PERMS))}
                    >
                      {t("team.grant")}
                    </button>
                  )}
                  {!m.is_you && (
                    <button
                      className="danger"
                      disabled={busy}
                      onClick={async () => {
                        const go = await ask({
                          title: t("team.remove"),
                          detail: t("team.confirmRemove", { email: m.email }),
                          confirmLabel: t("team.remove"),
                          danger: true,
                        });
                        if (go === null) return;
                        await run(() => api.removeMember(m.user_id));
                      }}
                    >
                      {t("team.removeShort")}
                    </button>
                  )}
                  {project && granted.has(m.user_id) && (
                    <button
                      className="ghost"
                      disabled={busy}
                      onClick={() => run(() => api.revokeAccess(project, m.user_id))}
                    >
                      {t("team.revoke")}
                    </button>
                  )}
                </div>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      {/* What the one button did, spelled out where it was pressed. A default
          that opens every folder is a reasonable default and a bad secret. */}
      {teamProjects.length > 0 && (
        <p className="hint" style={{ maxWidth: 620, marginTop: "var(--s-3)" }}>
          {t("team.letInHint")}
        </p>
      )}

      {teamProjects.length > 0 ? (
        <p className="hint" style={{ marginTop: "var(--s-3)" }}>
          {t("team.accessFor")}{" "}
          <select
            style={{ width: "auto" }}
            value={project}
            onChange={(e) => setProject(e.target.value)}
          >
            {teamProjects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </p>
      ) : (
        // Said rather than left blank. Step 4 above tells an owner to pick a
        // project and grant access; with no folder on the server there is
        // nothing to pick, and an instruction pointing at an absent control is
        // how somebody concludes the screen is broken.
        <p className="hint" style={{ maxWidth: 620, marginTop: "var(--s-3)" }}>
          {t("team.noTeamProjects")}
        </p>
      )}

      {team.members.length === 1 && (
        <p className="hint" style={{ maxWidth: 620 }}>{t("team.aloneOnServer")}</p>
      )}

      <h2 className="sectionTitle" style={{ marginTop: "var(--s-6)" }}>
        {t("team.invite")}
      </h2>
      <p className="hint">{t("team.inviteHint")}</p>
      <div className="row" style={{ maxWidth: 680 }}>
        <input
          type="email"
          placeholder={t("auth.email")}
          value={email}
          spellCheck={false}
          onChange={(e) => setEmail(e.target.value)}
        />
        <select style={{ width: "auto" }} value={role} onChange={(e) => setRole(e.target.value)}>
          {ROLES.map((r) => (
            <option key={r} value={r}>
              {t(roleKey(r))}
            </option>
          ))}
        </select>
        <button
          className="primary"
          disabled={busy || !email.includes("@")}
          onClick={() =>
            run(async () => {
              const out = await api.invite(email, role);
              setCode({ code: out.code, email });
              setEmail("");
            })
          }
        >
          {t("team.sendInvite")}
        </button>
      </div>

      {code && (
        <div className="notice" role="status" style={{ marginTop: "var(--s-3)" }}>
          <div>
            <div>{t("team.codeFor", { email: code.email })}</div>
            {/* Shown once. The server keeps only a hash of it, so there is no
                screen anywhere that can show it again. */}
            <div className="mono" style={{ fontSize: 16, margin: "var(--s-2) 0" }}>
              {code.code}
            </div>
            <div className="muted small">{t("team.codeOnce")}</div>
          </div>
          <button className="ghost" onClick={() => setCode(null)}>
            {t("app.dismiss")}
          </button>
        </div>
      )}

      <h2 className="sectionTitle" style={{ marginTop: "var(--s-6)" }}>
        {t("team.thisAccount")}
      </h2>
      <button className="ghost" onClick={onSignOut}>
        {t("app.signOut")}
      </button>
      <p className="hint" style={{ maxWidth: 620, marginTop: "var(--s-3)" }}>
        {t("team.rotateHint")}
      </p>
      <button
        className="ghost"
        disabled={busy}
        onClick={async () => {
          const go = await ask({
            title: t("team.rotate"),
            detail: t("team.confirmRotate"),
            confirmLabel: t("team.rotate"),
          });
          if (go === null) return;
          await run(() => api.removeMember(null));
        }}
      >
        {t("team.rotate")}
      </button>

      {team.invited.length > 0 && (
        <>
          <h2 className="sectionTitle" style={{ marginTop: "var(--s-6)" }}>
            {t("team.pending")}
          </h2>
          <table className="grid">
            <tbody>
              {team.invited.map((i) => (
                <tr key={i.email}>
                  <td>{i.email}</td>
                  <td className="muted">{t(roleKey(i.role))}</td>
                  <td className="muted small">
                    {t("team.expires", { when: new Date(i.expires_at).toLocaleString() })}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}

      {/* Who did what.
          Here rather than as a fifth entry in the sidebar, and that is
          deliberate: the sidebar's own comment says the same four appear
          everywhere so that an operator does not have to relearn the layout
          the day their team grows. Audit does not exist at all in local mode —
          there is nobody else to account for — so a tab that appeared and
          disappeared would be exactly the rearrangement that argument is
          against.

          Shown to owners and admins only. The server refuses everyone else,
          and offering a section that answers with a permission error is worse
          than not offering it. */}
      {(team.members.find((m) => m.is_you)?.role === "owner" ||
        team.members.find((m) => m.is_you)?.role === "admin") && (
        <>
          <h2 className="sectionTitle">{t("team.audit")}</h2>
          <Audit />
        </>
      )}
    </div>
  );
}

/** Roles arrive from the server as free text; a key built from one would render
 *  as the key rather than fail. */
function roleKey(role: string): "role.owner" | "role.admin" | "role.manager" | "role.member" {
  switch (role) {
    case "owner":
      return "role.owner";
    case "admin":
      return "role.admin";
    case "manager":
      return "role.manager";
    default:
      return "role.member";
  }
}
