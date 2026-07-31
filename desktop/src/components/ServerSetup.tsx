import { useState } from "react";
import { api, type Shell } from "../api";

/** First run. Fury is self-hosted, so there is no address to default to — the
 *  app cannot do anything until someone says where their server is. The address
 *  is checked before it is saved, because a typo'd host otherwise surfaces one
 *  screen later as a failed sign-in, which reads as "wrong password". */
export function ServerSetup({ onDone }: { onDone: (shell: Shell) => void }) {
  const [url, setUrl] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

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
      <p className="muted center">Where is your Fury server?</p>
      <input
        type="text"
        placeholder="fury.example.com"
        value={url}
        autoFocus
        spellCheck={false}
        onChange={(e) => setUrl(e.target.value)}
      />
      <p className="muted small center">
        https:// is assumed unless you say otherwise.
      </p>
      {error && <p className="error">{error}</p>}
      <button type="submit" disabled={busy || !url.trim()}>
        {busy ? "Checking…" : "Connect"}
      </button>
    </form>
  );
}
