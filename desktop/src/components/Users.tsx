// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useCallback, useEffect, useState } from "react";
import { api, type AccessRole, type Perm, type Project } from "../api";
import { useI18n, type Key } from "../i18n";
import { withStepUp } from "../stepUp";
import { AccessRoles } from "./AccessRoles";
import { Audit } from "./Audit";
import { Security } from "./Security";
import { TeamDomainLists } from "./TeamDomainLists";
import type { OrgDomainList } from "../api";
import { useAsk } from "./Ask";
import { Select } from "./Select";

type Members = Awaited<ReturnType<typeof api.orgMembers>>;
type Grants = Awaited<ReturnType<typeof api.grants>>;

const ROLES = ["admin", "manager", "member"] as const;

/** The page's tabs, in order. The last three are the owner's and admins'
 *  business; the server refuses everybody else there anyway. */
type UsersTab = "team" | "access" | "roles" | "sites" | "security" | "audit";
const TAB_LABEL: Record<UsersTab, Key> = {
  team: "team.tabTeam",
  access: "team.tabAccess",
  roles: "team.tabRoles",
  sites: "team.tabSites",
  security: "team.tabSecurity",
  audit: "team.tabAudit",
};
/** The tab last open, so coming back to Users lands where one left. */
const TAB_KEY = "fury.usersTab";

/** Where a newcomer downloads Fury, in the instructions sent with a code. */
const FURY_SITE = "https://furybrowser.dev";

/** What being let into a project means: see it, open the profiles in it, and
 *  edit them. Not `reveal_secrets`, not `manage_access`, not deletion — those
 *  stay a deliberate act on a row, which is the whole argument for the
 *  permission set existing. The server caps this by the recipient's role
 *  anyway (`role_ceiling`), so a generous list here cannot promote anyone. */
const MEMBER_PERMS = ["view", "launch", "edit_profile"] as Perm[];

/** Every flag the server knows, in the order `Perm::ALL` lists them
 *  (shared-rs/src/rbac.rs). The row's editor shows all ten; for a long time
 *  the seven past MEMBER_PERMS could only be set with a request to the API,
 *  which docs/14 said out loud and nobody was going to do. */
