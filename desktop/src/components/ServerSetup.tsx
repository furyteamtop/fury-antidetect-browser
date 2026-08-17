// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useState } from "react";
import { useI18n } from "../i18n";
import { api, type Shell } from "../api";
import { DEFAULT_SERVER } from "../defaults";

/** First run. Fury is self-hosted, so there is no address to default to — the
 *  app cannot do anything until someone says where their server is. The address
 *  is checked before it is saved, because a typo'd host otherwise surfaces one
 *  screen later as a failed sign-in, which reads as "wrong password". */
export function ServerSetup({
  onDone,
  onSignup,
  onEnrol,
  onLocal,
}: {
  onDone: (shell: Shell) => void;
  /** Somebody arriving with a code from a colleague, which is the commonest way
   *  a second person ever reaches this screen — and the one door it did not
   *  have.
   *
   *  The steps on the team screen told them to find it under Settings → Team
   *  server, and that is true of an install already running: it is not true of
   *  a fresh one, where Settings is behind a window that has nothing in it yet.
   *  So an invited colleague read an instruction naming a place their copy did
   *  not have, on the only screen it would show them. */
  onEnrol: () => void;
  /** This screen assumes an account already exists on the address typed in.
   *  Somebody who has none needs the other door, and it has to be on this
   *  screen — it is the first one a new install shows. */
  onSignup: () => void;
  /** Called once the shell is local again.
   *
   *  This screen had two buttons and both of them demanded a server address:
   *  connect to one, or create an account on one. A person who has neither --
   *  which is most people on a first run, and the configuration the README
   *  calls the default -- had no door at all. Working alone was already
   *  supported and already implemented; it was simply not offered until after
   *  you had answered the question you could not answer.
   *
   *  It reads as "so where do I get a server?", and the natural next thought is
   *  that the application ought to supply one. It ought not: a hosted server
   *  would hold other people's profiles, proxy credentials and cookies, which
   *  is the most sensitive data this project touches and the opposite of the
   *  promise on the About screen. The missing thing was the button, not the
   *  server. */
  onLocal: () => void;
}) {
  // Prefilled, not hidden: see defaults.ts. Somebody with their own server
  // clears one field; somebody with none is no longer stuck.
  const [url, setUrl] = useState(DEFAULT_SERVER);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const { t } = useI18n();

  const goLocal = async () => {
    setBusy(true);
    setError(null);
    try {
      // Not a no-op even here. A fresh install is already local, but this
      // screen is also reached after a server was set and then became
      // unreachable, and in that state the shell still believes it is in team
      // mode. Forgetting it is what makes the button honest in both.
      onDone(await api.disconnectServer());
      onLocal();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onDone(await api.setServer(url));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="login" onSubmit={submit}>
      <h1>Fury</h1>
      <p className="muted center">{t("srv.where")}</p>
      <input
        type="text"
        placeholder={t("srv.placeholder")}
        value={url}
        autoFocus
        spellCheck={false}
        onChange={(e) => setUrl(e.target.value)}
      />
      <p className="muted small center">
        {t("srv.httpsAssumed")}
      </p>
      {error && <p className="error">{error}</p>}
      <button type="submit" disabled={busy || !url.trim()}>
        {busy ? t("srv.checking") : t("srv.connect")}
      </button>
      {/* Above sign-up, because the two are not equally likely here and picking
          the wrong one is expensive: an invited colleague who presses "create
          an account" owns a new organisation of one and wonders where the
          team's profiles are. */}
      <button type="button" className="alt" onClick={onEnrol} disabled={busy}>
        {t("enrol.have")}
      </button>
      <button type="button" className="alt" onClick={onSignup} disabled={busy}>
        {t("signup.start")}
      </button>
      <button type="button" className="quiet" onClick={goLocal} disabled={busy}>
        {t("auth.workLocally")}
      </button>
      <p className="hint">{t("auth.workLocallyNote")}</p>
    </form>
  );
}
