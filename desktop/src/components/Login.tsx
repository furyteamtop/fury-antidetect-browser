import { useState } from "react";
import { useI18n } from "../i18n";
import { api } from "../api";

export function Login({
  onSuccess,
  onEnrol,
  onSignup,
  unlockFor,
}: {
  onSuccess: () => void;
  /** There is no registration form, so the only way to a first account is an
   *  invitation — and this screen is where someone holding one arrives. */
  onEnrol: () => void;
  /** No account anywhere yet. Only useful against a server that takes open
   *  sign-ups, which most will not — the screen behind this asks before it
   *  offers a form. */
  onSignup: () => void;
  /** Set when the session is alive but the organisation key is not: the app was
   *  restarted, and the key lives in memory for exactly as long as the process
   *  that holds it. This is the same form doing a different job — the password
   *  is what the key is derived from, so there is nothing else it could be. */
  unlockFor?: string | null;
}) {
  const [email, setEmail] = useState(unlockFor ?? "");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const { t } = useI18n();

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.login(email, password);
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
          <button type="button" className="linky" onClick={onEnrol}>
            {t("enrol.have")}
          </button>
          <button type="button" className="linky" onClick={onSignup}>
            {t("signup.start")}
          </button>
        </>
      )}
    </form>
  );
}
