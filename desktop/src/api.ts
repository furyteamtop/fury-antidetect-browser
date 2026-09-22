// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

// The server is the only authority on what a user may do. Nothing here decides
// permissions; it reads the set the server resolved and renders accordingly.
//
// Two transports, one surface:
//
//   * In the packaged app, every call is a Tauri command. The session token
//     lives in Rust and in the OS keychain, and never enters this document.
//   * Under `npm run dev` in a plain browser there is no Rust side, so calls go
//     out via fetch through Vite's proxy and the token sits in localStorage.
//
// The browser path is kept because the edit-reload loop for the interface is
// seconds there and tens of seconds through a Tauri rebuild. It is a
// development convenience and is not what ships — which is why the token
// handling differs, and why that difference is stated here rather than hidden.

import { invoke } from "@tauri-apps/api/core";

export const isDesktop =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export type Perm =
  | "view" | "launch" | "edit_profile" | "edit_fingerprint" | "edit_proxy"
  | "reveal_secrets" | "export_cookies" | "create_profile" | "delete_profile"
  | "manage_access";

export interface Project {
  /** Which world it lives in: this machine, or the server. */
  origin: Origin;
  id: string;
  name: string;
  profile_count: number;
}

export interface ProxySummary {
  id: string;
  name: string;
  kind: string;
  /** Already masked by the server when the caller lacks reveal_secrets.
   *  Never masked in local mode — there is nobody to hide it from. */
  display: string;
  country: string | null;
  /** The exit's zone from the last check, if any — what "follow the exit" resolves to. */
  last_timezone?: string | null;
}

export interface LockInfo {
  user_id: string;
  user_email: string;
  machine_name: string;
  acquired_at: string;
  expires_at: string;
}

/** A login stored beside a profile: what a shared account needs besides cookies. */
export interface Credential {
  id: string;
  profile_id: string;
  label: string;
  site: string;
  username: string | null;
  password: string | null;
  /** An `otpauth://` URI once saved; a bare base32 secret is accepted on input.
   *  Present here because the operator has to be able to see and correct what
   *  they typed — there is nobody on this machine to hide it from. */
  totp: string | null;
  notes: string;
  /** Team mode only: the server had this login and this machine's organisation
   *  key did not open it — almost always a key rotated after this copy was
   *  handed over. Shown, not hidden: an empty row reads as a deleted password. */
  unreadable?: boolean;
}

export interface TotpCode {
  code: string;
  seconds_remaining: number;
  /** The one after this. Shown so a code handed over with two seconds left is
   *  not a login that fails for a reason nobody blames on the clock. */
  next: string;
}

/** Which world a row came from: this machine's own store, or the server.
 *
 *  Connected to a server the list carries both, so the shell's mode no longer
 *  tells an action where to send itself -- the row does. Every call that acts
 *  on ONE existing profile passes it. */
export type Origin = "local" | "team" | "shared";

export interface Profile {
  id: string;
  origin: Origin;
  /** How many people hold this profile through a share. Zero for a local one. */
  shared_with: number;
  /** Null when the profile is in no project. Profiles is the master list —
   *  every profile on this machine — and a project is a grouping a profile can
   *  be put into or taken out of without ever being at risk. */
  project_id: string | null;
  /** Where it is filed, so the flat list can show it without a call per row. */
  project_name: string | null;
  name: string;
  /** Round-tripped by the editor; both were absent from the row until 12.09.2026,
   *  and the editor wrote the absence back on every save. */
  notes?: string;
  start_urls?: string[];
  /** The account's stage — see status.ts. Empty is none. */
  status?: string;
  tags: string[];
  persona_id: string;
  /** Zero in team mode — the server never exposes a seed. */
  fp_seed: number;
  proxy: ProxySummary | null;
  /** Which proxy, on the way IN. `proxy` above is what comes back OUT. */
  proxy_id?: string | null;
  /** Null means "follow the proxy's exit", resolved at launch. */
  timezone: string | null;
  languages: string[] | null;
  /** Names of domain lists the relay applies. Local profiles only; absent
   *  from what a server returns. */
  blocklists?: string[];
  lock: LockInfo | null;
  permissions: Perm[];
  /** Local mode only: the agent knows what it launched. In team mode a
   *  colleague's browser shows up through `lock`, not here. */
  running: boolean;
  last_opened_at: string | null;
}

/** A named list of domains the relay refuses — or, with `@allow-only` on its
 *  first line, the only domains it permits. */
export interface DomainList {
  name: string;
  domains: number;
  allow_only: boolean;
}

export interface LoginOutcome {
  me: Me | null;
  challenge: string | null;
  must_enrol_totp: boolean;
}

export interface SecurityPolicy {
  second_factor: "off" | "new_device" | "always";
  ip_allowlist: string[];
  owner_exempt_from_allowlist: boolean;
  /** A fresh code before purging, removing a member, changing the policy. */
  sensitive_actions_2fa: boolean;
}

