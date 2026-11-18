// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

/** The one address donations go to: USDT on TRON (TRC20), the author's own.
 *
 *  Written in four tracked places (this file, both READMEs and the site), and
 *  tools/ci/check-wallet.py fails the build when any of them differs from the
 *  reference held in that script. The usual attack on a donation address in a
 *  public repository is not a break-in but one changed character in a pull
 *  request or a fork; a mismatch has to stop CI rather than wait for somebody
 *  to notice. */
export const USDT_TRC20 = "TNvKC6tXKtsxA4tDrjU86zbmwMGuxfmatq";
