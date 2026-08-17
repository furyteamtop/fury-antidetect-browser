// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { useI18n } from "../i18n";
import { api, type Shell } from "../api";

export function Login({
  onSuccess,
  onEnrol,
  onSignup,
  onLocal,
  unlockFor,
  awaitingKey,
}: {
  onSuccess: () => void;
  /** There is no registration form, so the only way to a first account is an
   *  invitation — and this screen is where someone holding one arrives. */
  onEnrol: () => void;
  /** No account anywhere yet. Only useful against a server that takes open
   *  sign-ups, which most will not — the screen behind this asks before it
   *  offers a form. */
  onSignup: () => void;
  /// Called after the server has been forgotten and the shell is local again.
  onLocal: () => void;
  /** Set when the session is alive but the organisation key is not: the app was
   *  restarted, and the key lives in memory for exactly as long as the process
   *  that holds it. This is the same form doing a different job — the password
   *  is what the key is derived from, so there is nothing else it could be. */
  unlockFor?: string | null;
  /** …except when there is no key on the server either, and then a password is
   *  not a slower way in but no way in. See below. */
  awaitingKey?: boolean;
}) {
  const [email, setEmail] = useState(unlockFor ?? "");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /// Whether the last look found the key still missing, so the button can say
  /// that it looked.
  const [waited, setWaited] = useState(false);
  const { t } = useI18n();

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.login(email, password);
      // Unlocking is not the same as signing in, and it is only the second one
      // that this call finishing means. A member whose key has not been handed
      // over signs in perfectly and unlocks nothing — so before reporting
      // success, ask what the shell now holds. Without this the screen simply
      // reappeared, blank, after a password that was in fact correct, which
      // reads as the button not working.
      if (unlockFor) {
        const after = await api.shell();
        if (!after.org_key_ready) {
          setError(t("auth.stillNoKey"));
          return;
        }
      }
      onSuccess();
    } catch {
      // The server answers identically for an unknown address and a wrong
      // password, so this message must not distinguish them either — saying
      // "no such user" here would undo that.
      setError(t("auth.wrong"));
    } finally {
      setBusy(false);
    }
  };

  /** Out of a screen that had no way out.
   *
   *  Every other state here offers three doors and an escape hatch; the unlock
   *  state offered one button, and for the member waiting on a key it was a
   *  button that could not work. Signing out is the honest exit: it drops the
   *  session and lands on the sign-in screen, which does have the rest. */
  const signOut = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.logout();
    } finally {
      setBusy(false);
      onSuccess();
    }
  };

  /** Whether the wait is over.
   *
   *  Two ways it can be, and the second is the one worth naming: the key having
   *  appeared on the server does not put it in this process. It is sealed to a
   *  public key whose private half only this password opens, so what follows is
   *  the unlock form — which is the screen behind this one, and reached by
   *  `awaiting_key` going false. Waiting for `org_key_ready` alone would wait
   *  for ever. */
  const arrived = (s: Shell) => s.org_key_ready || !s.awaiting_key;

  /** Ask again, by hand.
   *
   *  The key arrives from somebody else's machine at a moment nobody here can
   *  predict. The poll below covers the ordinary case; this button is for the
   *  person who has just been told on the phone that it has been sent and does
   *  not want to wait ten more seconds — and it answers, rather than appearing
   *  to do nothing, which is what makes a button nobody presses twice. */
  const recheck = async () => {
    setBusy(true);
    setError(null);
    setWaited(false);
    try {
      if (arrived(await api.shell())) {
        onSuccess();
        return;
      }
      setWaited(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  /** And without being asked, while this screen is the one on show.
   *
   *  The rest of the application polls the shell every five seconds; that poll
   *  lives behind the sign-in and does not run here. So somebody waiting for a
   *  colleague to press a button would sit in front of a screen that had
   *  already stopped being true — the key handed over, and nothing on this
   *  machine noticing until they tried something. Ten seconds, and only in this
   *  state: everywhere else this component makes no requests at all. */
  useEffect(() => {
    if (!awaitingKey) return;
    const timer = setInterval(() => {
      void api.shell().then((s) => {
        if (arrived(s)) onSuccess();
      }, () => {});
    }, 10_000);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [awaitingKey]);

  const goLocal = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.disconnectServer();
      onLocal();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  /** Enrolled, and holding nothing — waiting on somebody else.
   *
   *  A separate screen rather than a sentence above the password box, because
   *  the password box is the mistake. What this person has to do is not on this
   *  machine at all: an owner opens Users and presses "Give the key", which
   *  seals it to the public key published at enrolment. Until then there is
   *  nothing here to type, and every field offered is a field that will be
   *  filled in and rejected.
   *
   *  So: what is happening, who has to act, and the two things that are
   *  genuinely theirs to do — look again, or leave. */
  if (awaitingKey) {
    return (
      <div className="login">
        <h1>Fury</h1>
        {unlockFor && <p className="center">{unlockFor}</p>}
        <p className="warn small center">{t("auth.awaitingKey")}</p>
        {error && <p className="error center">{error}</p>}
        {waited && <p className="hint">{t("auth.stillWaiting")}</p>}
        <button type="button" className="primary" disabled={busy} onClick={() => void recheck()}>
          {busy ? t("auth.rechecking") : t("auth.recheck")}
        </button>
        <button type="button" className="alt" disabled={busy} onClick={() => void signOut()}>
          {t("app.signOut")}
        </button>
        <button type="button" className="quiet" disabled={busy} onClick={goLocal}>
          {t("auth.workLocally")}
        </button>
        <p className="hint">{t("auth.workLocallyNote")}</p>
      </div>
    );
  }

  return (
    <form className="login" onSubmit={submit}>
      <h1>Fury</h1>
      {unlockFor && <p className="muted center">{t("auth.unlockWhy")}</p>}
      <input
        type="email"
        placeholder={t("auth.email")}
        value={email}
        autoFocus
        onChange={(e) => setEmail(e.target.value)}
      />
      <input
        type="password"
        placeholder={t("auth.password")}
        value={password}
        onChange={(e) => setPassword(e.target.value)}
      />
      {error && <p className="error">{error}</p>}
      <button type="submit" disabled={busy || !email || !password}>
        {busy ? t("auth.signingIn") : unlockFor ? t("auth.unlock") : t("auth.signIn")}
      </button>
      {!unlockFor && (
        <>
          {/* Two doors, and they are not the same one. An invitation joins a
              team that exists; a sign-up makes a new one. Someone who picks
              the wrong one ends up owning an organisation of one and wondering
              where their colleague's profiles went. */}
          <button type="button" className="alt" onClick={onEnrol}>
            {t("enrol.have")}
          </button>
          <button type="button" className="alt" onClick={onSignup}>
            {t("signup.start")}
          </button>
          {/* The way back out, and it was missing.

              api.disconnectServer() has existed all along — and lived in
              Settings, which is behind the sign-in. So an operator who
              connected to a server once and then wanted to work alone was
              looking at the escape hatch through the door it opens. This screen
              was a dead end: sign in, join a team, or start one.

              Solo is what the README calls the default and it genuinely is —
              a fresh install with no server has never asked for an account. The
              bug was that team mode was a one-way door. */}
          <button type="button" className="quiet" onClick={goLocal} disabled={busy}>
            {t("auth.workLocally")}
          </button>
          <p className="hint">{t("auth.workLocallyNote")}</p>
        </>
      )}
      {/* And out of the unlock state too, for the same argument one comment up.
          Unlocking has exactly one button, and somebody who cannot remember the
          password behind it — or who is waiting on a key against an older
          server that cannot say so — was left with a form and no way past it. */}
      {unlockFor && (
        <>
          <button type="button" className="alt" disabled={busy} onClick={() => void signOut()}>
            {t("app.signOut")}
          </button>
          <button type="button" className="quiet" onClick={goLocal} disabled={busy}>
            {t("auth.workLocally")}
          </button>
        </>
      )}
    </form>
  );
}
