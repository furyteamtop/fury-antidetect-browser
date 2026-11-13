// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useCallback, useEffect, useState } from "react";
import { api, type Me, type RecoveryAttempt, type RecoveryStatus } from "../api";
import { useI18n } from "../i18n";

/** The recovery key in Settings (docs/25): whether there is one, making a new
 *  one, and any attempt to use it that is under way right now.
 *
 *  Making one asks for the password again, and for the authenticator code
 *  when the account has one: a key that can take the account over is not
 *  something a person passing an unlocked laptop should be able to mint. */
export function RecoveryKey({ email, onChanged }: { email: string; onChanged?: () => void }) {
  const { t, say } = useI18n();
  const [status, setStatus] = useState<RecoveryStatus | null>(null);
  const [totp, setTotp] = useState(false);
  const [asking, setAsking] = useState(false);
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [shown, setShown] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      setStatus(await api.recoveryStatus());
      setTotp((await api.totpStatus()).enabled);
    } catch (e) {
      setError(say(e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    void load();
  }, [load]);

  const create = async () => {
    setBusy(true);
    setError(null);
    try {
      const printed = await api.recoveryCreate(password, totp ? code : null);
      setShown(printed);
      setAsking(false);
      setPassword("");
      setCode("");
      await load();
      onChanged?.();
    } catch (e) {
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  if (shown) {
    return <CodeReveal code={shown} email={email} onDone={() => setShown(null)} />;
  }

  return (
    <div className="field">
      <label>{t("rk.title")}</label>
      <div>
        {status === null ? (
          <p className="hint">{t("ext.loading")}</p>
        ) : status.has_key ? (
          <p>{t("rk.has", { date: (status.created_at ?? "").slice(0, 10) })}</p>
        ) : (
          <p className="warn">{t("rk.none")}</p>
        )}
        <p className="hint">{t("rk.why")}</p>
        {status?.pending.map((p) => (
          <div key={p.id} className="notice warnBar" style={{ display: "block" }}>
            <p style={{ margin: 0 }}>{t("rk.pendingMine", { machine: p.machine_name || "?", ip: p.ip ?? "?" })}</p>
            <button
              className="danger"
              disabled={busy}
              onClick={async () => {
                setBusy(true);
                try {
                  await api.recoveryCancel(p.id);
                  await load();
                } catch (e) {
                  setError(say(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              {t("rk.cancelIt")}
            </button>
          </div>
        ))}
        {asking ? (
          <div style={{ display: "grid", gap: "var(--s-2)", maxWidth: 360 }}>
            <input
              type="password"
              placeholder={t("auth.password")}
              value={password}
              autoFocus
              onChange={(e) => setPassword(e.target.value)}
            />
            {totp && (
              <input
                inputMode="numeric"
                autoComplete="one-time-code"
                placeholder={t("auth.code")}
                value={code}
                maxLength={6}
                onChange={(e) => setCode(e.target.value.replace(/\D/g, ""))}
              />
            )}
            <div className="row">
              <button className="primary" disabled={busy || !password || (totp && code.length !== 6)} onClick={() => void create()}>
                {busy ? t("ck.working") : t("rk.make")}
              </button>
              <button className="ghost" disabled={busy} onClick={() => setAsking(false)}>
                {t("ui.cancel")}
              </button>
            </div>
            {status?.has_key && <p className="hint">{t("rk.replaces")}</p>}
          </div>
        ) : (
          <button disabled={status === null} onClick={() => setAsking(true)}>
            {status?.has_key ? t("rk.makeNew") : t("rk.make")}
          </button>
        )}
        {error && <p className="error">{error}</p>}
      </div>
    </div>
  );
}

/** The code, the one time it exists outside the person's own records.
 *
 *  Copy, save to a file in Downloads, and a box to tick before the screen lets
 *  go: a code shown once and closed by a stray click is a code nobody has. */
export function CodeReveal({
  code,
  email,
  onDone,
  compact,
}: {
  code: string;
  email: string;
  onDone: () => void;
  /** In the narrow sign-in column: no label column beside it. */
  compact?: boolean;
}) {
  const { t, say } = useI18n();
  const [saved, setSaved] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  return (
    <div className={compact ? undefined : "field"} style={compact ? { display: "grid", gap: "var(--s-2)", width: "100%" } : undefined}>
      {!compact && <label>{t("rk.title")}</label>}
      <div>
        <p>{t("rk.showOnce")}</p>
        <div
          className="mono"
          style={{ fontSize: compact ? 15 : 17, lineHeight: 1.7, letterSpacing: 1, userSelect: "all", padding: "var(--s-3)", border: "1px solid var(--line)", borderRadius: "var(--r)", overflowWrap: "anywhere", textAlign: "center" }}
        >
          {code}
        </div>
        <div className="row" style={{ marginTop: "var(--s-2)" }}>
          <button onClick={() => void navigator.clipboard.writeText(code).then(() => setNote(t("rk.copied")))}>{t("rk.copy")}</button>
          <button
            onClick={async () => {
              try {
                setNote(t("rk.savedTo", { path: await api.saveRecoveryFile(code, email) }));
              } catch (e) {
                setNote(say(e));
              }
            }}
          >
            {t("rk.saveFile")}
          </button>
        </div>
        {note && <p className="hint" style={{ marginTop: "var(--s-2)" }}>{note}</p>}
        <p className="hint" style={{ marginTop: "var(--s-2)" }}>{t("rk.whereToKeep")}</p>
        <label className="row" style={{ marginTop: "var(--s-2)" }}>
          <input type="checkbox" style={{ width: 14, height: 14 }} checked={saved} onChange={(e) => setSaved(e.target.checked)} />
          <span>{t("rk.iSaved")}</span>
        </label>
        <button className="primary" style={{ marginTop: "var(--s-2)" }} disabled={!saved} onClick={onDone}>
          {t("rk.done")}
        </button>
      </div>
    </div>
  );
}

/** What the main window says about recovery, polled once a minute:
 *
 *  - to the owner and admins, every open attempt to recover an account in the
 *    team, with a button to end it. For an account with no authenticator and
 *    no mail on the server, this is the 24 hours in which a stolen key can be
 *    stopped, so it is loud and it is at the top;
 *  - to anyone, an attempt on their own account (they are signed in, so it is
 *    likely not them);
 *  - to the owner without a key, the reminder to make one, until they do or
 *    put it off for this session. */
export function RecoveryBanners({ me, onOpenSettings }: { me: Me; onOpenSettings: () => void }) {
  const { t, say } = useI18n();
  const [attempts, setAttempts] = useState<RecoveryAttempt[]>([]);
  const [hasKey, setHasKey] = useState(true);
  const [later, setLater] = useState(() => {
    try {
      return sessionStorage.getItem("fury.recoveryKeyLater") === "1";
    } catch {
      return false;
    }
  });
  const [error, setError] = useState<string | null>(null);
  const staff = me.role === "owner" || me.role === "admin";

  const load = useCallback(async () => {
    try {
      const mine = await api.recoveryStatus();
      setHasKey(mine.has_key);
      const team = staff ? await api.orgRecoveries() : [];
      const all = [...team, ...mine.pending.filter((p) => !team.some((x) => x.id === p.id))];
      setAttempts(all);
    } catch {
      // A server before 0022 has none of this; say nothing rather than fail.
      setAttempts([]);
      setHasKey(true);
    }
  }, [staff]);
  useEffect(() => {
    void load();
    const timer = setInterval(() => void load(), 60_000);
    return () => clearInterval(timer);
  }, [load]);

  return (
    <>
      {attempts.map((a) => (
        <div key={a.id} className="notice warnBar" role="alert">
          <div style={{ flex: 1 }}>
            <strong>{t("rk.attempt", { email: a.email })}</strong>
            <div className="muted" style={{ marginTop: "var(--s-1)" }}>
              {a.factor === "wait" && a.ready_at
                ? t("rk.attemptWait", { when: new Date(a.ready_at).toLocaleString(), machine: a.machine_name || "?", ip: a.ip ?? "?" })
                : t("rk.attemptFrom", { machine: a.machine_name || "?", ip: a.ip ?? "?" })}
            </div>
          </div>
          <button
            className="danger"
            onClick={async () => {
              try {
                await api.recoveryCancel(a.id);
                await load();
              } catch (e) {
                setError(say(e));
              }
            }}
          >
            {t("rk.cancelIt")}
          </button>
        </div>
      ))}
      {error && <div className="notice warnBar">{error}</div>}
      {me.role === "owner" && !hasKey && !later && (
        <div className="notice" role="status">
          <div style={{ flex: 1 }}>
            <strong>{t("rk.ownerReminder")}</strong>
            <div className="muted" style={{ marginTop: "var(--s-1)" }}>{t("rk.ownerReminderWhy")}</div>
          </div>
          <button className="primary" onClick={onOpenSettings}>
            {t("rk.make")}
          </button>
          <button
            className="ghost"
            onClick={() => {
              try {
                sessionStorage.setItem("fury.recoveryKeyLater", "1");
              } catch {
                /* this session only either way */
              }
              setLater(true);
            }}
          >
            {t("rk.later")}
          </button>
        </div>
      )}
    </>
  );
}