const ALL_PERMS: Perm[] = [
  "view",
  "launch",
  "edit_profile",
  "edit_fingerprint",
  "edit_proxy",
  "reveal_secrets",
  "export_cookies",
  "create_profile",
  "delete_profile",
  "manage_access",
];

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
  serverUrl,
  onSignOut,
  onConnect,
}: {
  projects: Project[];
  /** Working alone. The tab is here all the same — it is where an account
   *  lives, and hiding it until one exists means the way to get one is a
   *  setting nobody opens. */
  local: boolean;
  /** Where this team lives, written into the instructions sent with a code:
   *  a code is only valid on the server that issued it. */
  serverUrl: string | null;
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
  const [code, setCode] = useState<{ code: string; email: string; hours: number } | null>(null);
  const [tabState, setTabState] = useState<UsersTab>(() => {
    try {
      return (localStorage.getItem(TAB_KEY) as UsersTab | null) ?? "team";
    } catch {
      return "team";
    }
  });
  const pickTab = (x: UsersTab) => {
    setTabState(x);
    try {
      localStorage.setItem(TAB_KEY, x);
    } catch {
      // A private window: the tab is simply not remembered.
    }
  };
  // The invitation form, opened by its button rather than always on show.
  const [inviting, setInviting] = useState(false);
  // "copied" for two seconds, "failed" until the code goes away.
  const [copied, setCopied] = useState<"copied" | "failed" | null>(null);
  // The instructions to send with the code, open under it; and the same
  // copied/failed for the whole text.
  const [sending, setSending] = useState(false);
  // Whether this server confirms an invited address with a letter (server
  // 0018). Until it does, a code works for whoever types it first, and the
  // card under the code says so. Unknown counts as no.
  const [emailCheck, setEmailCheck] = useState(false);
  useEffect(() => {
    if (local || !serverUrl) return;
    void api
      .serverInfo(serverUrl)
      .then((i) => setEmailCheck(!!i.email_check))
      .catch(() => setEmailCheck(false));
  }, [local, serverUrl]);
  const [textCopied, setTextCopied] = useState<"copied" | "failed" | null>(null);

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
  // Which project an invitation pre-ticks for Let in; its own picker on the
  // Team tab, starting from the first project.
  const [inviteProject, setInviteProject] = useState<string>(teamProjects[0]?.id ?? "");
  const [grants, setGrants] = useState<Grants | null>(null);
  // Whom the "add from the team" picker has chosen, and on which of the
  // organisation's roles ("" is MEMBER_PERMS).
  const [adding, setAdding] = useState("");
  const [addingRole, setAddingRole] = useState("");
  // The organisation's own roles (server 0017), for every picker on this screen.
  const [roles, setRoles] = useState<AccessRole[]>([]);
  // Which of them the next invitation is for; remembered with it, see INVITED.
  const [inviteRole, setInviteRole] = useState("");
  // Somebody being let in, and the folders ticked for them so far.
  const [admitting, setAdmitting] = useState<{
    member: Members["members"][number];
    picked: Set<string>;
    accessRole: string;
    // The verification code for the key the server lists for them, and
    // whether the owner has said the person read out the same one.
    code: string | null;
    confirmed: boolean;
  } | null>(null);
  // The organisation's domain lists, for attaching to a grant in the row.
  const [orgLists, setOrgLists] = useState<OrgDomainList[]>([]);
  useEffect(() => {
    if (local) return;
    void api.orgDomainLists().then(setOrgLists).catch(() => setOrgLists([]));
  }, [local, team]);

  // The list arrives after the first render and changes while the screen is
  // open — a folder created on the server, or the last one deleted. A selection
  // that is no longer in it would keep asking about a project nobody can see.
  useEffect(() => {
    if (!teamProjects.some((p) => p.id === project)) {
      setProject(teamProjects[0]?.id ?? "");
    }
    if (inviteProject && !teamProjects.some((p) => p.id === inviteProject)) {
      setInviteProject(teamProjects[0]?.id ?? "");
    }
  }, [projects]); // eslint-disable-line react-hooks/exhaustive-deps

  const load = useCallback(async () => {
    try {
      setTeam(await api.orgMembers());
    } catch (e) {
      setError(say(e));
    }
    // Not fatal either way: an older server has no roles, and the pickers
    // simply do not appear.
    try {
      setRoles(await api.accessRoles());
    } catch {
      setRoles([]);
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

  // A rotation carries every profile, bundle and saved login over to the new
  // key, except what the old key could not open: lost already, to a rotation
  // by a Fury that did not carry them. Said, because it is otherwise silent.
  const noteUnreadable = (out: { unreadable?: number } | null) => {
    if (out?.unreadable) setError(t("team.rotateUnreadable", { n: String(out.unreadable) }));
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

  const myRole = team.members.find((m) => m.is_you)?.role;
  const isAdmin = myRole === "owner" || myRole === "admin";
  const me = team.members.find((m) => m.is_you);
  // Enrolled and not yet let in: the one thing on this page that waits on
  // the person reading it, so it is counted on the Team tab.
  const waiting = team.members.filter((m) => !m.has_key).length;
  const tabs: UsersTab[] = isAdmin
    ? ["team", "access", "roles", "sites", "security", "audit"]
    : ["team", "access", "roles"];
  const tab = tabs.includes(tabState) ? tabState : "team";

  const openAdmit = (member: Members["members"][number], picked: Set<string>) => {
    setAdmitting({ member, picked, accessRole: invitedRole(member.email, roles), code: null, confirmed: false });
    void api.verifyCode(member.public_key).then(
      (code) => setAdmitting((a) => (a && a.member.user_id === member.user_id ? { ...a, code } : a)),
      (e) => setError(say(e)),
    );
  };
  const granted = new Set(grants?.granted.map((g) => g.user_id) ?? []);

  return (
    <div className="teamPane">
      {dialog}
      {admitting && (
        <div
          className="scrim"
          onMouseDown={(e) => e.target === e.currentTarget && setAdmitting(null)}
        >
          <form
            className="palette"
            style={{ maxHeight: "none" }}
            role="dialog"
            aria-modal="true"
            onSubmit={(e) => {
              e.preventDefault();
              const { member, picked, accessRole } = admitting;
              setAdmitting(null);
              void run(async () => {
                await api.handOverKey(member.user_id, member.public_key);
                // Sequential rather than concurrent: each is audited, and a
                // half-applied burst leaves an owner reading a failure with no
                // way to tell which ones landed.
                for (const p of teamProjects) {
                  if (picked.has(p.id)) {
                    await api.grantAccess(p.id, member.user_id, MEMBER_PERMS, undefined, accessRole || null);
                  }
                }
              });
            }}
          >
            <div style={{ padding: "var(--s-5) var(--s-5) var(--s-3)" }}>
              <div className="name" style={{ marginBottom: "var(--s-2)" }}>
                {t("team.letInTitle", { email: admitting.member.email })}
              </div>
              {/* The check that the key about to be trusted is theirs. An
                  invitation code works for whoever types it first, so the
                  row alone cannot say who enrolled (14.10.2026). */}
              <div className="notice" style={{ margin: "0 0 var(--s-3)", display: "block" }}>
                <div className="small">{t("team.verifyAsk", { email: admitting.member.email })}</div>
                <div className="mono" style={{ fontSize: 22, letterSpacing: 2, margin: "var(--s-1) 0" }}>
                  {admitting.code ?? "…"}
                </div>
                <label className="row" style={{ gap: "var(--s-2)", alignItems: "flex-start" }}>
                  <input
                    type="checkbox"
                    style={{ width: "auto", marginTop: 3 }}
                    checked={admitting.confirmed}
                    onChange={(e) => setAdmitting({ ...admitting, confirmed: e.target.checked })}
                  />
                  <span className="small">{t("team.verifyConfirm")}</span>
                </label>
                <div className="hint small" style={{ marginTop: "var(--s-1)" }}>{t("team.verifyMismatch")}</div>
              </div>
              {teamProjects.length > 0 && (
                <p className="hint" style={{ margin: 0 }}>
                  {t("team.letInPick")}
                </p>
              )}
              {/* Said, because the ticks below would otherwise look like they
                  limit somebody they cannot: an owner or admin reaches every
                  folder by role (rbac.rs, has_implicit_project_access). */}
              {(admitting.member.role === "admin" || admitting.member.role === "owner") && (
                <p className="hint" style={{ margin: "var(--s-2) 0 0" }}>
                  {t("team.letInAdminSeesAll")}
                </p>
              )}
              <div style={{ marginTop: "var(--s-3)", display: "grid", gap: "var(--s-2)" }}>
                {teamProjects.map((p) => (
                  <label key={p.id} style={{ display: "flex", gap: "var(--s-2)", alignItems: "center" }}>
                    <input
                      type="checkbox"
                      style={{ width: "auto" }}
                      checked={admitting.picked.has(p.id)}
                      onChange={(e) => {
                        const picked = new Set(admitting.picked);
                        if (e.target.checked) picked.add(p.id);
                        else picked.delete(p.id);
                        setAdmitting({ ...admitting, picked });
                      }}
                    />
                    {p.name}
                  </label>
                ))}
              </div>
              {roles.length > 0 && admitting.member.role !== "admin" && admitting.member.role !== "owner" && (
                <label className="row" style={{ gap: "var(--s-2)", marginTop: "var(--s-3)" }}>
                  <span className="small">{t("ar.withRights")}</span>
                  <Select
                    style={{ width: "auto" }}
                    value={admitting.accessRole}
                    onChange={(e) => setAdmitting({ ...admitting, accessRole: e.target.value })}
                  >
                    <option value="">{t("ar.default")}</option>
                    {roles.map((r) => (
                      <option key={r.id} value={r.id}>
                        {r.name}
                      </option>
                    ))}
                  </Select>
                </label>
              )}
            </div>
            <div className="modalFoot">
              <div className="spacer" />
              <button type="button" className="ghost" onClick={() => setAdmitting(null)}>
                {t("ui.cancel")}
              </button>
              <button type="submit" className="primary" disabled={!admitting.confirmed || !admitting.code}>
                {admitting.picked.size > 0 ? t("team.letIn") : t("team.giveKey")}
              </button>
            </div>
          </form>
        </div>
      )}
      {error && <p className="error">{error}</p>}

      {/* Tabs rather than one long page (the owner, 14.10.2026): the team,
          who reaches which project, what the roles mean, and the owner's and
          admins' own business -- sites, security, the log -- each on its own,
          so that each answers one question. Signing out sits at the end of the
          bar, where it is found without scrolling past everything. */}
      <div className="tabs pageTabs" role="tablist">
        {tabs.map((x) => (
          <button key={x} role="tab" aria-selected={tab === x} onClick={() => pickTab(x)}>
            {t(TAB_LABEL[x])}
            {x === "team" && waiting > 0 && <span className="tabBadge">{waiting}</span>}
          </button>
        ))}
        <div className="spacer" />
        <span className="muted small pageTabsWho">{me?.email}</span>
        <button className="ghost small" onClick={onSignOut}>
          {t("app.signOut")}
        </button>
      </div>

      <div className="tabBody">
      {tab === "team" && (
        <>
          {/* The four steps, large, while there is nobody to apply them to;
              folded away once the team exists and they are known. */}
          {team.members.length === 1 ? (
            <div className="panel">
              <div className="panelTitle">{t("team.howTitle")}</div>
              <ol className="steps" style={{ margin: 0 }}>
                <li>{t("team.how1")}</li>
                <li>{t("team.how2")}</li>
                <li>{t("team.how3")}</li>
                <li>{t("team.how4")}</li>
              </ol>
            </div>
          ) : null}

          <div className="toolbarRow">
            <h2 className="sectionTitle" style={{ margin: 0 }}>
              {t("team.people")} <span className="count">{team.members.length}</span>
            </h2>
            <div className="spacer" />
            {!inviting && !code && (
              <button className="primary" onClick={() => setInviting(true)}>
                {t("team.inviteButton")}
              </button>
            )}
          </div>

          {waiting > 0 && !inviting && !code && (
            <p className="hint small" style={{ marginTop: 0 }}>{t("team.waitingNote", { n: waiting })}</p>
          )}

          {(inviting || code) && (
            <div className="panel">
              <div className="toolbarRow" style={{ marginBottom: "var(--s-2)" }}>
                <div className="panelTitle" style={{ margin: 0 }}>{t("team.invite")}</div>
                <div className="spacer" />
                {!code && (
                  <button className="ghost small" onClick={() => setInviting(false)}>
                    {t("ui.cancel")}
                  </button>
                )}
              </div>
              {!code && (
                <>
                  <p className="hint small" style={{ marginTop: 0 }}>{t("team.inviteHint")}</p>
                  {/* Labelled fields in one row, the button under them: four
                      unlabelled boxes and a button that wrapped to a line of
                      its own read as a filter bar, not a form. */}
                  <div className="inviteGrid">
                    <label>
                      <span>{t("team.emailLabel")}</span>
                      <input
                        type="email"
                        placeholder="name@example.com"
                        value={email}
                        spellCheck={false}
                        autoFocus
                        onChange={(e) => setEmail(e.target.value)}
                      />
                    </label>
                    <label>
                      <span>{t("team.role")}</span>
                      <Select value={role} onChange={(e) => setRole(e.target.value)}>
                        {/* An admin cannot invite another admin (create_invitation
                            in api.rs), so the option is not offered to them. */}
                        {(myRole === "owner" ? ROLES : ROLES.filter((r) => r !== "admin")).map((r) => (
                          <option key={r} value={r}>
                            {t(roleKey(r))}
                          </option>
                        ))}
                      </Select>
                    </label>
                    {teamProjects.length > 0 && role !== "admin" && (
                      <label>
                        <span>{t("team.inviteProject")}</span>
                        <Select value={inviteProject} onChange={(e) => setInviteProject(e.target.value)}>
                          <option value="">{t("team.inviteNoProject")}</option>
                          {teamProjects.map((p) => (
                            <option key={p.id} value={p.id}>
                              {p.name}
                            </option>
                          ))}
                        </Select>
                      </label>
                    )}
                    {roles.length > 0 && inviteProject && role !== "admin" && (
                      <label>
                        <span>{t("team.inviteRights")}</span>
                        <Select value={inviteRole} onChange={(e) => setInviteRole(e.target.value)}>
                          <option value="">{t("ar.default")}</option>
                          {roles.map((r) => (
                            <option key={r.id} value={r.id}>
                              {r.name}
                            </option>
                          ))}
                        </Select>
                      </label>
                    )}
                  </div>
                  <p className="hint small">
                    {t(roleWhat(role))}
                    {inviteProject && role !== "admin" ? " " + t("team.inviteToHint") : ""}
                  </p>
                  <button
                    className="primary"
                    disabled={busy || !email.includes("@")}
                    onClick={() =>
                      run(async () => {
                        const out = await api.invite(email, role);
                        if (inviteProject && role !== "admin") rememberInvite(email, inviteProject, inviteRole);
                        setCode({ code: out.code, email, hours: out.expires_in_hours });
                        setCopied(null);
                        setSending(false);
                        setTextCopied(null);
                        setEmail("");
                      })
                    }
                  >
                    {t("team.sendInvite")}
                  </button>
                </>
              )}
              {code && (
                <div className="notice" role="status" style={{ marginTop: "var(--s-3)", alignItems: "flex-start" }}>
                  <div style={{ flex: 1 }}>
                    <div>{t("team.codeFor", { email: code.email })}</div>
                    {/* Shown once. The server keeps only a hash of it. */}
                    <div className="row" style={{ gap: "var(--s-2) var(--s-3)", margin: "var(--s-2) 0", flexWrap: "wrap" }}>
                      {/* One line: a code broken across lines gets retyped with the break. */}
                      <span className="mono" style={{ fontSize: 16, userSelect: "all", whiteSpace: "nowrap" }}>
                        {code.code}
                      </span>
                      <button
                        className="alt"
                        onClick={async () => {
                          try {
                            await navigator.clipboard.writeText(code.code);
                            setCopied("copied");
                            window.setTimeout(() => setCopied((c) => (c === "copied" ? null : c)), 2000);
                          } catch {
                            // Said out loud: this code cannot be shown a second time,
                            // so a button that silently did nothing would be believed.
                            setCopied("failed");
                          }
                        }}
                      >
                        {copied === "copied" ? t("pw.copied") : t("pw.copy")}
                      </button>
                      <button className="alt" aria-expanded={sending} onClick={() => setSending((s) => !s)}>
                        {t("team.send")}
                      </button>
                    </div>
                    {copied === "failed" && <div className="small">{t("team.copyFailed")}</div>}
                    <div className="muted small">{t("team.codeOnce")}</div>
                    <div className={emailCheck ? "muted small" : "warn small"} style={{ marginTop: "var(--s-1)" }}>
                      {emailCheck ? t("team.codeMailed", { email: code.email }) : t("team.codeBearer")}
                    </div>
                    {sending && (() => {
                      // Everything the person needs on the other end, in the order
                      // the screens ask for it: where to get Fury, which button,
                      // which server, the code, the password warning, and that they
                      // still have to be let in -- the step a newcomer cannot see.
                      const text = t("team.msgBody", {
                        email: code.email,
                        server: (serverUrl ?? "").replace(/\/+$/, ""),
                        code: code.code,
                        hours: code.hours,
                        letter: emailCheck ? t("team.msgLetter", { email: code.email }) : "",
                      });
                      const subject = t("team.msgSubject");
                      return (
                        <div style={{ marginTop: "var(--s-3)" }}>
                          <div className="small" style={{ marginBottom: "var(--s-1)" }}>
                            {t("team.sendHint", { email: code.email })}
                          </div>
                          <textarea
                            readOnly
                            rows={13}
                            value={text}
                            style={{ width: "100%", fontSize: 12 }}
                            onFocus={(e) => e.currentTarget.select()}
                          />
                          <div className="row" style={{ gap: "var(--s-2)", marginTop: "var(--s-2)", flexWrap: "wrap" }}>
                            <button
                              className="alt"
                              onClick={() =>
                                void api
                                  .openUrl(
                                    `mailto:${encodeURIComponent(code.email)}?subject=${encodeURIComponent(subject)}&body=${encodeURIComponent(text)}`,
                                  )
                                  .catch((e) => setError(say(e)))
                              }
                            >
                              {t("team.sendMail")}
                            </button>
                            <button
                              className="alt"
                              onClick={() =>
                                void api
                                  .openUrl(
                                    `https://t.me/share/url?url=${encodeURIComponent(FURY_SITE)}&text=${encodeURIComponent(text)}`,
                                  )
                                  .catch((e) => setError(say(e)))
                              }
                            >
                              {t("team.sendTelegram")}
                            </button>
                            <button
                              className="alt"
                              onClick={async () => {
                                try {
                                  await navigator.clipboard.writeText(text);
                                  setTextCopied("copied");
                                  window.setTimeout(() => setTextCopied((c) => (c === "copied" ? null : c)), 2000);
                                } catch {
                                  setTextCopied("failed");
                                }
                              }}
                            >
                              {textCopied === "copied" ? t("team.textCopied") : t("team.copyText")}
                            </button>
                          </div>
                          {textCopied === "failed" && <div className="small">{t("team.copyTextFailed")}</div>}
                        </div>
                      );
                    })()}
                  </div>
                  <button
                    className="ghost"
                    onClick={() => {
                      setCode(null);
                      setCopied(null);
                      setSending(false);
                      setTextCopied(null);
                      setInviting(false);
                    }}
                  >
                    {t("app.dismiss")}
                  </button>
                </div>
              )}
            </div>
          )}

          <table className="grid">
            <thead>
              <tr>
                <th>{t("team.member")}</th>
                <th>{t("team.role")}</th>
                <th>{t("team.key")}</th>
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
                  <td className="muted">
                    {/* A select only where the server would accept the change
                        (api.rs, set_member_role): never the owner, never yourself,
                        and an admin handles managers and members only. */}
                    {canChangeRole(myRole, m) ? (
                      <Select
                        style={{ width: "auto" }}
                        value={m.role}
                        disabled={busy}
                        aria-label={t("team.role")}
                        onChange={async (e) => {
                          const to = e.target.value;
                          if (to === "admin") {
                            const go = await ask({
                              title: t("team.makeAdmin", { email: m.email }),
                              detail: t("team.makeAdminDetail"),
                              confirmLabel: t("team.makeAdminConfirm"),
                            });
                            if (go === null) return;
                          }
                          await run(() => withStepUp(ask, t, () => api.setMemberRole(m.user_id, to)));
                        }}
                      >
                        {(myRole === "owner" ? ROLES : ROLES.filter((r) => r !== "admin")).map((r) => (
                          <option key={r} value={r}>
                            {t(roleKey(r))}
                          </option>
                        ))}
                      </Select>
                    ) : (
                      <span title={t(roleWhat(m.role))}>{t(roleKey(m.role))}</span>
                    )}
                  </td>
                  <td>
                    {m.has_key ? (
                      <span className="state free">{t("team.hasKey")}</span>
                    ) : (
                      <span className="state lock">{t("team.waitingForKey")}</span>
                    )}
                  </td>
                  <td className="actions">
                    <div>
                      {/* Letting somebody in hands over the key and asks which
                          projects to open; a project they were invited into is
                          already ticked. Which folders is asked, not assumed: a
                          tester who invited somebody for one client found them
                          looking at all of them (01.10.2026). */}
                      {isAdmin && !m.has_key && (
                        <button
                          disabled={busy}
                          onClick={() =>
                            teamProjects.length > 0
                              ? openAdmit(m, new Set(invitedFor(m.email)))
                              : openAdmit(m, new Set())
                          }
                        >
                          {teamProjects.length > 0 ? t("team.letIn") : t("team.giveKey")}
                        </button>
                      )}
                      {/* Handing the team over (docs/25): the owner's own act, to
                          an admin who already holds the key, with the password
                          again and the authenticator code when there is one. */}
                      {myRole === "owner" && m.role === "admin" && m.has_key && (
                        <button
                          className="ghost"
                          disabled={busy}
                          onClick={async () => {
                            const password = await ask({
                              title: t("team.handOver", { email: m.email }),
                              detail: t("team.handOverDetail", { email: m.email }),
                              placeholder: t("auth.password"),
                              confirmLabel: t("team.handOverConfirm"),
                              danger: true,
                              secret: true,
                            });
                            if (!password) return;
                            await run(async () => {
                              try {
                                await api.handOverOwnership(m.user_id, password, null);
                              } catch (e) {
                                if ((e as { code?: string } | null)?.code !== "err.totpRequired") throw e;
                                const code = await ask({ title: t("team.handOver", { email: m.email }), detail: t("auth.codeWhy"), placeholder: t("auth.code"), confirmLabel: t("team.handOverConfirm"), danger: true });
                                if (!code) return;
                                await api.handOverOwnership(m.user_id, password, code);
                              }
                            });
                          }}
                        >
                          {t("team.handOverShort")}
                        </button>
                      )}
                      {/* Removing follows the role rule: not yourself, never
                          the owner, an admin only below admin. A member saw
                          "Remove" beside the owner (14.10.2026). */}
                      {canChangeRole(myRole, m) && (
                        <button
                          className="ghost danger"
                          disabled={busy}
                          onClick={async () => {
                            const go = await ask({
                              title: t("team.remove"),
                              detail: t("team.confirmRemove", { email: m.email }),
                              confirmLabel: t("team.remove"),
                              danger: true,
                            });
                            if (go === null) return;
                            await run(async () => noteUnreadable(await withStepUp(ask, t, () => api.removeMember(m.user_id))));
                          }}
                        >
                          {t("team.removeShort")}
                        </button>
                      )}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {team.invited.length > 0 && (
            <>
              <h2 className="sectionTitle" style={{ marginTop: "var(--s-5)" }}>
                {t("team.pending")}
              </h2>
              <table className="grid">
                <tbody>
                  {team.invited.map((i) => (
                    <tr key={i.email}>
                      <td>{i.email}</td>
                      <td className="muted">{t(roleKey(i.role))}</td>
                      <td className="muted small">
                        {t("team.expires", { when: new Date(i.expires_at).toLocaleString(undefined, { dateStyle: "short", timeStyle: "short" }) })}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </>
          )}

          {team.members.length > 1 && (
            <details className="howTo">
              <summary>{t("team.howTitle")}</summary>
              <ol className="steps">
                <li>{t("team.how1")}</li>
                <li>{t("team.how2")}</li>
                <li>{t("team.how3")}</li>
                <li>{t("team.how4")}</li>
              </ol>
            </details>
          )}
        </>
      )}

      {tab === "access" && (
        <>
          <div className="toolbarRow">
            <span className="small">{t("team.projectPick")}</span>
            {teamProjects.length > 0 && (
              <Select style={{ width: "auto" }} value={project} onChange={(e) => setProject(e.target.value)}>
                {teamProjects.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
              </Select>
            )}
          </div>
          <p className="hint small" style={{ marginTop: 0 }}>{t("team.accessHint")}</p>
          {teamProjects.length === 0 ? (
            // Said rather than left blank: with no folder on the server there is
            // nothing to give anybody access to.
            <p className="hint" style={{ maxWidth: 620 }}>{t("team.noTeamProjects")}</p>
          ) : (
            <>
              <table className="grid">
                <thead>
                  <tr>
                    <th>{t("team.member")}</th>
                    <th>{t("team.rights")}</th>
                    <th />
                  </tr>
                </thead>
                <tbody>
                  {team.members
                    .filter((m) => m.role === "owner" || m.role === "admin" || granted.has(m.user_id))
                    .map((m) => {
                      const g = grants?.granted.find((x) => x.user_id === m.user_id);
                      const have = new Set<Perm>(g?.permissions ?? []);
                      const attached = new Set(g?.domain_lists ?? []);
                      const everything = m.role === "owner" || m.role === "admin";
                      const linked = roles.find((r) => r.id === g?.access_role);
                      return (
                        <tr key={m.user_id}>
                          <td style={{ minWidth: 180 }}>
                            <div className="name">{m.email}</div>
                            <div className="muted small">{t(roleKey(m.role))}</div>
                          </td>
                          <td>
                            {everything ? (
                              <span className="muted small">{t("team.everything")}</span>
                            ) : (
                              <>
                                {/* Each box saves on change. `view` is always on:
                                    a grant without it lets somebody act on
                                    profiles they cannot see. `manage_access` is
                                    greyed for a member, because the server drops
                                    it for that role (role_ceiling). */}
                                {/* A role's boxes are the role's: ticking one
                                    here would quietly unlink this person from it,
                                    so they are locked and "by hand" is the way out,
                                    keeping the boxes as they are. */}
                                {roles.length > 0 && (
                                  <div className="row" style={{ gap: "var(--s-2)", marginBottom: "var(--s-2)", flexWrap: "wrap" }}>
                                    <span className="small">{t("ar.inRow")}</span>
                                    <Select
                                      style={{ width: "auto" }}
                                      value={linked?.id ?? ""}
                                      disabled={busy}
                                      onChange={(e) => {
                                        const id = e.target.value;
                                        void run(() =>
                                          id
                                            ? api.grantAccess(project, m.user_id, [], undefined, id)
                                            : api.grantAccess(project, m.user_id, g?.permissions ?? MEMBER_PERMS, undefined, null),
                                        );
                                      }}
                                    >
                                      <option value="">{t("ar.manual")}</option>
                                      {roles.map((r) => (
                                        <option key={r.id} value={r.id}>
                                          {r.name}
                                        </option>
                                      ))}
                                    </Select>
                                    {linked && <span className="muted small">{t("ar.linkedHint")}</span>}
                                  </div>
                                )}
                                <div className="permsGrid">
                                  {ALL_PERMS.map((p) => {
                                    const locked =
                                      !!linked || p === "view" || (p === "manage_access" && m.role === "member");
                                    return (
                                      <label key={p} className="row" style={{ gap: 6, alignItems: "flex-start" }}>
                                        <input
                                          type="checkbox"
                                          style={{ width: 13, height: 13, marginTop: 2, accentColor: "var(--accent)" }}
                                          checked={p === "view" || have.has(p)}
                                          disabled={busy || locked}
                                          onChange={(e) => {
                                            const next = new Set(have);
                                            next.add("view");
                                            if (e.target.checked) next.add(p);
                                            else next.delete(p);
                                            void run(() =>
                                              api.grantAccess(
                                                project,
                                                m.user_id,
                                                ALL_PERMS.filter((x) => next.has(x)),
                                                [...attached],
                                                null,
                                              ),
                                            );
                                          }}
                                        />
                                        <span className="small">{t(("perm." + p) as Key)}</span>
                                      </label>
                                    );
                                  })}
                                </div>
                                {orgLists.length > 0 && (
                                  <div className="row" style={{ flexWrap: "wrap", gap: "var(--s-2)", marginTop: "var(--s-2)" }}>
                                    <span className="muted small">{t("tdl.applies")}</span>
                                    {orgLists.map((l) => (
                                      <label key={l.id} className="row" style={{ gap: 4 }}>
                                        <input
                                          type="checkbox"
                                          style={{ width: 13, height: 13, accentColor: "var(--accent)" }}
                                          checked={attached.has(l.id)}
                                          disabled={busy}
                                          onChange={(e) => {
                                            const next = new Set(attached);
                                            if (e.target.checked) next.add(l.id);
                                            else next.delete(l.id);
                                            void run(() => api.grantAccess(project, m.user_id, g?.permissions ?? MEMBER_PERMS, [...next]));
                                          }}
                                        />
                                        <span className="small">{l.name}</span>
                                      </label>
                                    ))}
                                  </div>
                                )}
                              </>
                            )}
                          </td>
                          <td className="actions">
                            {!everything && (
                              <button
                                className="ghost"
                                disabled={busy}
                                onClick={() => run(() => api.revokeAccess(project, m.user_id))}
                              >
                                {t("team.removeFromProject")}
                              </button>
                            )}
                          </td>
                        </tr>
                      );
                    })}
                </tbody>
              </table>

              {/* From the team: anybody let in, not owner or admin (they see every
                  project already), and not here yet. Somebody still waiting for
                  the key is added when they are let in, from the invite below. */}
              {(() => {
                const addable = team.members.filter(
                  (m) => m.has_key && m.role !== "owner" && m.role !== "admin" && !granted.has(m.user_id),
                );
                return (
                  <div className="row" style={{ marginTop: "var(--s-3)", gap: "var(--s-2)", flexWrap: "wrap" }}>
                    <span className="small">{t("team.addFromTeam")}</span>
                    {addable.length > 0 ? (
                      <>
                        <Select
                          style={{ width: "auto" }}
                          value={adding}
                          onChange={(e) => setAdding(e.target.value)}
                        >
                          <option value="">{t("team.pickMember")}</option>
                          {addable.map((m) => (
                            <option key={m.user_id} value={m.user_id}>
                              {m.email}
                            </option>
                          ))}
                        </Select>
                        {roles.length > 0 && (
                          <Select
                            style={{ width: "auto" }}
                            value={addingRole}
                            aria-label={t("ar.inRow")}
                            onChange={(e) => setAddingRole(e.target.value)}
                          >
                            <option value="">{t("ar.default")}</option>
                            {roles.map((r) => (
                              <option key={r.id} value={r.id}>
                                {r.name}
                              </option>
                            ))}
                          </Select>
                        )}
                        <button
                          className="primary"
                          disabled={busy || !adding}
                          onClick={() =>
                            run(async () => {
                              await api.grantAccess(project, adding, MEMBER_PERMS, undefined, addingRole || null);
                              setAdding("");
                            })
                          }
                        >
                          {t("team.add")}
                        </button>
                      </>
                    ) : (
                      <span className="muted small">{t("team.nobodyToAdd")}</span>
                    )}
                  </div>
                );
              })()}
            </>
          )}
        </>
      )}

      {tab === "roles" && (
        <>
          <h2 className="sectionTitle">{t("team.teamRoles")}</h2>
          <p className="hint small" style={{ marginTop: 0, maxWidth: 720 }}>{t("team.teamRolesHint")}</p>
          <div className="roleCards">
            {(["owner", "admin", "manager", "member"] as const).map((r) => (
              <div key={r} className="roleCard">
                <div className="name">{t(roleKey(r))}</div>
                {/* The sentence without its "Owner:" lead: the card's title says it. */}
                <div className="hint small">{capitalise(t(roleWhat(r)).replace(/^[^:]+:\s*/, ""))}</div>
              </div>
            ))}
          </div>
          {isAdmin && (
            <AccessRoles
              roles={roles}
              allPerms={ALL_PERMS}
              onChanged={async () => {
                await load();
                await loadGrants();
              }}
            />
          )}
        </>
      )}

      {tab === "sites" && isAdmin && <TeamDomainLists canEdit />}

      {tab === "security" && isAdmin && (
        <>
          <Security isOwner={myRole === "owner"} />
          <h2 className="sectionTitle" style={{ marginTop: "var(--s-6)" }}>{t("team.rotate")}</h2>
      <p className="hint" style={{ maxWidth: 620, marginTop: 0 }}>
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
          await run(async () => noteUnreadable(await api.removeMember(null)));
        }}
      >
        {t("team.rotate")}
      </button>
        </>
      )}

      {tab === "audit" && isAdmin && <Audit />}
      </div>
    </div>
  );
}

/** Roles arrive from the server as free text; a key built from one would render
 *  as the key rather than fail. */
/** Whether the server would let `me` change `m`'s role. Mirrors
 *  set_member_role in server/src/api.rs; the server decides regardless. */
function canChangeRole(me: string | undefined, m: { role: string; is_you: boolean }): boolean {
  if (m.is_you || m.role === "owner") return false;
  if (me === "owner") return true;
  return me === "admin" && m.role !== "admin";
}

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

function capitalise(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

/** What a role means, in the words of the server's checks: api.rs
 *  (create_project, create_invitation, hand_over_key, set_member_role,
 *  list_audit), security.rs (put_policy) and role_ceiling in shared-rs. */
function roleWhat(role: string): "role.ownerWhat" | "role.adminWhat" | "role.managerWhat" | "role.memberWhat" {
  switch (role) {
    case "owner":
      return "role.ownerWhat";
    case "admin":
      return "role.adminWhat";
    case "manager":
      return "role.managerWhat";
    default:
      return "role.memberWhat";
  }
}

/** Which projects somebody was invited into, remembered on this machine until
 *  they are let in, so the Let in dialog has those projects ticked already.
 *  Local, because it is a convenience for the person who sent the invite and
 *  not something the server needs to hold. */
const INVITED = "fury.invitedFor";

function invitedFor(email: string): string[] {
  try {
    const all = JSON.parse(localStorage.getItem(INVITED) ?? "{}") as Record<string, string[]>;
    return all[email.toLowerCase()] ?? [];
  } catch {
    return [];
  }
}

function rememberInvite(email: string, project: string, accessRole: string) {
  try {
    const all = JSON.parse(localStorage.getItem(INVITED) ?? "{}") as Record<string, string[]>;
    const key = email.toLowerCase();
    all[key] = Array.from(new Set([...(all[key] ?? []), project]));
    localStorage.setItem(INVITED, JSON.stringify(all));
    const roles = JSON.parse(localStorage.getItem(INVITED_ROLE) ?? "{}") as Record<string, string>;
    roles[key] = accessRole;
    localStorage.setItem(INVITED_ROLE, JSON.stringify(roles));
  } catch {
    // A private window: the project is simply not pre-ticked.
  }
}

/** The organisation role the invitation was sent on, kept beside the projects
 *  and for the same reason. One that has since been deleted is dropped. */
const INVITED_ROLE = "fury.invitedRole";

function invitedRole(email: string, roles: AccessRole[]): string {
  try {
    const all = JSON.parse(localStorage.getItem(INVITED_ROLE) ?? "{}") as Record<string, string>;
    const id = all[email.toLowerCase()] ?? "";
    return roles.some((r) => r.id === id) ? id : "";
  } catch {
    return "";
  }
}
