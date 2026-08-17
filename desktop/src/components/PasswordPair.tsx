// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useState } from "react";
import { useI18n } from "../i18n";

/** No `l`, `I`, `1`, `O` or `0`, and no punctuation.
 *
 *  This password gets written down — on paper, in a chat message to oneself,
 *  into a password manager by hand — because it is the one secret in the
 *  product that nobody can reset. A character pair that cannot be told apart in
 *  a screenshot, or a symbol that sits somewhere else on a Russian keyboard,
 *  turns that into a locked account. Fifty-six characters over twenty places is
 *  116 bits, which is far past anything the argon2 hash behind it needs. */
const ALPHABET = "abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/** One character, uniformly. The rejection loop is not decoration: `% 56` over
 *  a raw 32-bit draw favours the first eight letters of the alphabet, and a
 *  generator with a thumb on the scale is worse than none because nobody can
 *  see it. */
function pick(n: number): number {
  const limit = Math.floor(0x100000000 / n) * n;
  const buf = new Uint32Array(1);
  for (;;) {
    crypto.getRandomValues(buf);
    if (buf[0] < limit) return buf[0] % n;
  }
}

/** Grouped in fours, like the invitation code two screens away. Somebody
 *  copying twenty characters by eye loses their place; five short groups do not
 *  need a place to be kept. The hyphens are part of the password. */
function generate(groups = 5, size = 4): string {
  const out: string[] = [];
  for (let g = 0; g < groups; g++) {
    let group = "";
    for (let i = 0; i < size; i++) group += ALPHABET[pick(ALPHABET.length)];
    out.push(group);
  }
  return out.join("-");
}

/** Choose a password, twice, with a way to not have to choose it.
 *
 *  Shared by enrolment and sign-up because both ask exactly this and both carry
 *  the same warning: what is typed here derives the key that opens the team's
 *  data, no copy exists anywhere, and nobody — including whoever runs the
 *  server — can reset it.
 *
 *  Which is the argument for the generate button rather than against it. The
 *  two empty boxes and a twelve-character minimum are an invitation to reuse a
 *  password the person already knows, on the one account where reuse cannot be
 *  undone. Pressing the button produces something that was never anywhere else,
 *  fills both fields with it, and — this is the part that matters — SHOWS it,
 *  because a secret that cannot be read cannot be saved, and this one has to be
 *  saved before the button below is pressed. */
export function PasswordPair({
  password,
  again,
  onPassword,
  onAgain,
  autoFocus,
}: {
  password: string;
  again: string;
  onPassword: (v: string) => void;
  onAgain: (v: string) => void;
  autoFocus?: boolean;
}) {
  const { t } = useI18n();
  const [shown, setShown] = useState(false);
  const [copied, setCopied] = useState(false);
  const [made, setMade] = useState(false);
  const [copyFailed, setCopyFailed] = useState(false);

  const copy = async () => {
    setCopyFailed(false);
    try {
      await navigator.clipboard.writeText(password);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      // A webview that refuses clipboard writes. Said out loud rather than
      // swallowed: a button that does nothing visible leaves somebody believing
      // they have a copy of the one password nobody can reissue. The field is
      // revealed instead, so there is something to select by hand.
      setShown(true);
      setCopyFailed(true);
    }
  };

  return (
    <>
      <input
        type={shown ? "text" : "password"}
        className={shown ? "mono" : undefined}
        placeholder={t("enrol.password")}
        value={password}
        autoFocus={autoFocus}
        spellCheck={false}
        autoCapitalize="off"
        autoComplete="new-password"
        onChange={(e) => {
          onPassword(e.target.value);
          setMade(false);
        }}
      />
      <input
        type={shown ? "text" : "password"}
        className={shown ? "mono" : undefined}
        placeholder={t("enrol.passwordAgain")}
        value={again}
        spellCheck={false}
        autoCapitalize="off"
        autoComplete="new-password"
        onChange={(e) => {
          onAgain(e.target.value);
          setMade(false);
        }}
      />

      <div className="pwTools">
        <button
          type="button"
          className="alt"
          onClick={() => {
            const made = generate();
            onPassword(made);
            // Both fields, together. Asking somebody to retype twenty random
            // characters they did not choose is asking them to fail, and the
            // second box exists to catch a typo in something typed — there is
            // nothing to catch here.
            onAgain(made);
            setShown(true);
            setMade(true);
          }}
        >
          {t("pw.generate")}
        </button>
        <button type="button" className="alt" onClick={() => setShown((s) => !s)}>
          {shown ? t("pw.hide") : t("pw.show")}
        </button>
        <button type="button" className="alt" disabled={!password} onClick={() => void copy()}>
          {copied ? t("pw.copied") : t("pw.copy")}
        </button>
      </div>

      {/* A hint and not a second warning. The sentence under this one — that
          nobody can reset this password — is the amber one, and two blocks of
          amber in a column stop reading as urgent and start reading as
          decoration. */}
      {made && <p className="hint">{t("pw.saveIt")}</p>}
      {copyFailed && <p className="hint">{t("pw.copyFailed")}</p>}
    </>
  );
}
