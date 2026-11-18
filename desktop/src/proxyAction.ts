// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

export type ProxyAction = {
  action: "check";
  status: "pending" | "ok" | "error";
  exit?: { ip?: string; country?: string; timezone?: string };
  ms?: number;
  error?: string;
  errorCode?: string | null;
  checkerCountry?: string | null;
};

export const proxyActionKey = (origin: "local" | "team", id: string) => `${origin}:${id}`;
