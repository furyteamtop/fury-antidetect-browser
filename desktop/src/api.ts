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
  project_id: string;
  name: string;
  tags: string[];
  persona_id: string;
  proxy: ProxySummary | null;
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
    throw new ApiError(status, message, body);
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

  profiles: (projectId: string): Promise<Profile[]> =>
    isDesktop
      ? cmd<Profile[]>("profiles", { projectId })
      : http<Profile[]>(`/v1/projects/${projectId}/profiles`),

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
  proxies: (): Promise<LocalProxy[]> => cmd<LocalProxy[]>("proxies"),
  saveProxy: (proxy: Partial<LocalProxy>): Promise<{ id: string }> =>
    cmd<{ id: string }>("save_proxy", { proxy }),
  deleteProxy: (id: string): Promise<unknown> => cmd("delete_proxy", { id }),
  saveProfile: (profile: unknown): Promise<{ id: string }> =>
    cmd<{ id: string }>("save_profile", { profile }),
  deleteProfile: (id: string): Promise<unknown> => cmd("delete_profile", { id }),
  createProject: (name: string): Promise<{ id: string }> =>
    cmd<{ id: string }>("create_project", { name }),
};
