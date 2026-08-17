// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { useI18n } from "../i18n";
import { api } from "../api";
import { DEFAULT_SERVER } from "../defaults";

/** Making an account from nothing.
 *
 *  The gap this fills: until now the only way onto a server was a code someone
 *  ran a command to issue, which is right for a team a person already runs and
 *  wrong for a product a person downloads. Somebody who installed Fury, wanted
 *  their profiles on a server, and had nobody to ask had nowhere to go.
 *
 *  The address is asked first and asked alone, because whether this screen can
 *  even be used depends on the answer: a server decides for itself whether it
 *  takes strangers, and most will not. Offering the form before knowing would
 *  mean filling it in to be told no.
 *
 *  Worth saying plainly on the screen, and it is: the account created here owns
 *  a NEW organisation. It joins nothing. Somebody expecting to land in their
 *  colleague's team needs an invitation, and this is where they find that out —
 *  before they have made a second account nobody wanted. */
export function Signup({
  onDone,
  onCancel,
  /// The server the shell is already pointed at, when it is pointed at one.
  ///
  /// Without this the flow asked for the address twice: once to connect, and
  /// again on the first screen of sign-up, having just been told. Two identical
  /// questions in a row on the very first screen a new person sees reads as the
  /// application having forgotten the answer — which it had.
  serverUrl,
}: {
  onDone: () => void;
  onCancel: () => void;
  serverUrl?: string | null;
}) {
  const { t, say } = useI18n();
  // serverUrl wins when the shell is already pointed at one -- somebody
  // signing up on their own server should not have to retype it. Otherwise
  // the offered server, for the person who has none.
  const [url, setUrl] = useState(serverUrl ?? DEFAULT_SERVER);
  const [allowed, setAllowed] = useState<boolean | null>(null);
  /// Whether the address step was skipped, so "Back" goes somewhere sensible.
  const [asked, setAsked] = useState(false);
  const [email, setEmail] = useState("");
  const [org, setOrg] = useState("");
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const MIN_PASSWORD = 12;

  /** Why the button is disabled, in a sentence, or null when it is not.
   *
   *  It used to be `disabled={!ready}` and nothing else, and the screen said
   *  nowhere that the password has a minimum length. A filled-in form with a
   *  nine-character password produced a dead button and no explanation --
   *  every field looks answered, so the natural reading is that the
   *  application is broken. Reported as exactly that.
   *
   *  Order matters: it reports the FIRST thing that is missing, top to bottom,
   *  so the message follows the eye down the form rather than jumping. */
  const blocker =
    !email.includes("@") ? t("signup.needEmail")
    : org.trim() === "" ? t("signup.needOrg")
    : password.length < MIN_PASSWORD
      ? t("signup.needPassword", { n: String(MIN_PASSWORD), have: String(password.length) })
    : password !== again ? t("signup.needMatch")
    : null;
  const ready = blocker === null;

  const ask = async (e?: React.FormEvent) => {
    e?.preventDefault();
    setBusy(true);
    setError(null);
    setAsked(true);
    try {
      const open = await api.serverAllowsSignup(url);
      setAllowed(open);
      if (!open) setError(t("signup.closed"));
    } catch (e) {
      setError(say(e));
      // Back to the address step. A server that cannot be reached is usually a
      // typed address, and the field has to be in front of the person again.
      setAllowed(null);
    } finally {
      setBusy(false);
    }
  };

  // Ask the server about itself straight away when the shell already knows
  // where it is. The address step still exists — for somebody signing up
  // against a server they have not connected to, and for when this check fails.
  useEffect(() => {
    if (serverUrl && !asked) void ask();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [serverUrl]);

  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.signup(url, email.trim(), password, org.trim());
      onDone();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="login" onSubmit={allowed ? create : ask}>
      <h1>Fury</h1>

      {/* The address step is skipped when the shell already knows the server,
          and this is what stops it flashing on the way past: the first render
          happens before the check has answered, and showing an address box for
          a moment — prefilled, then gone — reads as a glitch. */}
      {serverUrl && allowed === null && !error ? (
        <p className="muted center">{t("srv.checking")}</p>
      ) : allowed !== true ? (
        <>
          <p className="muted center">{t("signup.whichServer")}</p>
          <input
            type="text"
            placeholder={t("srv.placeholder")}
            value={url}
            autoFocus
            spellCheck={false}
            onChange={(e) => {
              setUrl(e.target.value);
              setAllowed(null);
              setError(null);
            }}
          />
          <button className="primary" type="submit" disabled={busy || url.trim() === ""}>
            {busy ? t("srv.checking") : t("signup.check")}
          </button>
        </>
      ) : (
        <>
          <p className="muted center">{t("signup.newTeam", { url })}</p>
          <input
            type="email"
            placeholder={t("auth.email")}
            value={email}
            autoFocus
            spellCheck={false}
            onChange={(e) => setEmail(e.target.value)}
          />
          <input
            type="text"
            placeholder={t("signup.orgName")}
            value={org}
            onChange={(e) => setOrg(e.target.value)}
          />
          <input
            type="password"
            placeholder={t("enrol.password")}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          <input
            type="password"
            placeholder={t("enrol.passwordAgain")}
            value={again}
            onChange={(e) => setAgain(e.target.value)}
          />
          {/* The same warning enrolment gives, for the same reason: this
              password is the only thing that opens the organisation key, and
              nobody holds a copy. */}
          <p className="warn small center">{t("enrol.noRecovery")}</p>
          {/* No confirmation mail is coming, and saying so here is the whole
              point: the server has no mail library and sends nothing at all.
              The account is live the moment this button works.

              Somebody who signs up and then waits for a letter concludes the
              registration failed, and the second attempt fails properly -- the
              address is taken, by them. Reported exactly that way, with a
              ten-minute mailbox open beside the application.

              It also carries the consequence, which matters more than the
              reassurance: an address nobody verifies and a password nobody can
              reset means a typo here is a locked door with no key. */}
          <p className="hint center">{t("signup.noEmailSent")}</p>
          {blocker && <p className="hint center">{blocker}</p>}
          <button className="primary" type="submit" disabled={busy || !ready}>
            {busy ? t("signup.creating") : t("signup.create")}
          </button>
        </>
      )}

      {error && <p className="error center">{error}</p>}
      <button type="button" className="alt" onClick={onCancel}>
        {t("enrol.back")}
      </button>
    </form>
  );
}
