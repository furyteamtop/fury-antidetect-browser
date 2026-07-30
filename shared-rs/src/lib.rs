//! Types shared between `fury-server` and `fury-agent`.
//!
//! Everything here is transport-agnostic: no axum, no sqlx, no reqwest. Both
//! sides depend on this crate so the wire format cannot drift.

pub mod api;
pub mod fingerprint;
pub mod persona;
pub mod rbac;

pub use fingerprint::FingerprintConfig;
pub use persona::{Persona, ProfileContext};
pub use rbac::{effective, LaunchRestrictions, OrgRole, Perm, PermSet};

/// Bumped when the fingerprint config schema changes in a way the core must
/// know about. The core refuses to start on an unknown version rather than
/// silently ignoring fields it does not understand.
pub const FINGERPRINT_SCHEMA_VERSION: u32 = 1;
