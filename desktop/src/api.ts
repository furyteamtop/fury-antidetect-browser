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
}

export interface LockInfo {
  user_id: string;
  user_email: string;
  machine_name: string;
  acquired_at: string;
  expires_at: string;
}

export interface Profile {
  id: string;
  /** Null when the profile is in no project. Profiles is the master list —
   *  every profile on this machine — and a project is a grouping a profile can
   *  be put into or taken out of without ever being at risk. */
  project_id: string | null;
  /** Where it is filed, so the flat list can show it without a call per row. */
  project_name: string | null;
  name: string;
  tags: string[];
  persona_id: string;
  /** Zero in team mode — the server never exposes a seed. */
  fp_seed: number;
  proxy: ProxySummary | null;
  /** Null means "follow the proxy's exit", resolved at launch. */
  timezone: string | null;
  languages: string[] | null;
  lock: LockInfo | null;
  permissions: Perm[];
  /** Local mode only: the agent knows what it launched. In team mode a
   *  colleague's browser shows up through `lock`, not here. */
  running: boolean;
  last_opened_at: string | null;
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
  fonts: number;
  noise: { canvas: boolean; audio: boolean; client_rects: boolean };
  /** Contradictions that make this device impossible. Non-empty blocks saving:
   *  an inconsistent profile stands out more than an un-spoofed one. */
  problems: string[];
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
  /** Whether the organisation key is remembered between launches. */
  remember_org_key: boolean;
  /** This build, for the About panel and for any bug report that follows. */
  version: string;
  /** Whether this process holds the organisation key. A session survives a
   *  restart; the key deliberately does not. So "signed in" and "able to
   *  decrypt" are different states, and both have to be visible. */
  org_key_ready: boolean;
  last_email: string | null;
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
      remember_org_key: true,
      version: "dev",
      org_key_ready: false,
      last_email: null,
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
  async enrol(url: string, code: string, password: string, createsOrg: boolean): Promise<Me> {
    if (!isDesktop) {
      return Promise.reject(
        new ApiError(0, "Enrolment generates keys in Rust and is desktop-only."),
      );
    }
    return cmd<Me>("enrol", { url, code, password, createsOrg });
  },

  async login(email: string, password: string): Promise<Me> {
    if (isDesktop) return cmd<Me>("login", { email, password });

    const res = await fetch("/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, password, machine_name: navigator.platform }),
    });
    if (!res.ok) throw new ApiError(res.status, "Wrong email or password.");
    const { token } = await res.json();
    localStorage.setItem(TOKEN_KEY, token);
    return http<Me>("/v1/me");
  },

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

  /** Every profile when `projectId` is omitted — which is the Profiles view,
   *  and the ordinary case. Passing one narrows it. */
  profiles: (projectId?: string): Promise<Profile[]> =>
    isDesktop
      ? cmd<Profile[]>("profiles", { projectId: projectId ?? null })
      : projectId
        ? http<Profile[]>(`/v1/projects/${projectId}/profiles`)
        : Promise.resolve([]),

  // ---- the team (server only) ------------------------------------------

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
    granted: { user_id: string; email: string; role: string; permissions: Perm[] }[];
    implicit: { user_id: string; email: string; role: string }[];
  }> => cmd("grants", { projectId }),

  grantAccess: (projectId: string, userId: string, permissions: Perm[]): Promise<unknown> =>
    cmd("grant_access", { projectId, userId, permissions }),

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
  moveProfiles: (ids: string[], projectId: string | null): Promise<{ moved: number }> =>
    isDesktop
      ? cmd<{ moved: number }>("move_profiles", { ids, projectId })
      : Promise.reject(new ApiError(0, "Moving profiles is desktop-only for now.")),

  launch: (profileId: string, force = false): Promise<LaunchResult> =>
    isDesktop
      ? cmd<LaunchResult>("launch", { profileId, force })
      : http<LaunchResult>(`/v1/profiles/${profileId}/lock`, {
          method: "POST",
          body: JSON.stringify({
            machine_id: devMachineId(),
            machine_name: navigator.platform,
            force,
          }),
        }).then((r) => ({ ...r, launched: false })),

  stop: (profileId: string): Promise<unknown> =>
    isDesktop
      ? cmd<unknown>("stop", { profileId })
      : http(`/v1/profiles/${profileId}/unlock`, { method: "POST" }),

  // Local mode only: with a server, profiles and proxies are edited where the
  // permissions live, and that screen does not exist yet.
  disconnectServer: (): Promise<Shell> => cmd<Shell>("disconnect_server"),
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
  saveProxy: (proxy: Partial<LocalProxy>): Promise<{ id: string }> =>
    cmd<{ id: string }>("save_proxy", { proxy }),
  deleteProxy: (id: string): Promise<unknown> => cmd("delete_proxy", { id }),
  checkProxy: (url: string, checkerUrl?: string | null, proxyId?: string | null): Promise<{
    ok: boolean; error?: string; ip?: string; country?: string;
    city?: string; timezone?: string; org?: string; ms?: number;
  }> => cmd("check_proxy", { url, checkerUrl: checkerUrl || null, proxyId: proxyId || null }),
  rotateProxy: (id: string): Promise<{ ok: boolean; error?: string }> =>
    cmd("rotate_proxy", { id }),
  saveProfile: (profile: unknown): Promise<{ id: string }> =>
    cmd<{ id: string }>("save_profile", { profile }),
  deleteProfile: (id: string): Promise<unknown> => cmd("delete_profile", { id }),

  /** Many profiles from one template. Each gets its own seed and its own
   *  persona — see the agent for why that is the whole safety property. */
  createProfiles: (
    count: number,
    namePattern: string,
    template: unknown,
  ): Promise<{ created: unknown[]; failed: { n: number; error: string }[] }> =>
    cmd("create_profiles", { count, namePattern, template }),

  /** Copies the setup, never the identity: same persona and proxy, a fresh
   *  seed, no browser data. */
  /** The copy is made where the profile lives — the agent locally, the server
   *  in team mode — because only they hold the fields the listing omits. */
  cloneProfile: (
    id: string,
    count: number,
    name?: string,
  ): Promise<{ created: unknown[]; failed?: { n: number; error: string }[] }> =>
    cmd("clone_profile", { id, count, name: name || null }),

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
  exportProject: (id: string, path: string, passphrase: string, withData = true):
    Promise<{ path: string; bytes: number }> =>
    cmd("export_project", { id, path, passphrase, withData }),
  importProject: (path: string, passphrase: string):
    Promise<{ project_id: string; profiles: number }> =>
    cmd("import_project", { path, passphrase }),
  trash: (): Promise<Profile[]> => cmd<Profile[]>("trash"),
  restoreProfile: (id: string): Promise<unknown> => cmd("restore_profile", { id }),
  purgeProfile: (id: string): Promise<unknown> => cmd("purge_profile", { id }),
  renameProject: (id: string, name: string): Promise<unknown> =>
    cmd("rename_project", { id, name }),
  deleteProject: (id: string): Promise<unknown> => cmd("delete_project", { id }),
  createProject: (name: string): Promise<{ id: string }> =>
    cmd<{ id: string }>("create_project", { name }),
};
