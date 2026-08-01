import { useState } from "react";
import { useI18n } from "../i18n";
import { api, type Shell } from "../api";

/** First run. Fury is self-hosted, so there is no address to default to — the
 *  app cannot do anything until someone says where their server is. The address
 *  is checked before it is saved, because a typo'd host otherwise surfaces one
 *  screen later as a failed sign-in, which reads as "wrong password". */
export function ServerSetup({
  onDone,
  onSignup,
}: {
  onDone: (shell: Shell) => void;
  /** This screen assumes an account already exists on the address typed in.
   *  Somebody who has none needs the other door, and it has to be on this
   *  screen — it is the first one a new install shows. */
  onSignup: () => void;
}) {
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const { t } = useI18n();

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
      <button type="button" className="alt" onClick={onSignup}>
        {t("signup.start")}
      </button>
    </form>
  );
}
