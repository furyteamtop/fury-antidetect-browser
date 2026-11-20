// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import type { ReactNode } from "react";
import type { Dir, SortState } from "../sort";

/** A column header that sorts its table (sort.ts). The whole label is the
 *  button, and the arrow appears only on the column that is sorted: a column
 *  of arrows on every header would say nothing about which one is in force.
 *  aria-sort carries the same fact to a screen reader. */
export function SortTh<K extends string>({
  k,
  sort,
  first = "asc",
  children,
}: {
  k: K;
  sort: SortState<K> & { click: (key: K, first?: Dir) => void };
  /** The direction of the first click: names read A to Z, dates and counts
   *  read biggest first. */
  first?: Dir;
  children: ReactNode;
}) {
  const on = sort.key === k;
  return (
    <th aria-sort={on ? (sort.dir === "asc" ? "ascending" : "descending") : "none"}>
      <button type="button" className={`sortHead${on ? " on" : ""}`} onClick={() => sort.click(k, first)}>
        {children}
        <span className="sortArrow" aria-hidden="true">
          {on ? (sort.dir === "asc" ? "▲" : "▼") : "▲"}
        </span>
      </button>
    </th>
  );
}
