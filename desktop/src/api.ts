// The server is the only authority on what a user may do. Nothing here decides
// permissions; it reads the set the server resolved and renders accordingly.

export type Perm =
  | "view" | "launch" | "edit_profile" | "edit_fingerprint" | "edit_proxy"
  | "reveal_secrets" | "export_cookies" | "create_profile" | "delete_profile"
  | "manage_access";

export interface Project {
  id: string;
  name: string;
  description: string;
  color: string | null;
  profile_count: number;
  permissions: Perm[];
}

export interface ProxySummary {
  id: string;
  name: string;
  kind: string;
  /** Already masked by the server when the caller lacks reveal_secrets. */
  display: string;
  country: string | null;
  shared_with_profiles: number;
}

export interface LockInfo {
  user_id: string;
  user_email: string;
  machine_name: string;
  acquired_at: string;
  expires_at: string;
}

export interface Me {
  user_id: string;
  email: string;
  org_id: string;
  role: "owner" | "admin" | "manager" | "member";
}

export interface Profile {
  id: string;
  project_id: string;
  name: string;
  tags: string[];
  persona_id: string;
  proxy: ProxySummary | null;
  current_version: number;
  lock: LockInfo | null;
  permissions: Perm[];
}

const TOKEN_KEY = "fury.token";

export function storedToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
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

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
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
  return `Request failed (${status}).`;
}

export const api = {
  async login(email: string, password: string): Promise<void> {
    const res = await fetch("/v1/auth/login", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ email, password, machine_name: navigator.platform }),
    });
    if (!res.ok) throw new ApiError(res.status, "Wrong email or password.");
    const { token } = await res.json();
    localStorage.setItem(TOKEN_KEY, token);
  },

  async logout(): Promise<void> {
    // Revoke first, forget second. Clearing the token up front would send the
    // request unauthenticated, leaving a live session on the server — the
    // opposite of what signing out is for. The local token is dropped even if
    // the call fails, so a user on a dead network still ends up logged out.
    try {
      await request("/v1/auth/logout", { method: "POST" });
    } catch {
      // Already invalid, or unreachable. Either way, nothing left to do.
    } finally {
      localStorage.removeItem(TOKEN_KEY);
    }
  },

  me: () => request<Me>("/v1/me"),

  projects: () => request<Project[]>("/v1/projects"),

  profiles: (projectId: string) =>
    request<Profile[]>(`/v1/projects/${projectId}/profiles`),

  lock: (profileId: string, force = false) =>
    request<{ lock_token: string; expires_at: string; restrictions: Record<string, boolean> }>(
      `/v1/profiles/${profileId}/lock`,
      {
        method: "POST",
        body: JSON.stringify({
          machine_id: machineId(),
          machine_name: navigator.platform,
          force,
        }),
      },
    ),

  unlock: (profileId: string) =>
    request(`/v1/profiles/${profileId}/unlock`, { method: "POST" }),
};

/** Stable per installation, so the lock list names the same machine each time. */
function machineId(): string {
  const key = "fury.machine_id";
  let id = localStorage.getItem(key);
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem(key, id);
  }
  return id;
}
