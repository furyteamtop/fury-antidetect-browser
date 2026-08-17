// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { api, type Profile } from "../api";
import { useI18n } from "../i18n";

/** Give profiles to somebody, and take them back.
 *
 *  Takes a LIST rather than one profile, because "share these five" was the
 *  first thing asked for after the feature was described and doing it one
 *  dialog at a time is not a feature, it is a chore. The server grants one
 *  profile at a time -- each carries its own key and there is no key that opens
 *  several -- so this loops, and reports per profile rather than pretending the
 *  batch is atomic. A run that fails on the fourth of five leaves four shared,
 *  and saying so is more useful than a single red line.
 *
 *  Only the current holders of the FIRST profile are listed. Showing a merged
 *  list for five would invite revoking from a person who holds three of them
 *  and thinking all three were withdrawn. */
export function ShareDialog({
  profiles,
  onClose,
}: {
  profiles: Profile[];
  onClose: () => void;
}) {
  const { t } = useI18n();
  const [email, setEmail] = useState("");
  const [canLaunch, setCanLaunch] = useState(true);
  const [canEdit, setCanEdit] = useState(false);
  const [canReveal, setCanReveal] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string[]>([]);
  const [holders, setHolders] = useState<
    { user_id: string; email: string; permissions: number }[]
  >([]);

  const one = profiles.length === 1 ? profiles[0] : null;

  const loadHolders = async () => {
    if (!one) return;
    try {
      setHolders(await api.profileShares(one.id));
    } catch {
      // A profile nobody has shared yet answers with an empty list; a failure
      // here is not worth a red box above a form that still works.
      setHolders([]);
    }
  };

  useEffect(() => {
    void loadHolders();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [one?.id]);

  /** Make sure everything being given away is somewhere the other person can
   *  reach, and do it without asking.
   *
   *  Sharing needs the profile on a server. That is true and it is OUR problem:
   *  the first version made it the operator's, by disabling the button for a
   *  local profile and telling them to send it across first -- into a project
   *  they also had to choose, from a list that could be empty, behind an error
   *  message telling them to pick one. Four steps to express one intention.
   *
   *  So the dialog does it. A local profile is copied to the server on the way,
   *  into a project named after the folder it was already in, or a default one
   *  made once and reused. The person asked to share a profile; they get a
   *  shared profile.
   *
   *  The copy is still a copy: the original stays on this machine, which is
   *  what upload_profile has always done and what the message afterwards says. */
  const ensureOnServer = async (rows: Profile[]): Promise<Profile[]> => {
    const locals = rows.filter((p) => p.origin === "local");
    if (locals.length === 0) return rows;

    const projects = await api.projects();
    const team = projects.filter((p) => p.origin === "team");
    const moved = new Map<string, string>();

    for (const p of locals) {
      // Its own folder's name if it had one, so a structure somebody built here
      // survives the trip instead of everything landing in one bucket.
      const wanted = p.project_name?.trim() || t("share.autoProject");
      let target = team.find((x) => x.name === wanted)?.id;
      if (!target) {
        const made = await api.createProjectIn(wanted, "team");
        target = made.id;
        team.push({ id: made.id, name: wanted, profile_count: 0, origin: "team" });
      }
      const r = await api.uploadProfile(p.id, target);
      moved.set(p.id, r.id);
    }

    return rows.map((p) => (moved.has(p.id) ? { ...p, id: moved.get(p.id)!, origin: "team" } : p));
  };

  const share = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    setDone([]);

    // view is implied: a share that does not let somebody see the profile is
    // not a share. The server refuses it too, and agreeing here means the
    // person never meets that error.
    const permissions = ["view"];
    if (canLaunch) permissions.push("launch");
    if (canEdit) permissions.push("edit_profile");
    if (canReveal) permissions.push("reveal_secrets");

    let targets: Profile[];
    try {
      targets = await ensureOnServer(profiles);
    } catch (err) {
      setError((err as Error).message);
      setBusy(false);
      return;
    }

    const ok: string[] = [];
    for (const p of targets) {
      try {
        await api.shareProfile(p.id, email.trim(), permissions);
        ok.push(p.name);
        setDone([...ok]);
      } catch (err) {
        setError(
          profiles.length === 1
            ? (err as Error).message
            : t("share.stopped", { name: p.name, why: (err as Error).message }),
        );
        break;
      }
    }
    setBusy(false);
    if (ok.length === targets.length) {
      setEmail("");
      await loadHolders();
    }
  };

  const revoke = async (userId: string) => {
    if (!one) return;
    setBusy(true);
    try {
      await api.revokeShare(one.id, userId);
      await loadHolders();
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  /** One checkbox with its label beside it, and a hint under it.
   *
   *  Written out rather than reached for from a stylesheet class: the first
   *  version used `className="check"`, which exists nowhere in styles.css, so
   *  the browser drew each box on its own line above its own label at full
   *  width. It looked broken because it was. */
  const check = (on: boolean, set: (v: boolean) => void, label: string, hint?: string) => (
    <div style={{ marginBottom: "var(--s-2)" }}>
      <label className="row" style={{ gap: 8 }}>
        <input
          type="checkbox"
          style={{ width: 14, height: 14, accentColor: "var(--accent)" }}
          checked={on}
          onChange={(e) => set(e.target.checked)}
        />
        <span>{label}</span>
      </label>
      {hint && <p className="hint">{hint}</p>}
    </div>
  );

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true">
        <div className="modalHead">
          <h2>
            {one
              ? t("share.title", { name: one.name })
              : t("share.titleMany", { n: String(profiles.length) })}
          </h2>
        </div>

        <div className="modalBody">
          <form className="form" id="shareForm" onSubmit={share}>
            <div className="field">
              <label htmlFor="share-email">{t("share.who")}</label>
              <div>
                <input
                  id="share-email"
                  type="email"
                  placeholder="name@example.com"
                  value={email}
                  autoFocus
                  spellCheck={false}
                  onChange={(e) => setEmail(e.target.value)}
                />
                <p className="hint">{t("share.whoHint")}</p>
                {profiles.some((p) => p.origin === "local") && (
                  <p className="hint">{t("share.willCopy")}</p>
                )}
              </div>
            </div>

            <div className="field">
              <label>{t("share.mayThey")}</label>
              <div>
                {check(canLaunch, setCanLaunch, t("share.mayOpen"))}
                {check(canEdit, setCanEdit, t("share.mayEdit"))}
                {check(canReveal, setCanReveal, t("share.mayReveal"), t("share.revealHint"))}
              </div>
            </div>

            {/* Above the button and not below it: revoking cannot reach cookies
                somebody has already opened, and that belongs in front of the
                person while they are still deciding. */}
            <p className="warn small">{t("share.cannotUndo")}</p>

            {one && holders.length > 0 && (
              <div className="field">
                <label>{t("share.holders")}</label>
                <div>
                  {holders.map((h) => (
                    <div key={h.user_id} className="row" style={{ gap: "var(--s-2)" }}>
                      <span className="ellipsis">{h.email}</span>
                      <div className="spacer" />
                      <button
                        type="button"
                        className="ghost"
                        disabled={busy}
                        onClick={() => void revoke(h.user_id)}
                      >
                        {t("share.takeBack")}
                      </button>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </form>
        </div>

        <div className="modalFoot">
          {error && <span className="error">{error}</span>}
          {!error && done.length > 0 && (
            <span className="muted small">{t("share.doneN", { n: String(done.length) })}</span>
          )}
          <div className="spacer" />
          <button type="button" className="ghost" onClick={onClose} disabled={busy}>
            {t("ui.close")}
          </button>
          <button
            type="submit"
            form="shareForm"
            className="primary"
            disabled={busy || !email.includes("@")}
          >
            {busy ? t("share.sharing") : t("share.give")}
          </button>
        </div>
      </div>
    </div>
  );
}
