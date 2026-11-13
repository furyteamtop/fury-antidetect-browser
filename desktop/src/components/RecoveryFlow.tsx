// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { api, type RecoveryStep } from "../api";
import { useI18n } from "../i18n";
import { Brand } from "./Brand";
import { CodeReveal } from "./RecoveryKey";

type Stage = "key" | "code" | "wait" | "password" | "done";

/** "Forgot password?" (docs/25).
 *
 *  The address and the recovery key; then whatever second step the server
 *  chose -- the authenticator code, an emailed code, or a 24-hour wait the
 *  team's owner and admins are told about and can cancel; then a new password.
 *  The key is spent by the recovery, so the last screen shows its successor,
 *  once, before signing in. */
export function RecoveryFlow({
  initialEmail,
  version,
  onDone,
  onBack,
}: {
  initialEmail: string;
  version?: string;
  onDone: () => void;
  onBack: () => void;
}) {
  const { t, say, resolved } = useI18n();
  const [stage, setStage] = useState<Stage>("key");
  const [email, setEmail] = useState(initialEmail);
  const [key, setKey] = useState("");
  const [step, setStep] = useState<RecoveryStep | null>(null);
  const [code, setCode] = useState("");
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [newKey, setNewKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const advance = (s: RecoveryStep) => {
    setStep(s);
    setCode("");
    setStage(s.ready ? "password" : s.factor === "wait" ? "wait" : "code");
  };

  const run = async (work: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await work();
    } catch (e) {
      const err = e as { code?: string } | null;
      setError(say(e));
      // A cancelled or lapsed attempt cannot be continued: back to the key.
      if (err?.code === "err.recoveryClosed") setStage("key");
    } finally {
      setBusy(false);
    }
  };

  // The wait is polled once a minute while this screen is open.
  useEffect(() => {
    if (stage !== "wait") return;
    const timer = setInterval(() => {
      void api.recoveryPoll().then(advance, (e) => {
        setError(say(e));
        setStage("key");
      });
    }, 60_000);
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stage]);

  const leave = async () => {
    await api.recoveryAbandon().catch(() => {});
    onBack();
  };

  const when = step?.ready_at ? new Date(step.ready_at).toLocaleString(resolved) : "";

  return (
    <form className="login" onSubmit={(e) => e.preventDefault()}>
      <Brand version={version} />
      <h3 className="center" style={{ margin: 0 }}>{t("rf.title")}</h3>

      {stage === "key" && (
        <>
          <p className="muted small center">{t("rf.keyWhy")}</p>
          <input type="email" placeholder={t("auth.email")} value={email} onChange={(e) => setEmail(e.target.value)} />
          <textarea
            className="mono"
            rows={3}
            spellCheck={false}
            placeholder="xxxx-xxxx-xxxx-…"
            value={key}
            autoFocus
            onChange={(e) => setKey(e.target.value)}
          />
          <button
            type="submit"
            disabled={busy || !email || key.replace(/[^0-9a-z]/gi, "").length < 52}
            onClick={() => void run(async () => advance(await api.recoveryStart(email, key, resolved)))}
          >
            {busy ? t("ck.working") : t("rf.continue")}
          </button>
          <p className="hint small">{t("rf.noKey")}</p>
        </>
      )}

      {stage === "code" && step && (
        <>
          <p className="muted small center">{step.factor === "totp" ? t("rf.totpWhy") : t("rf.emailWhy", { email })}</p>
          <input
            inputMode="numeric"
            autoComplete="one-time-code"
            placeholder={t("auth.code")}
            value={code}
            autoFocus
            maxLength={6}
            onChange={(e) => setCode(e.target.value.replace(/\D/g, ""))}
          />
          <button type="submit" disabled={busy || code.length !== 6} onClick={() => void run(async () => advance(await api.recoveryVerify(code)))}>
            {busy ? t("ck.working") : t("auth.confirmCode")}
          </button>
        </>
      )}

      {stage === "wait" && (
        <>
          <p className="warn small center">{t("rf.waitWhy", { when })}</p>
          <p className="hint small">{t("rf.waitNote")}</p>
          <button type="button" disabled={busy} onClick={() => void run(async () => advance(await api.recoveryPoll()))}>
            {busy ? t("auth.rechecking") : t("auth.recheck")}
          </button>
        </>
      )}

      {stage === "password" && (
        <>
          <p className="muted small center">{t("rf.passwordWhy")}</p>
          <input type="password" placeholder={t("rf.newPassword")} value={password} autoFocus onChange={(e) => setPassword(e.target.value)} />
          <input type="password" placeholder={t("rf.repeat")} value={again} onChange={(e) => setAgain(e.target.value)} />
          {password.length > 0 && password.length < 8 && <p className="hint small">{t("rf.short")}</p>}
          {again.length > 0 && again !== password && <p className="hint small">{t("rf.mismatch")}</p>}
          <button
            type="submit"
            disabled={busy || password.length < 8 || password !== again}
            onClick={() =>
              void run(async () => {
                const r = await api.recoveryFinish(password);
                setNewKey(r.code);
                setStage("done");
              })
            }
          >
            {busy ? t("ck.working") : t("rf.setPassword")}
          </button>
        </>
      )}

      {stage === "done" && newKey && (
        <>
          <p className="center">{t("rf.doneWhy")}</p>
          <CodeReveal compact code={newKey} email={email} onDone={onDone} />
        </>
      )}

      {error && <p className="error">{error}</p>}
      {stage !== "done" && (
        <button type="button" className="quiet" disabled={busy} onClick={() => void leave()}>
          {t("ui.cancel")}
        </button>
      )}
    </form>
  );
}
