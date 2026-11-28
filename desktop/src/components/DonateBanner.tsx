// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useRef, useState } from "react";
import { USDT_TRC20 } from "../donate";
import { useI18n } from "../i18n";
import { Icon } from "./Icon";

/** The request for donations, shown after an update.
 *
 *  Asked for by the owner for 0.7.4 (28.11.2026): once after an update, no
 *  more often than every two weeks, in English with Russian behind a button.
 *  Releases come out every few days, so "after every update" alone would have
 *  meant a request every few days; the two weeks are counted from the last
 *  time it was shown, and an update inside them passes without one.
 *
 *  The text is deliberately outside the interface dictionary: it is the
 *  author's own words in the two languages he wrote them in, not interface
 *  copy, and it should not be machine-turned into thirteen more. */

const SEEN = "fury.donate.seenVersion";
const SHOWN = "fury.donate.shownAt";
const GAP_MS = 14 * 24 * 60 * 60 * 1000;

/** Whether this launch shows the request, and the bookkeeping for it.
 *
 *  A version not seen before is an update, with one exception: a first launch
 *  ever. Before 0.7.4 nothing recorded the version, so "no record" is told
 *  apart by whether this window has kept anything else of Fury's (language,
 *  theme, a dismissed update, a session): an operator updating from 0.7.3 has,
 *  a fresh install has not, and a fresh install is not asked for money on its
 *  first screen. */
export function takeDonateTurn(version: string, now = Date.now()): boolean {
  // Decided once per window: the answer writes to storage, so asking again
  // (React's development double render, a remount) would answer "no".
  if (decided?.version !== version) decided = { version, show: decide(version, now) };
  return decided.show;
}

let decided: { version: string; show: boolean } | null = null;

function decide(version: string, now: number): boolean {
  try {
    const seen = localStorage.getItem(SEEN);
    if (seen === version) return false;
    localStorage.setItem(SEEN, version);
    if (seen === null) {
      const before = Object.keys(localStorage).some((k) => k.startsWith("fury.") && k !== SEEN);
      if (!before) return false;
    }
    const last = Number(localStorage.getItem(SHOWN) ?? 0);
    if (last && now - last < GAP_MS) return false;
    localStorage.setItem(SHOWN, String(now));
    return true;
  } catch {
    // No storage, no memory of having asked: better not to ask on every start.
    return false;
  }
}

type Lang = "en" | "ru";

const text: Record<Lang, {
  title: string;
  lead: string;
  costs: string[];
  count: string;
  ask: string;
  network: string;
  copy: string;
  copied: string;
  copyFailed: string;
  other: string;
  close: string;
}> = {
  en: {
    title: "Fury is free. Help keep it that way.",
    lead: "Fury costs nothing and will stay that way. But building it does cost money and time every month:",
    costs: [
      "A rented Windows server builds every Windows release.",
      "For the Mac version, my MacBook Air M5 compiles each new browser core for about 7 hours. That happens at least twice a month so Fury stays on a current Chromium.",
      "On top of that come dozens of your requests and fixes every month.",
    ],
    count: "Fury has shipped over 100 updates so far.",
    ask: "Keeping all of this going costs about $100 a month. If Fury saves you time or money, a donation of any size helps the next updates come out.",
    network: "USDT on the TRON network (TRC20) only. Sent on another network, it is lost.",
    copy: "Copy",
    copied: "Copied",
    copyFailed: "The clipboard refused. The address is selected: press Ctrl+C (⌘C on a Mac).",
    other: "Русский",
    close: "Close",
  },
  ru: {
    title: "Fury бесплатный. Помогите ему таким и остаться.",
    lead: "Fury ничего не стоит и стоить не будет. Но его сборка каждый месяц требует денег и времени:",
    costs: [
      "Арендованный сервер на Windows собирает каждую версию под Windows.",
      "Версию для Mac собирает мой MacBook Air M5: каждое новое ядро браузера компилируется около 7 часов. Так бывает минимум два раза в месяц, чтобы Fury держался на свежем Chromium.",
      "И к этому каждый месяц добавляются десятки ваших просьб и доработок.",
    ],
    count: "У Fury уже вышло больше 100 обновлений.",
    ask: "Всё это обходится примерно в $100 в месяц. Если Fury экономит вам время или деньги, помогите донатом любого размера, так новые обновления будут выходить и дальше.",
    network: "Только USDT в сети TRON (TRC20). Отправленное в другой сети пропадёт.",
    copy: "Копировать",
    copied: "Скопировано",
    copyFailed: "Буфер обмена отказал. Адрес выделен: нажмите Ctrl+C (⌘C на Mac).",
    other: "English",
    close: "Закрыть",
  },
};

export function DonateBanner({ onClose }: { onClose: () => void }) {
  const { resolved } = useI18n();
  // English first, as written; an interface already in Russian or Ukrainian
  // opens on the Russian text instead of making its reader press for it.
  const [lang, setLang] = useState<Lang>(resolved === "ru" || resolved === "uk" ? "ru" : "en");
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const code = useRef<HTMLElement>(null);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const s = text[lang];

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(USDT_TRC20);
      setState("copied");
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setState((x) => (x === "copied" ? "idle" : x)), 2000);
    } catch {
      // As in Settings: a refusing clipboard leaves the address selected.
      if (code.current) {
        const range = document.createRange();
        range.selectNodeContents(code.current);
        const sel = window.getSelection();
        sel?.removeAllRanges();
        sel?.addRange(range);
      }
      setState("failed");
    }
  };

  return (
    <div className="notice donateBanner" role="status" lang={lang}>
      <div className="donateHead">
        <strong>{s.title}</strong>
        <div className="row">
          <button className="ghost" onClick={() => setLang(lang === "en" ? "ru" : "en")}>
            {s.other}
          </button>
          <button className="ghost" onClick={onClose} aria-label={s.close} title={s.close}>
            <Icon name="close" size={13} />
          </button>
        </div>
      </div>
      <p>{s.lead}</p>
      <ul>
        {s.costs.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ul>
      <p>{s.count}</p>
      <p>{s.ask}</p>
      <div className="row" style={{ flexWrap: "wrap" }}>
        <span className="muted">USDT (TRC20):</span>
        <code className="mono" ref={code} style={{ userSelect: "all", wordBreak: "break-all" }}>
          {USDT_TRC20}
        </code>
        <button className="primary" onClick={() => void copy()}>
          <Icon name="copy" size={13} /> {state === "copied" ? s.copied : s.copy}
        </button>
      </div>
      <p className="hint">{state === "failed" ? s.copyFailed : s.network}</p>
    </div>
  );
}
