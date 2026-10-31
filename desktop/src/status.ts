// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

import { api, type Profile } from "./api";

/** The account's stage (docs/16 5.5): free text with a suggested vocabulary.
 *
 *  Free text because an agency's stages are its own — one runs "farm → warm →
 *  spend → rest", another "new → verified → limited". A fixed enum would be
 *  ours, and the first team whose stages did not fit would put theirs in tags
 *  again, which is where they were before this existed. The suggestions get a
 *  colour and a translation; anything else is shown as typed, in neutral. */
export const SUGGESTED = ["new", "warming", "ready", "working", "paused", "limited", "banned"] as const;
export type Suggested = (typeof SUGGESTED)[number];

/** Hue per known stage; unknown stages are grey. Green for the ones that
 *  earn, amber for the ones that wait, red for the ones that are over. */
export function statusHue(status: string): number | null {
  switch (status.trim().toLowerCase()) {
    case "new": return 210;
    case "warming": return 40;
    case "ready": return 150;
    case "working": return 120;
    case "paused": return 200; // a cool grey-blue: parked, not dead
    case "limited": return 25;
    case "banned": return 5;
    default: return null;
  }
}

export function isSuggested(s: string): s is Suggested {
  return (SUGGESTED as readonly string[]).includes(s.trim().toLowerCase());
}

/** Set one profile's stage, keeping the rest as the list has it. The agent
 *  takes the proxy by id and refuses a partial proxy object; the server reads
 *  `proxy.id` — the same split as the bulk proxy action in App.tsx. Used by
 *  farming, which marks profiles "warming" and, at the end, "ready". */
export async function setStage(p: Profile, status: string): Promise<void> {
  await api.saveProfile(
    p.origin === "local" ? { ...p, proxy_id: p.proxy?.id ?? "", proxy: null, status } : { ...p, status },
    p.origin,
  );
}
