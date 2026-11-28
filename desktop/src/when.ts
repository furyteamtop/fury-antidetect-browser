// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useEffect, useState } from "react";
import { languageNames, type Resolved } from "./i18n";

/** How a list says when something last happened: "4 hours ago", or the date.
 *
 *  A tester asked for the first (26.11.2026): what he reads the column for is
 *  which profiles have gone stale, and a full date makes him do the
 *  subtraction. Others match rows against their own records and want the date,
 *  so it is a choice, and whichever is not shown is on hover.
 *
 *  Remembered on this machine only, like the sort: a way of looking. */
export const whenStyles = ["ago", "date"] as const;
export type WhenStyle = (typeof whenStyles)[number];

const KEY = "fury.when";

function stored(): WhenStyle {
  try {
    return localStorage.getItem(KEY) === "date" ? "date" : "ago";
  } catch {
    return "ago";
  }
}

// Chosen in the settings dialog while the table is on screen behind it.
const listeners = new Set<(s: WhenStyle) => void>();

export function useWhenStyle(): [WhenStyle, (s: WhenStyle) => void] {
  const [style, setStyle] = useState<WhenStyle>(stored);
  useEffect(() => {
    listeners.add(setStyle);
    return () => {
      listeners.delete(setStyle);
    };
  }, []);
  return [
    style,
    (s) => {
      try {
        localStorage.setItem(KEY, s);
      } catch {
        // Remembered for this window only.
      }
      listeners.forEach((fn) => fn(s));
    },
  ];
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** Whole units that have passed, the largest that fits. The words and their
 *  plural forms are the web view's own, in the interface's language. */
export function ago(iso: string, language: Resolved, now = Date.now()): string {
  const rtf = new Intl.RelativeTimeFormat(languageNames[language].tag, { numeric: "always" });
  // A server whose clock is ahead of this machine's would otherwise say
  // "in 2 minutes" about something that has happened.
  const past = Math.max(0, now - Date.parse(iso));
  if (past < MINUTE) {
    return new Intl.RelativeTimeFormat(languageNames[language].tag, { numeric: "auto" }).format(0, "second");
  }
  if (past < HOUR) return rtf.format(-Math.floor(past / MINUTE), "minute");
  if (past < DAY) return rtf.format(-Math.floor(past / HOUR), "hour");
  if (past < 30 * DAY) return rtf.format(-Math.floor(past / DAY), "day");
  if (past < 365 * DAY) return rtf.format(-Math.min(11, Math.floor(past / (30 * DAY))), "month");
  return rtf.format(-Math.floor(past / (365 * DAY)), "year");
}

/** What to show, and what to put on hover. */
export function when(iso: string, style: WhenStyle, language: Resolved): { text: string; title: string } {
  const date = new Date(iso).toLocaleString();
  const rel = ago(iso, language);
  return style === "ago" ? { text: rel, title: date } : { text: date, title: rel };
}
