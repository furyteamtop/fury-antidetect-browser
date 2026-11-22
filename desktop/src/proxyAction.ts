// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

export type ProxyExit = { ip?: string | null; country?: string | null; timezone?: string | null };

export type ProxyAction = {
  action: "check" | "rotate";
  status: "pending" | "ok" | "error";
  /** `null` means a successful rotation invalidated the old exit. */
  exit?: ProxyExit | null;
  ms?: number;
  checkerCountry?: string | null;
  error?: string;
  errorCode?: string | null;
  /** The displayed exit predates a completed rotation and is being replaced. */
  afterRotation?: boolean;
};

export const proxyActionKey = (origin: "local" | "team", id: string) => `${origin}:${id}`;

/** An in-session observation takes precedence over the saved exit. */
export const proxyExit = (action: ProxyAction | undefined, saved: ProxyExit): ProxyExit | null =>
  action?.exit === undefined ? saved : action.exit;
