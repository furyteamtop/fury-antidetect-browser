// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

/** The server offered to somebody who has not got one.
 *
 *  Fury is self-hosted and the first screen said so, with two buttons that both
 *  needed an address: connect to a server, or make an account on one. A person
 *  with neither read that as "so where do I get a server?" -- which is a fair
 *  question and had no answer in the application.
 *
 *  There are two answers now and they are for different people. Somebody
 *  working alone needs no server and no account at all: that is the
 *  "work without an account" button, it is the default the README describes,
 *  and the browser spoofs exactly the same either way. Somebody who wants their
 *  profiles on a server, or a team, needs one of these -- and this address is
 *  offered so that not having a machine of your own is not the end of the road.
 *
 *  WHAT SIGNING UP HERE MEANS, because a default that hides this would be worse
 *  than no default. Every sign-up creates its own organisation with the caller
 *  as owner, and an organisation is the boundary every query on that server
 *  already filters on -- so a stranger who signs up cannot address anything of
 *  anyone else's. That is structural rather than a check. The vault is still
 *  wrapped with a key derived from the account password, which the server never
 *  sees; what it holds is ciphertext it cannot open.
 *
 *  It is still somebody else's machine. Anyone who would rather that not be
 *  true can clear the field and type their own, which is why this is a
 *  PREFILLED VALUE and not a hidden default -- the address is visible, editable,
 *  and deleting it costs one keystroke.
 *
 *  The name is what it is because the machine has no domain. sslip.io resolves a
 *  hostname to the address written inside it, which is what makes a certificate
 *  possible at all -- Let's Encrypt will not issue for a bare IP, and without a
 *  certificate every password and vault key would cross the network in the
 *  clear. Replace it here when a real domain exists; nothing else needs to
 *  change.
 */
export const DEFAULT_SERVER = "204-168-178-23.sslip.io";
