// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors
//
// The logins that belong to a profile, and the two-factor codes for them.
//
// Why this exists at all: a shared account with two-factor authentication has a
// seed, and in practice that seed lives in a group chat or in one person's
// phone. The chat is forever and searchable; the phone is a colleague who has
// to be awake. Here it lives with the profile, sealed with the machine key, and
// the code is generated on the operator's own machine when they ask.
//
// What it deliberately does not do is type the code into the page. A browser
// that fills a one-time code by itself does it at a speed no person does, and
// that timing is measurable from the page. It is shown; a person pastes it.

import { useEffect, useRef, useState } from "react";

import { api, type Credential, type TotpCode } from "../api";
import { useI18n } from "../i18n";

function blank(profileId: string): Credential {
  return {
    id: "",
    profile_id: profileId,
    label: "",
    site: "",
    username: null,
    password: null,
    totp: null,
    notes: "",
  };
}

export function Logins({ profileId }: { profileId: string }) {
  const { t, say } = useI18n();
  const [rows, setRows] = useState<Credential[]>([]);
  const [editing, setEditing] = useState<Credential | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = async () => {
    try {
      setRows(await api.credentials(profileId));
      setError(null);
    } catch (e) {
      setError(say(e));
    }
  };

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profileId]);

  const save = async (c: Credential) => {
    setBusy(true);
    try {
      await api.saveCredential(c);
      setEditing(null);
      await load();
    } catch (e) {
      // Shown against the form rather than as a toast: the commonest failure is
      // a seed that is not a seed, and the person needs to see it beside the
      // box they pasted into.
      setError(say(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="logins">
      <header className="rowBetween">
        <h3>{t("cred.title")}</h3>
        <button className="ghost" onClick={() => setEditing(blank(profileId))}>
          {t("cred.add")}
        </button>
      </header>

      {error && (
        <div className="notice" role="status">
          {error}
          <button className="ghost" onClick={() => setError(null)}>
            {t("app.dismiss")}
          </button>
        </div>
      )}

      {rows.length === 0 && !editing && <p className="muted">{t("cred.empty")}</p>}

      {rows.map((c) => (
        <LoginRow
          key={c.id}
          credential={c}
          onEdit={() => setEditing(c)}
          onDelete={async () => {
            await api.deleteCredential(c.id);
            await load();
          }}
        />
      ))}

      {editing && (
        <LoginForm
          credential={editing}
          busy={busy}
          onCancel={() => setEditing(null)}
          onSave={save}
        />
      )}
    </section>
  );
}

function LoginRow({
  credential,
  onEdit,
  onDelete,
}: {
  credential: Credential;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const { t } = useI18n();
  const [shown, setShown] = useState(false);
  return (
    <div className="loginRow">
      <div>
        <strong>{credential.label || credential.site || credential.username}</strong>
        {credential.site && <div className="muted">{credential.site}</div>}
        {credential.username && <div className="mono">{credential.username}</div>}
      </div>
      <div className="loginSecrets">
        {credential.password && (
          <button
            className="ghost mono"
            title={t("cred.copyPassword")}
            onClick={() => void navigator.clipboard.writeText(credential.password ?? "")}
          >
            {shown ? credential.password : "••••••••"}
          </button>
        )}
        {credential.password && (
          <button className="ghost" onClick={() => setShown(!shown)}>
            {shown ? t("cred.hide") : t("cred.show")}
          </button>
        )}
        {credential.totp && <TotpBadge profileId={credential.profile_id} id={credential.id} />}
      </div>
      <div>
        <button className="ghost" onClick={onEdit}>
          {t("row.edit")}
        </button>
        <button className="ghost" onClick={onDelete}>
          {t("row.delete")}
        </button>
      </div>
    </div>
  );
}

/// Six digits, a countdown, and the next one.
function TotpBadge({ profileId, id }: { profileId: string; id: string }) {
  const { t, say } = useI18n();
  const [code, setCode] = useState<TotpCode | null>(null);
  const [error, setError] = useState<string | null>(null);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    let alive = true;
    const tick = async () => {
      try {
        const c = await api.totpCode(profileId, id);
        if (alive) {
          setCode(c);
          setError(null);
        }
      } catch (e) {
        if (alive) setError(say(e));
      }
    };
    void tick();
    // Asked again a second after this code expires rather than on a fixed
    // interval: a one-second poll would ask the agent thirty times per code for
    // an answer that changes once, and drift would eventually show a code that
    // had already lapsed.
    const schedule = () => {
      const wait = ((code?.seconds_remaining ?? 30) + 0.2) * 1000;
      timer.current = window.setTimeout(async () => {
        await tick();
        schedule();
      }, wait);
    };
    schedule();
    return () => {
      alive = false;
      if (timer.current) window.clearTimeout(timer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [profileId, id]);

  if (error) return <span className="muted">{error}</span>;
  if (!code) return <span className="muted">…</span>;

  return (
    <span className="totp">
      <button
        className="ghost mono totpCode"
        title={t("cred.copyCode")}
        onClick={() => void navigator.clipboard.writeText(code.code)}
      >
        {code.code}
      </button>
      <span className="muted" title={t("cred.next", { code: code.next })}>
        {code.seconds_remaining}s
      </span>
    </span>
  );
}

function LoginForm({
  credential,
  busy,
  onCancel,
  onSave,
}: {
  credential: Credential;
  busy: boolean;
  onCancel: () => void;
  onSave: (c: Credential) => void;
}) {
  const { t } = useI18n();
  const [draft, setDraft] = useState<Credential>(credential);
  const set = (k: keyof Credential, v: string) =>
    setDraft({ ...draft, [k]: v === "" && k !== "label" && k !== "site" && k !== "notes" ? null : v });

  return (
    <form
      className="loginForm"
      onSubmit={(e) => {
        e.preventDefault();
        onSave(draft);
      }}
    >
      <label>
        {t("cred.label")}
        <input value={draft.label} onChange={(e) => set("label", e.target.value)} required />
      </label>
      <label>
        {t("cred.site")}
        <input value={draft.site} onChange={(e) => set("site", e.target.value)} placeholder="example.com" />
      </label>
      <label>
        {t("cred.username")}
        <input value={draft.username ?? ""} onChange={(e) => set("username", e.target.value)} />
      </label>
      <label>
        {t("cred.password")}
        <input value={draft.password ?? ""} onChange={(e) => set("password", e.target.value)} />
      </label>
      <label>
        {t("cred.totp")}
        <input
          value={draft.totp ?? ""}
          onChange={(e) => set("totp", e.target.value)}
          placeholder="JBSW Y3DP EHPK 3PXP  —  otpauth://totp/…"
        />
        {/* Says both shapes because people have both: the string beside the QR
            code, and the link behind it. */}
        <span className="muted">{t("cred.totpHint")}</span>
      </label>
      <div className="rowEnd">
        <button type="button" className="ghost" onClick={onCancel}>
          {t("ui.cancel")}
        </button>
        <button type="submit" disabled={busy}>
          {t("ui.save")}
        </button>
      </div>
    </form>
  );
}
