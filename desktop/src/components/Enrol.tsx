// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useState } from "react";
import { useI18n } from "../i18n";
import { api, type Invitation, type Me } from "../api";
import { DEFAULT_SERVER } from "../defaults";
import { PasswordPair } from "./PasswordPair";

/** Spelled out rather than composed as `role.${x}`: the roles come from the
 *  server as free text, and a key built from one would neither typecheck nor
 *  fail visibly — it would render the key. */
function roleLabel(role: string, t: (k: "role.owner" | "role.admin" | "role.manager" | "role.member") => string) {
  switch (role) {
    case "owner":
      return t("role.owner");
    case "admin":
      return t("role.admin");
    case "manager":
      return t("role.manager");
    default:
      return t("role.member");
  }
}

/** Redeeming an invitation — how every account on a Fury server begins.
 *
 *  Two steps, deliberately. The code is checked first and what it is for is
 *  shown ("you are joining My team as owner") before anyone chooses a password:
 *  a password field on top of an unverified code means typing a password for a
 *  server you may not have an account on, and then being told so.
 *
 *  The password never leaves this screen in a form the server can use. The keys
 *  are generated in Rust — see src-tauri/crypto.rs — and only wrapped results
 *  are posted. Which is also why the warning below is not boilerplate: nobody
 *  can reset this password, because nobody else can unwrap what it wraps. */
export function Enrol({
  onDone,
  onCancel,
  /// The server the shell is already pointed at, when it is pointed at one.
  serverUrl,
}: {
  onDone: (me: Me) => void;
  onCancel: () => void;
  serverUrl?: string | null;
}) {
  // Empty was wrong for everyone who reaches this screen. Somebody redeeming an
  // invitation has a code and, very often, no idea what to write above it: the
  // colleague who sent the code sent the code, and the address it belongs to is
  // the thing they forgot to mention. Sign-up has offered the open server here
  // since it existed; enrolment asking for an address with nothing in the box
  // was the same first screen with the answer taken out.
  //
  // Prefilled and not fixed: a team on its own machine clears it and types
  // theirs, and the note under the field says so, because a code is only valid
  // on the server that issued it.
  const [url, setUrl] = useState(serverUrl ?? DEFAULT_SERVER);
  const [code, setCode] = useState("");
  const [invitation, setInvitation] = useState<Invitation | null>(null);
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const { t } = useI18n();

  const check = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      setInvitation(await api.invitation(url, code));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const enrol = async (e: React.FormEvent) => {
    e.preventDefault();
    if (password !== again) {
      setError(t("enrol.mismatch"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      onDone(await api.enrol(url, code, password, invitation!.creates_org_key));
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  if (!invitation) {
    return (
      <form className="login" onSubmit={check}>
        <h1>Fury</h1>
        <p className="muted center">{t("enrol.intro")}</p>
        <input
          type="text"
          placeholder={t("srv.placeholder")}
          value={url}
          autoFocus
          spellCheck={false}
          onChange={(e) => setUrl(e.target.value)}
        />
        {/* Only when the address is the offered one. Somebody whose shell is
            already pointed at their team's server is not being offered
            anything and does not need to be told about a server they are not
            using. */}
        {!serverUrl && <p className="hint">{t("enrol.serverPrefilled")}</p>}
        <input
          type="text"
          className="mono"
          placeholder="xxxx-xxxx-xxxx-xxxx-xxxx-xxxx-xxxx-xxxx"
          value={code}
          spellCheck={false}
          autoCapitalize="off"
          onChange={(e) => setCode(e.target.value)}
        />
        {error && <p className="error">{error}</p>}
        <button type="submit" disabled={busy || !url.trim() || !code.trim()}>
          {busy ? t("enrol.checking") : t("enrol.continue")}
        </button>
        <button type="button" className="alt" onClick={onCancel}>
          {t("enrol.back")}
        </button>
      </form>
    );
  }

  return (
    <form className="login" onSubmit={enrol}>
      <h1>Fury</h1>
      <p className="center">
        {t("enrol.joining", {
          email: invitation.email,
          org: invitation.organization,
          role: roleLabel(invitation.role, t),
        })}
      </p>
      <PasswordPair
        password={password}
        again={again}
        onPassword={setPassword}
        onAgain={setAgain}
        autoFocus
      />
      {/* Stated before the button, not after the mistake. */}
      <p className="warn small center">{t("enrol.noRecovery")}</p>
      {error && <p className="error">{error}</p>}
      <button type="submit" disabled={busy || password.length < 12 || !again}>
        {busy ? t("enrol.creating") : t("enrol.create")}
      </button>
      <button type="button" className="linky" onClick={() => setInvitation(null)}>
        {t("enrol.back")}
      </button>
    </form>
  );
}