export interface LoginEvent {
  id: number;
  email: string;
  outcome: string;
  ip: string | null;
  machine_name: string;
  user_agent: string | null;
  at: string;
}

export interface SessionRow {
  id: string;
  user_id: string;
  email: string;
  machine_name: string;
  ip: string | null;
  user_agent: string | null;
  created_at: string;
  last_seen_at: string;
  current: boolean;
}

export interface WarmPlan {
  urls: string[];
  dwell_seconds: [number, number];
  follow_link: boolean;
  close_after: boolean;
}

export interface WarmProgress {
  profile_id: string;
  name: string;
  total: number;
  done: number;
  current: string | null;
  cookies_before: number;
  cookies_now: number;
  finished: boolean;
  stopped: boolean;
  error: string | null;
  started_at_ms: number;
}

export interface OrgDomainList {
  id: string;
  name: string;
  body: string;
  updated_at: string;
  domains: number;
  allow_only: boolean;
}

export interface MirrorStatus {
  active: boolean;
  typing: boolean;
  members: { profile_id: string; name: string; pages: number }[];
  mirrored: number;
}

export interface Diagnosis {
  ok: boolean;
  steps: { step: string; ok: boolean; ms: number | null; detail: string; code: string | null }[];
  exit: {
    ip: string | null; country: string | null; region: string | null; city: string | null;
    timezone: string | null; org: string | null;
  };
  notes: { code: string; detail: string }[];
}

export interface Extension {
  id: string;
  name: string;
  version: string;
  path: string;
}

/** What the batch dialog remembers under a name. Never a seed, never a
 *  persona: those are per profile by design. */
export interface ProfileTemplate {
  name: string;
  pattern: string;
  proxy_id: string;
  tags: string[];
  status: string;
  start_urls: string[];
  languages: string[];
  timezone: string;
  notes: string;
}

export interface ExtensionEverywhere {
  id: string;
  name: string;
  version: string;
  profiles: { id: string; name: string; version: string }[];
}

/** One row of shared/extensions/catalogue.json. */
export interface CatalogueEntry {
  id: string;
  name: string;
  summary: { en: string; ru: string };
  category: string;
  homepage: string;
  licence: string;
  added_by: string;
  added_on: string;
}

export interface StoreInstallResult {
  extension: Extension | null;
  installed: string[];
  skipped: { id: string; reason: string }[];
  /** How many distinct proxies the package was fetched through. */
  routes: number;
}

export interface Usage {
  total: number;
  cache: number;
  keep: number;
  files: number;
}

export interface Persona {
  id: string;
  os: string;
  gpu: string;
  screen: string;
  weight: number;
  source: string | null;
}

export interface Preview {
  user_agent: string;
  platform: string;
  languages: string[];
  timezone: string;
  hardware_concurrency: number;
  device_memory: number;
  screen: string;
  gpu_vendor: string;
  gpu_renderer: string;
  client_hints_platform: string;
  chrome_version: string;
  client_hints: string;
  avail: string;
  device_pixel_ratio: number;
  color_depth: number;
  max_touch_points: number;
  /** null when the persona has no WebGPU adapter data. */
  webgpu: string | null;
  webgl_extensions: number;
  audio_sample_rate: number;
  /** null when the persona carries no voice list — the vector then follows the host. */
  voices: number | null;
  media_devices: string | null;
  ui_locale: string;
  webrtc: string;
  js_heap_gb: number | null;
  persona_source: string | null;
  persona_weight: number;
  fonts: number;
  noise: { canvas: boolean; audio: boolean; client_rects: boolean };
  /** Contradictions that make this device impossible. Non-empty blocks saving:
   *  an inconsistent profile stands out more than an un-spoofed one. */
  problems: string[];
}

/** What `parse_proxy_line` found in a pasted line. */
export interface ParsedProxyLine {
  kind: string;
  host: string;
  port: number;
  username: string | null;
  password: string | null;
  shape: "Url" | "HostPort" | "HostPortUserPass" | "AtSign";
}

export interface LocalProxy {
  id: string;
  name: string;
  kind: string;
  host: string;
  port: number;
  username: string | null;
  password: string | null;
  last_country: string | null;
  last_ip: string | null;
  /** The exit's zone from the last check; absent from a server-side proxy. */
  last_timezone?: string | null;
  /** Provider link that hands out a new exit IP. */
  rotate_url: string | null;
  /** Where to ask what the exit looks like; null uses the default. */
  checker_url: string | null;
}

export interface Me {
  user_id: string;
  email: string;
  org_id: string;
  role: "owner" | "admin" | "manager" | "member";
}

