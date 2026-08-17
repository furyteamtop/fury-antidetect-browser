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

    const ok: string[] = [];
    for (const p of profiles) {
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
    if (ok.length === profiles.length) {
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

  return (
    <div className="modalBack" onClick={onClose}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>
          {profiles.length === 1
            ? t("share.title", { name: profiles[0].name })
            : t("share.titleMany", { n: String(profiles.length) })}
        </h2>

        <form onSubmit={share}>
          <label>{t("share.who")}</label>
          <input
            type="email"
            placeholder="name@example.com"
            value={email}
            autoFocus
            spellCheck={false}
            onChange={(e) => setEmail(e.target.value)}
          />
          <p className="hint">{t("share.whoHint")}</p>

          <label>{t("share.mayThey")}</label>
          <label className="check">
            <input type="checkbox" checked={canLaunch} onChange={(e) => setCanLaunch(e.target.checked)} />
            {t("share.mayOpen")}
          </label>
          <label className="check">
            <input type="checkbox" checked={canEdit} onChange={(e) => setCanEdit(e.target.checked)} />
            {t("share.mayEdit")}
          </label>
          <label className="check">
            <input type="checkbox" checked={canReveal} onChange={(e) => setCanReveal(e.target.checked)} />
            {t("share.mayReveal")}
          </label>
          <p className="hint">{t("share.revealHint")}</p>

          {/* Said before the button, not after. Revoking closes the door and
              does not reach what somebody already opened, and anybody handing
              over live accounts should know that while deciding rather than
              afterwards. */}
          <p className="warn small">{t("share.cannotUndo")}</p>

          {done.length > 0 && (
            <p className="hint">{t("share.doneN", { n: String(done.length) })}</p>
          )}
          {error && <p className="error">{error}</p>}

          <div className="row end">
            <button type="button" className="ghost" onClick={onClose} disabled={busy}>
              {t("ui.close")}
            </button>
            <button type="submit" className="primary" disabled={busy || !email.includes("@")}>
              {busy ? t("share.sharing") : t("share.give")}
            </button>
          </div>
        </form>

        {one && holders.length > 0 && (
          <div className="settingsGroup">
            <h3>{t("share.holders")}</h3>
            <ul className="plain">
              {holders.map((h) => (
                <li key={h.user_id} className="row spread">
                  <span>{h.email}</span>
                  <button className="ghost" disabled={busy} onClick={() => void revoke(h.user_id)}>
                    {t("share.takeBack")}
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </div>
  );
}
