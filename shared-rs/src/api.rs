//! Wire types for the server REST API and the agent local API.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::rbac::{OrgRole, Perm};

// ---------------------------------------------------------------------------
// Projects & profiles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectSummary {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub color: Option<String>,
    pub profile_count: i64,
    /// What the requesting user may do here. Resolved server-side; the client
    /// must not compute this itself.
    pub permissions: Vec<Perm>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub id: Uuid,
    pub project_id: Uuid,
    /// So the flat list can say where a profile is filed without a second
    /// request per row.
    #[serde(default)]
    pub project_name: String,
    pub name: String,
    pub tags: Vec<String>,
    pub persona_id: String,
    pub proxy: Option<ProxySummary>,
    pub current_version: i32,
    pub lock: Option<LockInfo>,
    pub permissions: Vec<Perm>,
}

/// Proxy as shown to a user *without* `reveal_secrets`: enough to tell profiles
/// apart, nothing that could be reused elsewhere.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxySummary {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    /// Masked host, e.g. "res-\*\*\*.provider.net:8000".
    pub display: String,
    pub country: Option<String>,
    pub detected_kind: Option<String>,
    pub last_checked_at: Option<String>,
    /// How many other profiles share this exact proxy. Shown in the UI because
    /// several accounts behind one IP is a classic farm signal.
    pub shared_with_profiles: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    pub user_id: Uuid,
    pub user_email: String,
    pub machine_name: String,
    pub acquired_at: String,
    pub expires_at: String,
}

// ---------------------------------------------------------------------------
// Locking
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcquireLockRequest {
    pub machine_id: String,
    pub machine_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcquireLockResponse {
    /// Opaque token. Required to upload a bundle, which is what prevents a
    /// force-unlocked holder from overwriting newer state.
    pub lock_token: String,
    pub expires_at: String,
    /// How the agent must harden this launch. Computed server-side from the
    /// caller's permissions — these flags are the technical half of "an
    /// operator cannot take the data home", so they are never trusted from the
    /// client.
    pub restrictions: crate::rbac::LaunchRestrictions,
    /// Everything needed to actually start the browser.
    ///
    /// Folded into the lock rather than a separate `GET /launch-spec`, for two
    /// reasons. One round trip instead of two, and — the real one — every
    /// handout of proxy ciphertext is then bound to a lock, and therefore
    /// already written to the audit log as `profile.launch`. A separate
    /// endpoint would be a way to collect the organisation's proxy credentials
    /// without ever appearing to open anything.
    pub spec: LaunchSpec,
}

/// What the agent needs to run one profile, and nothing else.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchSpec {
    pub profile_id: Uuid,
    pub name: String,
    pub persona_id: String,
    /// Sixteen lowercase hex characters — see `persona::seed` for why this is
    /// pinned rather than left to each side's own integer type.
    pub fp_seed: String,
    /// `None` means "follow the exit" — the profile has `auto_timezone` set and
    /// the agent resolves the zone from the address the proxy actually leaves
    /// through.
    ///
    /// It has to be expressible. Before this was an `Option` the server refused
    /// to launch a profile with no timezone, so the shell always sent a string,
    /// so the agent's follow-the-exit branch was unreachable for every team
    /// profile — the feature existed and only local profiles could use it. The
    /// `auto_timezone` and `auto_locale` columns have been in the schema since
    /// the first migration and nothing had ever read them.
    pub timezone: Option<String>,
    /// `None` means "follow the exit", as above.
    ///
    /// Not an empty `Vec`: an empty list is a profile that sends no
    /// Accept-Language at all, which is a value no browser produces, and the
    /// two must not collapse into the same wire form.
    pub languages: Option<Vec<String>>,
    pub start_urls: Vec<String>,
    pub proxy: SealedProxy,
}

/// A proxy as the server holds it: addressable, and unreadable.
///
/// The credentials travel encrypted and are opened by the desktop shell, which
/// holds the organisation key. The server never had them and the agent never
/// needs the key that opens them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealedProxy {
    pub id: Uuid,
    /// http | https | socks5
    pub kind: String,
    pub host: String,
    pub port: u16,
    /// `{username, password}` as JSON, sealed under a per-proxy data key.
    #[serde(with = "hex_bytes")]
    pub credentials_enc: Vec<u8>,
    /// That data key, wrapped under the organisation key.
    #[serde(with = "hex_bytes")]
    pub wrapped_dek: Vec<u8>,
}

/// The plaintext behind `credentials_enc`. Exists on the client only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyCredentials {
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LockConflict {
    Held { holder: LockInfo },
}

// ---------------------------------------------------------------------------
// Bundles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleUploadRequest {
    pub lock_token: String,
    /// Version this upload is based on. Rejected if it is not the current one.
    pub base_version: i32,
    pub kind: String,
    pub size_bytes: i64,
    #[serde(with = "hex_bytes")]
    pub sha256: Vec<u8>,
    #[serde(with = "hex_bytes")]
    pub wrapped_dek: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleUploadResponse {
    pub bundle_id: Uuid,
    pub version: i32,
    /// Presigned PUT. The server never proxies bundle bytes.
    pub upload_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleDownloadResponse {
    pub version: i32,
    /// Snapshot first, then deltas in order.
    pub chain: Vec<BundlePart>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundlePart {
    pub version: i32,
    pub kind: String,
    pub download_url: String,
    #[serde(with = "hex_bytes")]
    pub sha256: Vec<u8>,
    #[serde(with = "hex_bytes")]
    pub wrapped_dek: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Agent local API
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartProfileResponse {
    pub pid: u32,
    /// Present only when the caller holds `export_cookies`. Anyone with CDP can
    /// read every cookie, so it is gated by the same permission.
    pub ws_endpoint: Option<String>,
    pub proxy_port: u16,
    pub effective_ip: Option<String>,
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProfileState {
    Stopped,
    Starting,
    Running { pid: u32, since: String },
    Syncing { progress: f32 },
    Failed { reason: String },
}

// ---------------------------------------------------------------------------
// Membership
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberSummary {
    pub user_id: Uuid,
    pub email: String,
    pub role: OrgRole,
    pub joined_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantRequest {
    pub user_id: Uuid,
    pub permissions: Vec<Perm>,
    pub expires_at: Option<String>,
}

// ---------------------------------------------------------------------------

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        unhex(&s).ok_or_else(|| serde::de::Error::custom("invalid hex"))
    }

    fn hex(v: &[u8]) -> String {
        v.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(s: &str) -> Option<Vec<u8>> {
        if s.len() % 2 != 0 {
            return None;
        }
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
            .collect()
    }
}