/** What the shell knows before anyone signs in. */
export interface Shell {
  server_url: string | null;
  machine_name: string;
  signed_in: boolean;
  native: boolean;
  /** "local" needs no account at all; "team" is a server someone chose. */
  mode: "local" | "team";
  agent_ready: boolean;
  /** Whether a browser is installed at all. The application and the browser are
   *  two downloads, so "the app runs" and "there is something to launch" are
   *  different questions. */
  core_ready: boolean;
  /** The agent's own sentence about why there is none, when it has one — a
   *  stale FURY_CORE reads nothing like a missing download. */
  core_problem: string | null;
  /** Progress of a core download the user asked for, straight from the agent.
   *  Absent until the agent has been asked once. `running` false with
   *  `installed` set means it finished; with `error` set means it did not. */
  /** Where the agent writes its own log. Shown on the About screen: a person
   *  reporting a problem should not have to be told the path per platform. */
  log_file?: string | null;
  core_download?: {
    running: boolean;
    downloaded: number;
    total: number;
    installed: string | null;
    error: string | null;
  } | null;
  /** Whether the organisation key is remembered between launches. */
  remember_org_key: boolean;
  /** This build, for the About panel and for any bug report that follows. */
  version: string;
  /** Whether this process holds the organisation key. A session survives a
   *  restart; the key deliberately does not. So "signed in" and "able to
   *  decrypt" are different states, and both have to be visible. */
  org_key_ready: boolean;
  last_email: string | null;
  /** The server this machine was last pointed at, kept after it stops being
   *  pointed at one — so returning is a button that knows the address rather
   *  than a first-run screen asking for it. */
  last_server: string | null;
  /** Signed in, holding no key, and there is none on the server to hold — the
   *  member enrolled and nobody has handed it over yet. Distinct from
   *  `org_key_ready` being false for the ordinary reason, because a password
   *  fixes one of those and cannot fix the other. False against a server too
   *  old to answer the question. */
  awaiting_key: boolean;
}

/** Note what is absent: the lock token. It authorises overwriting a bundle,
 *  so in the desktop build it stays in Rust — the interface only needs to know
 *  when the lock lapses and how a launch would be constrained. */
export interface LaunchResult {
  /** True when a browser actually started. False in team mode, where all that
   *  happened was taking the lock — saying otherwise would have the operator
   *  waiting for a window that never appears. */
  launched: boolean;
  pid?: number;
  expires_at?: string;
  restrictions?: Record<string, boolean>;
  renewed?: boolean;
}

const TOKEN_KEY = "fury.token";

// A build that once ran in the browser leaves a token behind in the webview's
// data store, where it survives every later launch of the packaged app. The
// server would happily keep renewing it — sessions slide forward on each
// authenticated request — so the copy the desktop stopped using is exactly the
// copy nothing will ever expire. Drop it on sight.
if (isDesktop && typeof localStorage !== "undefined") {
  localStorage.removeItem(TOKEN_KEY);
  localStorage.removeItem("fury.machine_id");
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
    public body: unknown = null,
    /** Set for failures this application produced itself, so the interface can
     *  say them in the operator's language. The message is the fallback. */
    public code: string | null = null,
  ) {
    super(message);
  }
}

/** Turns a status and the server's JSON body into something worth reading. */
function describe(status: number, body: any): string {
  if (body?.error === "denied") {
    return `Not permitted: this action needs "${body.missing_permission}".`;
  }
  if (body?.error === "locked") {
    const h = body.holder ?? {};
    return `In use by ${h.user_email ?? "someone"} on ${h.machine_name ?? "another machine"}.`;
  }
  if (status === 401) return "Session expired. Sign in again.";
  // 404 covers both "gone" and "not yours" — the server deliberately does not
  // distinguish them, so neither does this message.
  if (status === 404) return "Not found, or you no longer have access.";
  if (body?.message) return body.message;
  // status 0 means the request never reached a server; the Rust side has
  // already written a specific sentence for that case.
  if (status === 0) return "Could not reach the server.";
  return `Request failed (${status}).`;
}

// ---------------------------------------------------------------------------
// desktop transport
// ---------------------------------------------------------------------------

/** Rust rejects with the serialised ApiErr — {status, body, message}. */
/** What an invitation code is for. `creates_org_key` is the server's word, not
 *  the client's: a member who decided for themselves that they were an owner
 *  would generate a second organisation key, and their data would then be
 *  readable by nobody. */
export type Invitation = {
  email: string;
  organization: string;
  role: string;
  creates_org_key: boolean;
};

async function cmd<T>(name: string, args: Record<string, unknown> = {}): Promise<T> {
  try {
    return await invoke<T>(name, args);
  } catch (raw: any) {
    const status = typeof raw?.status === "number" ? raw.status : 0;
    const body = raw?.body ?? null;
    // Prefer the message the transport wrote (it names the actual host and
    // failure), falling back to the shared description for server-side errors.
    const fromBody = describe(status, body);
    const message =
      status === 0 && typeof raw?.message === "string" ? raw.message : fromBody;
    throw new ApiError(status, message, body, typeof raw?.code === "string" ? raw.code : null);
  }
}

// ---------------------------------------------------------------------------
// browser transport (development only)
// ---------------------------------------------------------------------------

