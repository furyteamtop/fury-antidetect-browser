import { useState } from "react";
import { useI18n } from "../i18n";
import { api } from "../api";

export function Login({ onSuccess }: { onSuccess: () => void }) {
  const [email, setEmail] = useState("");
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
        {busy ? t("auth.signingIn") : t("auth.signIn")}
      </button>
    </form>
  );
}
