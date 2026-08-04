// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { useI18n } from "../i18n";
import { api } from "../api";

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
  const [url, setUrl] = useState(serverUrl ?? "");
  const [allowed, setAllowed] = useState<boolean | null>(null);
  /// Whether the address step was skipped, so "Back" goes somewhere sensible.
  const [asked, setAsked] = useState(false);
  const [email, setEmail] = useState("");
  const [org, setOrg] = useState("");
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const ready =
    email.includes("@") &&
    org.trim() !== "" &&
    password.length >= 12 &&
    password === again;

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