export function storedToken(): string | null {
  return isDesktop ? null : localStorage.getItem(TOKEN_KEY);
}

async function http<T>(path: string, init: RequestInit = {}): Promise<T> {
  const token = storedToken();
  const res = await fetch(path, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(init.headers ?? {}),
    },
  });

  if (res.status === 204) return undefined as T;
  const body = await res.json().catch(() => null);

  if (!res.ok) {
    // 401 means the session is gone — expired, or revoked because someone
    // removed this operator. Both mean the same thing to the UI, and holding a
    // dead token would only produce a wall of errors.
    if (res.status === 401) localStorage.removeItem(TOKEN_KEY);
    throw new ApiError(res.status, describe(res.status, body), body);
  }
  return body as T;
}

/** Stable per installation, so the lock list names the same browser each time. */
function devMachineId(): string {
  const key = "fury.machine_id";
  let id = localStorage.getItem(key);
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem(key, id);
  }
  return id;
}

// ---------------------------------------------------------------------------

export const api = {
  shell(): Promise<Shell> {
    if (isDesktop) return cmd<Shell>("shell_state");
    return Promise.resolve({
      mode: "team" as const,
      agent_ready: false,
      core_ready: true,
      core_problem: null,
      core_download: null,
      remember_org_key: true,
      version: "dev",
      org_key_ready: false,
      last_email: null,
      last_server: null,
      awaiting_key: false,
      // Vite proxies /v1, so in this mode the address is fixed by the dev
      // config rather than chosen by the operator.
      server_url: "http://127.0.0.1:8901 (vite proxy)",
      machine_name: navigator.platform || "this browser",
      signed_in: storedToken() !== null,
      native: false,
    });
  },

  setServer(url: string): Promise<Shell> {
    if (!isDesktop) {
      return Promise.reject(
        new ApiError(0, "The server address is fixed by vite.config.ts in development."),
      );
    }
    return cmd<Shell>("set_server", { url });
  },

  /** What an invitation code is for, before anyone types a password. The
   *  address travels with the call because this is how someone reaches a server
   *  for the first time — there is nothing in settings yet. */
  async invitation(url: string, code: string): Promise<Invitation> {
    if (!isDesktop) {
      return Promise.reject(
        new ApiError(0, "Enrolment generates keys in Rust and is desktop-only."),
      );
    }
    return cmd<Invitation>("invitation", { url, code });
  },

  /** Redeem it. The password is passed to Rust and no further: the keys are
   *  generated there, and only wrapped material reaches the server. */
  /** Whether a server takes open sign-ups. An old server that never heard of
   *  the question answers no, which is the right answer. */
  serverAllowsSignup: (url: string): Promise<boolean> =>
    cmd<boolean>("server_allows_signup", { url }),

  /** Make an account with no invitation, on a server that allows it. Creates
   *  an organisation with you as its owner. */
  signup: (url: string, email: string, password: string, orgName: string): Promise<Me> =>
    cmd<Me>("signup", { url, email, password, orgName }),

  async enrol(url: string, code: string, password: string, createsOrg: boolean): Promise<Me> {
    if (!isDesktop) {
      return Promise.reject(
        new ApiError(0, "Enrolment generates keys in Rust and is desktop-only."),
      );
    }
    return cmd<Me>("enrol", { url, code, password, createsOrg });
  },

  /** A sign-in comes back as an identity, or as a challenge when the
   *  organisation wants a code first — then `loginTotp` finishes it. */
  async login(email: string, password: string): Promise<LoginOutcome> {
    if (isDesktop) return cmd<LoginOutcome>("login", { email, password });

    const res = await fetch("/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, password, machine_name: navigator.platform }),
    });
    if (!res.ok) throw new ApiError(res.status, "Wrong email or password.");
    const body = await res.json();
    if (body.second_factor === "totp") return { me: null, challenge: body.challenge, must_enrol_totp: false };
    localStorage.setItem(TOKEN_KEY, body.token);
    return { me: await http<Me>("/v1/me"), challenge: null, must_enrol_totp: !!body.must_enrol_totp };
  },
  loginTotp: (email: string, password: string, challenge: string, code: string): Promise<LoginOutcome> =>
    cmd<LoginOutcome>("login_totp", { email, password, challenge, code }),

  // ---- team security -----------------------------------------------------

  totpStatus: (): Promise<{ enabled: boolean; enabled_at: string | null; pending: boolean; required_by_org: string }> =>
    cmd("totp_status"),
  totpSetup: (): Promise<{ uri: string; secret: string }> => cmd("totp_setup"),
  totpConfirm: (code: string): Promise<{ enabled: boolean }> => cmd("totp_confirm", { code }),
  /** Marks this session as recently verified; see stepUp.ts. */
  totpVerify: (code: string): Promise<{ verified: boolean; minutes: number }> => cmd("totp_verify", { code }),
  totpDisable: (code: string): Promise<{ enabled: boolean }> => cmd("totp_disable", { code }),
  orgSecurity: (): Promise<{ policy: SecurityPolicy; members: { user_id: string; email: string; totp_enabled_at: string | null }[] }> =>
    cmd("org_security"),
  setOrgSecurity: (policy: SecurityPolicy): Promise<{ policy: SecurityPolicy }> => cmd("set_org_security", { policy }),
  loginEvents: (before?: number | null, outcome?: string | null, email?: string | null): Promise<LoginEvent[]> =>
    cmd("login_events", { before: before ?? null, outcome: outcome ?? null, email: email ?? null }),
  sessions: (all: boolean): Promise<SessionRow[]> => cmd("sessions", { all }),
  revokeSession: (id: string): Promise<{ revoked: number }> => cmd("revoke_session", { id }),
  revokeMemberSessions: (userId: string): Promise<{ revoked: number }> => cmd("revoke_member_sessions", { userId }),

  async logout(): Promise<void> {
    if (isDesktop) return cmd<void>("logout");
    // Revoke first, forget second. Clearing the token up front would send the
    // request unauthenticated, leaving a live session on the server — the
    // opposite of what signing out is for. The local token is dropped even if
    // the call fails, so a user on a dead network still ends up logged out.
    try {
      await http("/v1/auth/logout", { method: "POST" });
    } catch {
      // Already invalid, or unreachable. Either way, nothing left to do.
    } finally {
      localStorage.removeItem(TOKEN_KEY);
    }
  },

  me: (): Promise<Me> => (isDesktop ? cmd<Me>("me") : http<Me>("/v1/me")),

  projects: (): Promise<Project[]> =>
    isDesktop ? cmd<Project[]>("projects") : http<Project[]>("/v1/projects"),

  createProjectIn: (name: string, origin: Origin): Promise<{ id: string }> =>
    cmd<{ id: string }>("create_project", { name, origin }),

  /** Every profile when `projectId` is omitted — which is the Profiles view,
   *  and the ordinary case. Passing one narrows it. */
  profiles: (projectId?: string, origin?: Origin): Promise<Profile[]> =>
    isDesktop
      ? cmd<Profile[]>("profiles", { projectId: projectId ?? null, origin: origin ?? null })
      : projectId
        ? http<Profile[]>(`/v1/projects/${projectId}/profiles`)
        : Promise.resolve([]),

  // ---- the team (server only) ------------------------------------------

  /** Who did what, newest first. Owners and admins only; the server refuses
   *  everyone else, so a Member never sees this screen offered. */
  audit: (before?: number, filters?: { action?: string; actor?: string; since?: string; until?: string }): Promise<
    {
      id: number;
      actor: string;
      action: string;
      target_id: string | null;
      detail: unknown;
      at: string;
    }[]
  > => cmd("audit", {
    before: before ?? null,
    action: filters?.action || null,
    actor: filters?.actor || null,
    since: filters?.since || null,
    until: filters?.until || null,
  }),

  orgMembers: (): Promise<{
    members: {
      user_id: string;
      email: string;
      role: string;
      public_key: string;
      has_key: boolean;
      joined_at: string;
      is_you: boolean;
    }[];
    invited: { email: string; role: string; expires_at: string }[];
  }> => cmd("org_members"),

  setRememberOrgKey: (remember: boolean): Promise<Shell> =>
    cmd<Shell>("set_remember_org_key", { remember }),

  invite: (email: string, role: string): Promise<{ code: string; expires_in_hours: number }> =>
    cmd("invite", { email, role }),

  /** Seal the organisation key to a member's public key. Happens in Rust; the
   *  key itself never reaches this side. */
  handOverKey: (userId: string, publicKey: string): Promise<unknown> =>
    cmd("hand_over_key", { userId, publicKey }),

  grants: (projectId: string): Promise<{
    granted: { user_id: string; email: string; role: string; permissions: Perm[]; domain_lists?: string[] }[];
    implicit: { user_id: string; email: string; role: string }[];
  }> => cmd("grants", { projectId }),

  /** `domainLists` undefined keeps what the grant already carries. */
  grantAccess: (projectId: string, userId: string, permissions: Perm[], domainLists?: string[]): Promise<unknown> =>
    cmd("grant_access", { projectId, userId, permissions, domainLists: domainLists ?? null }),

  /** The organisation's domain lists — readable by every member, written by
   *  owners and admins, attached to grants. */
  orgDomainLists: (): Promise<OrgDomainList[]> => cmd<OrgDomainList[]>("org_domain_lists"),
  saveOrgDomainList: (id: string | null, name: string, body: string): Promise<{ id: string; domains: number }> =>
    cmd("save_org_domain_list", { id, name, body }),
  deleteOrgDomainList: (id: string): Promise<unknown> => cmd("delete_org_domain_list", { id }),

  revokeAccess: (projectId: string, userId: string): Promise<unknown> =>
    cmd("revoke_access", { projectId, userId }),

  /** Remove a member and replace the organisation key. Pass null to rotate
   *  without removing anyone — after a lost laptop, say. */
  removeMember: (userId: string | null): Promise<{ generation: number }> =>
    cmd("remove_member", { userId }),

  /** Ask the release feed whether there is a newer build. It never installs:
   *  see src-tauri/update.rs for why that waits on signed releases. */
  checkUpdate: (): Promise<{
    current: string;
    latest: string | null;
    url: string | null;
    notes: string | null;
    status: "current" | "available" | "unpublished" | "unreachable";
    message: string | null;
  }> =>
    isDesktop
      ? cmd("check_update")
      : Promise.resolve({
          current: "dev",
          latest: null,
          url: null,
          notes: null,
          status: "unreachable" as const,
          message: "Update checks are desktop-only.",
        }),

  /** Move profiles into a project, or out of every project with `null`. */
  moveProfiles: (
    ids: string[],
    projectId: string | null,
    origin?: Origin,
  ): Promise<{ moved: number }> =>
    isDesktop
      ? cmd<{ moved: number }>("move_profiles", { ids, projectId, origin: origin ?? null })
      : Promise.reject(new ApiError(0, "Moving profiles is desktop-only for now.")),

  launch: (profileId: string, force = false, origin?: Origin): Promise<LaunchResult> =>
    isDesktop
      ? cmd<LaunchResult>("launch", { profileId, force, origin: origin ?? null })
      : http<LaunchResult>(`/v1/profiles/${profileId}/lock`, {
          method: "POST",
          body: JSON.stringify({
            machine_id: devMachineId(),
            machine_name: navigator.platform,
            force,
          }),
        }).then((r) => ({ ...r, launched: false })),

  stop: (profileId: string, origin?: Origin): Promise<unknown> =>
    isDesktop
      ? cmd<unknown>("stop", { profileId, origin: origin ?? null })
      : http(`/v1/profiles/${profileId}/unlock`, { method: "POST" }),

  // Local mode only: with a server, profiles and proxies are edited where the
  // permissions live, and that screen does not exist yet.
  disconnectServer: (): Promise<Shell> => cmd<Shell>("disconnect_server"),
  saveServerKit: (dir: string): Promise<{ path: string; files: number }> =>
    cmd<{ path: string; files: number }>("save_server_kit", { dir }),
  personas: (): Promise<Persona[]> => cmd<Persona[]>("personas"),
  preview: (spec: {
    persona_id: string;
    fp_seed: number;
    /** Null follows the exit; the caller passes what it knows of it so the
     *  panel shows what the launch will claim, not what the field contains. */
    timezone: string | null;
    languages: string[] | null;
  }): Promise<Preview> => cmd<Preview>("preview", { spec }),
  proxies: (): Promise<LocalProxy[]> => cmd<LocalProxy[]>("proxies"),

  /** Logins stored beside a profile. Local mode only for now — see commands.rs. */
  credentials: (profileId: string): Promise<Credential[]> =>
    cmd<Credential[]>("credentials", { profileId }),
  saveCredential: (credential: Credential): Promise<{ id: string }> =>
    cmd<{ id: string }>("save_credential", { credential }),
  deleteCredential: (id: string, profileId: string): Promise<unknown> =>
    cmd("delete_credential", { id, profileId }),
  /** Six digits and how long they last. The seed stays in the agent. */
  /** Ask the agent to fetch and install the browser. Returns once the download
   *  has started; watch shell.core_download for the rest. */
  downloadCore: (): Promise<void> => cmd<void>("download_core"),

  /** Open a link in the operator's own browser. An <a target="_blank"> inside
   *  the application window has nowhere to go — the window is not a browser —
   *  so a link that looks like one does nothing until it goes through here. */
  openUrl: (url: string): Promise<void> => cmd<void>("open_url", { url }),

  totpCode: (profileId: string, id: string): Promise<TotpCode> =>
    cmd<TotpCode>("totp_code", { profileId, id }),

  saveProxy: (proxy: Partial<LocalProxy>): Promise<{ id: string }> =>
    cmd<{ id: string }>("save_proxy", { proxy }),
  deleteProxy: (id: string): Promise<unknown> => cmd("delete_proxy", { id }),
  checkProxy: (url: string, checkerUrl?: string | null, proxyId?: string | null): Promise<{
    ok: boolean; error?: string; ip?: string; country?: string;
    city?: string; timezone?: string; org?: string; ms?: number;
  }> => cmd("check_proxy", { url, checkerUrl: checkerUrl || null, proxyId: proxyId || null }),
  /** The check step by step; the first failing step carries a `code` the
   *  interface translates. See agent/src/diagnose.rs. */
  diagnoseProxy: (url: string, checkerUrl?: string | null, proxyId?: string | null): Promise<Diagnosis> =>
    cmd<Diagnosis>("diagnose_proxy", { url, checkerUrl: checkerUrl || null, proxyId: proxyId || null }),
  rotateProxy: (id: string): Promise<{ ok: boolean; error?: string }> =>
    cmd("rotate_proxy", { id }),
  // ---- giving one profile to one person --------------------------------

  /** Seals the profile key to the recipient and posts it. All of that happens
   *  in Rust: the key is derived from the organisation key, which the webview
   *  never sees and must not. */
  shareProfile: (
    profileId: string,
    email: string,
    permissions: string[],
    expiresAt?: string | null,
  ): Promise<{ ok: boolean }> =>
    cmd("share_profile", { profileId, email, permissions, expiresAt: expiresAt ?? null }),

  profileShares: (
    profileId: string,
  ): Promise<{ user_id: string; email: string; permissions: number; expires_at: string | null }[]> =>
    cmd("profile_shares", { profileId }),

  revokeShare: (profileId: string, userId: string): Promise<unknown> =>
    cmd("revoke_share", { profileId, userId }),

  /** Copy a local profile onto the server, browser data and all.
   *
   *  The local original stays where it is. `proxy_moved` comes back true when an
   *  exit went with it: it is re-sealed under the organisation key, joins the
   *  team's proxy list, and becomes usable by anyone the profile is shared
   *  with. */
  uploadProfile: (
    id: string,
    projectId: string,
  ): Promise<{ id: string; bytes: number; proxy_moved: boolean }> =>
    cmd("upload_profile", { id, projectId }),

  /** What other people have given to this account. */
  sharedWithMe: (): Promise<Profile[]> => cmd<Profile[]>("shared_with_me"),

  saveProfile: (profile: unknown, origin?: Origin): Promise<{ id: string }> =>
    cmd<{ id: string }>("save_profile", { profile, origin: origin ?? null }),
  deleteProfile: (id: string, origin?: Origin): Promise<unknown> =>
    cmd("delete_profile", { id, origin: origin ?? null }),

  /** Many profiles from one template. Each gets its own seed and its own
   *  persona — see the agent for why that is the whole safety property. */
  createProfiles: (
    count: number,
    namePattern: string,
    template: unknown,
  ): Promise<{ created: unknown[]; failed: { n: number; error: string }[] }> =>
    cmd("create_profiles", { count, namePattern, template }),
  /** Saved answers to the batch dialog, by name (docs/16 5.6). */
  templates: (): Promise<ProfileTemplate[]> => cmd("templates"),
  saveTemplate: (template: ProfileTemplate): Promise<{ saved: string }> => cmd("save_template", { template }),
  deleteTemplate: (name: string): Promise<unknown> => cmd("delete_template", { name }),

  /** Copies the setup, never the identity: same persona and proxy, a fresh
   *  seed, no browser data. */
  /** The copy is made where the profile lives — the agent locally, the server
   *  in team mode — because only they hold the fields the listing omits. */
  cloneProfile: (
    id: string,
    count: number,
    name?: string,
    origin?: Origin,
  ): Promise<{ created: unknown[]; failed?: { n: number; error: string }[] }> =>
    cmd("clone_profile", { id, count, name: name || null, origin: origin ?? null }),

  /** One line as the block importer would read it, or null if it is not one. */
  parseProxyLine: (line: string): Promise<ParsedProxyLine | null> =>
    cmd("parse_proxy_line", { line }),

  /** A pasted supplier block. Every line is reported back with its number. */
  importProxies: (
    text: string,
    namePrefix: string,
  ): Promise<{
    saved: { id: string; line: number; host: string; port: number; shape: string }[];
    rejected: { line: number; error: string; code?: string }[];
  }> => cmd("import_proxies", { text, namePrefix }),

  exportCookies: (id: string): Promise<{ cookies: unknown[] }> =>
    cmd("export_cookies", { id }),
  importCookies: (
    id: string,
    cookies: unknown[],
  ): Promise<{ imported: number; session_only: number; skipped: number }> =>
    cmd("import_cookies", { id, cookies }),
  // ---- synchronised windows ---------------------------------------------

  /** Start mirroring between these profiles. Ones not open are launched with
   *  the debugging port; ones open without it are refused by id. */
  mirrorStart: (profileIds: string[], typing: boolean): Promise<{
    joined: string[]; refused: { id: string; reason: string }[]; status: MirrorStatus;
  }> => cmd("mirror_start", { profileIds, typing }),
  mirrorStop: (): Promise<unknown> => cmd("mirror_stop"),
  mirrorStatus: (): Promise<MirrorStatus> => cmd<MirrorStatus>("mirror_status"),
  mirrorTyping: (on: boolean): Promise<unknown> => cmd("mirror_typing", { on }),

  // ---- warming ------------------------------------------------------------

  warmStart: (profileIds: string[], plan: WarmPlan): Promise<{ started: string[]; refused: { id: string; reason: string }[] }> =>
    cmd("warm_start", { profileIds, plan }),
  warmStatus: (): Promise<WarmProgress[]> => cmd<WarmProgress[]>("warm_status"),
  warmStop: (id: string): Promise<unknown> => cmd("warm_stop", { id }),
  warmClear: (): Promise<unknown> => cmd("warm_clear"),
  warmDefaults: (): Promise<{ urls: string[] }> => cmd("warm_defaults"),

  // ---- this machine as a persona ----------------------------------------

  /** Runs the installed Chrome at the probe and converts the dump. ~10 s;
   *  a Chrome window opens and closes. Nothing is sent anywhere. */
  capturePersona: (): Promise<{ persona: Record<string, unknown> & { id: string }; problems: string[]; browser: string }> =>
    cmd("capture_persona"),
  /** Writes it to ~/Downloads/fury-persona-<id>.json; returns the path. */
  savePersonaFile: (persona: unknown): Promise<string> => cmd<string>("save_persona_file", { persona }),

  // ---- domain lists ------------------------------------------------------

  blocklists: (): Promise<DomainList[]> => cmd<DomainList[]>("blocklists"),
  readBlocklist: (name: string): Promise<{ name: string; text: string }> =>
    cmd("read_blocklist", { name }),
  saveBlocklist: (name: string, text: string): Promise<DomainList> =>
    cmd<DomainList>("save_blocklist", { name, text }),
  deleteBlocklist: (name: string): Promise<unknown> => cmd("delete_blocklist", { name }),

  // ---- extensions and disk usage ----------------------------------------

  /** Installed into one profile. Identity survives cloning — see ext.rs. */
  extensions: (profileId: string): Promise<Extension[]> =>
    cmd<Extension[]>("extensions", { profileId }),
  /** The .crx as base64: the file was chosen in the webview, so bytes are what
   *  we have. Installing the same extension twice replaces it. */
  installExtension: (profileId: string, crxB64: string): Promise<Extension> =>
    cmd<Extension>("install_extension", { profileId, crxB64 }),
  /** Every extension on this machine, grouped by id, with the profiles it is in. */
  allExtensions: (): Promise<ExtensionEverywhere[]> => cmd<ExtensionEverywhere[]>("all_extensions"),
  /** One .crx into many profiles. Open profiles are skipped and named. */
  installExtensionMany: (profileIds: string[], crxB64: string): Promise<{
    extension: Extension | null; installed: string[]; skipped: { id: string; reason: string }[];
  }> => cmd("install_extension_many", { profileIds, crxB64 }),
  removeExtension: (profileId: string, id: string): Promise<unknown> =>
    cmd("remove_extension", { profileId, id }),
  extensionCatalogue: (): Promise<CatalogueEntry[]> => cmd("extension_catalogue"),
  /** By Web Store id, fetched through each profile's own proxy (docs/12, B). */
  installExtensionFromStore: (profileIds: string[], extId: string): Promise<StoreInstallResult> =>
    cmd("install_extension_from_store", { profileIds, extId }),

  /** A fresh fingerprint seed. Refused while the profile is open. Everything a
   *  site tied to the old fingerprint stops matching — which is the point, and
   *  the reason the interface confirms first. */
  reseedProfile: (id: string, origin?: Origin): Promise<{ id: string; fp_seed: number }> =>
    cmd("reseed_profile", { id, origin: origin ?? null }),

  /** How big a profile is on disk and how much of it is cache. */
  profileUsage: (id: string): Promise<Usage> => cmd<Usage>("profile_usage", { id }),
  /** Removes caches only. The agent refuses while the profile is open. */
  trimProfile: (id: string): Promise<{ bytes: number; removed: string[] }> =>
    cmd("trim_profile", { id }),

  exportProject: (id: string, path: string, passphrase: string, withData = true):
    Promise<{ path: string; bytes: number }> =>
    cmd("export_project", { id, path, passphrase, withData }),
  importProject: (path: string, passphrase: string):
    Promise<{ project_id: string; profiles: number }> =>
    cmd("import_project", { path, passphrase }),
  trash: (): Promise<Profile[]> => cmd<Profile[]>("trash"),
  /** Both take the row's origin: the trash holds this machine's deletions and
   *  the server's side by side, and restoring is a different call in each. */
  restoreProfile: (id: string, origin?: Origin): Promise<unknown> =>
    cmd("restore_profile", { id, origin: origin ?? null }),
  purgeProfile: (id: string, origin?: Origin): Promise<unknown> =>
    cmd("purge_profile", { id, origin: origin ?? null }),
  renameProject: (id: string, name: string, origin?: Origin): Promise<unknown> =>
    cmd("rename_project", { id, name, origin: origin ?? null }),
  deleteProject: (id: string, origin?: Origin): Promise<unknown> =>
    cmd("delete_project", { id, origin: origin ?? null }),
  createProject: (name: string): Promise<{ id: string }> =>
    cmd<{ id: string }>("create_project", { name }),
};
