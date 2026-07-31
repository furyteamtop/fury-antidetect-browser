import { useCallback, useEffect, useState } from "react";
import { api, type Perm, type Project } from "../api";
import { useI18n } from "../i18n";

type Members = Awaited<ReturnType<typeof api.orgMembers>>;
type Grants = Awaited<ReturnType<typeof api.grants>>;

const ROLES = ["admin", "manager", "member"] as const;

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
  const [team, setTeam] = useState<Members | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<string>("member");
  const [code, setCode] = useState<{ code: string; email: string } | null>(null);
  const [project, setProject] = useState<string>(projects[0]?.id ?? "");
  const [grants, setGrants] = useState<Grants | null>(null);

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
      {error && <p className="error">{error}</p>}

      <h2 className="sectionTitle">{t("team.people")}</h2>
      <table className="grid">
        <thead>
          <tr>
            <th>{t("team.member")}</th>
            <th>{t("team.role")}</th>
            <th>{t("team.key")}</th>
            {project && <th>{t("team.access", { project: projects.find((p) => p.id === project)?.name ?? "" })}</th>}
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
                  {!m.has_key && (
                    <button
                      disabled={busy}
                      onClick={() => run(() => api.handOverKey(m.user_id, m.public_key))}
                    >
                      {t("team.giveKey")}
                    </button>
                  )}
                  {project && !implicit.has(m.user_id) && !granted.has(m.user_id) && (
                    <button
                      className="ghost"
                      disabled={busy}
                      onClick={() =>
                        run(() =>
                          api.grantAccess(project, m.user_id, [
                            "view",
                            "launch",
                            "edit_profile",
                          ] as Perm[]),
                        )
                      }
                    >
                      {t("team.grant")}
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

      {projects.length > 0 && (
        <p className="hint" style={{ marginTop: "var(--s-3)" }}>
          {t("team.accessFor")}{" "}
          <select
            style={{ width: "auto" }}
            value={project}
            onChange={(e) => setProject(e.target.value)}
          >
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </p>
      )}

      <h2 className="sectionTitle" style={{ marginTop: "var(--s-6)" }}>
        {t("team.invite")}
      </h2>
      <p className="hint">{t("team.inviteHint")}</p>
      <div className="row" style={{ maxWidth: 560 }}>
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
