// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { useState } from "react";

/** Sorting a table by its column headers.
 *
 *  A click on a header sorts by that column in the direction it is usually
 *  read (names A to Z, dates newest first), a second click turns it round, a
 *  third goes back to the order the list came in. The choice is remembered per
 *  table on this machine only: it is a way of looking, not a property of the
 *  profiles, so it goes nowhere near the server.
 *
 *  Empty values (no proxy, never opened, no project) stay at the bottom in
 *  either direction. Reversing a sort by date is asking for the oldest first,
 *  not for forty "never" rows ahead of them. */
export type Dir = "asc" | "desc";
export interface SortState<K extends string> {
  key: K | null;
  dir: Dir;
}

export function useSort<K extends string>(storageKey: string, keys: readonly K[]) {
  const [state, setState] = useState<SortState<K>>(() => {
    try {
      const raw = localStorage.getItem(storageKey);
      if (raw) {
        const v = JSON.parse(raw) as SortState<K>;
        if (v && keys.includes(v.key as K) && (v.dir === "asc" || v.dir === "desc")) return v;
      }
    } catch {
      // A blocked or corrupt store means the list's own order, nothing worse.
    }
    return { key: null, dir: "asc" };
  });

  const click = (key: K, first: Dir = "asc") =>
    setState((s) => {
      const next: SortState<K> =
        s.key !== key
          ? { key, dir: first }
          : s.dir === first
            ? { key, dir: first === "asc" ? "desc" : "asc" }
            : { key: null, dir: "asc" };
      try {
        if (next.key === null) localStorage.removeItem(storageKey);
        else localStorage.setItem(storageKey, JSON.stringify(next));
      } catch {
        // Remembered for this window only.
      }
      return next;
    });

  return { ...state, click };
}

/** Numeric-aware, case-blind: "Profile 2" before "Profile 10", "amazon" beside
 *  "Amazon". The interface language is not the collation's business. */
const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** One comparable value, or several for tie-breaks. Null and "" are empty. */
export type SortValue = string | number | null | undefined;

function isEmpty(v: SortValue): boolean {
  return v === null || v === undefined || v === "";
}

function compareOne(a: SortValue, b: SortValue): number {
  if (typeof a === "number" && typeof b === "number") return a - b;
  return collator.compare(String(a), String(b));
}

/** A sorted copy. Stable, so rows that tie keep the order they came in. */
export function sortRows<T, K extends string>(
  rows: T[],
  state: SortState<K>,
  valueOf: (row: T, key: K) => SortValue | SortValue[],
): T[] {
  const key = state.key;
  if (key === null) return rows;
  const sign = state.dir === "asc" ? 1 : -1;
  const keyed = rows.map((row) => {
    const v = valueOf(row, key);
    return { row, v: Array.isArray(v) ? v : [v] };
  });
  keyed.sort((x, y) => {
    for (let i = 0; i < Math.max(x.v.length, y.v.length); i++) {
      const a = x.v[i];
      const b = y.v[i];
      const ea = isEmpty(a);
      const eb = isEmpty(b);
      if (ea && eb) continue;
      // Empty last whatever the direction; only the first value decides
      // where a row with nothing to sort by goes.
      if (ea) return 1;
      if (eb) return -1;
      const c = compareOne(a, b);
      if (c !== 0) return c * sign;
    }
    return 0;
  });
  return keyed.map((k) => k.row);
}
