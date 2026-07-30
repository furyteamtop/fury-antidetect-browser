//! Permission model.
//!
//! Permissions are a bitmask stored in `project_grants.permissions` and
//! `profile_grants.permissions`. The bit values are duplicated in
//! `server/migrations/0001_init.sql` — keep both in sync.
//!
//! The organisation role sets a *ceiling*. The grant sets the actual set.
//! Effective permissions = role ceiling ∩ grant.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrgRole {
    Owner,
    Admin,
    Manager,
    Member,
}

/// A single capability. Values are stable — never renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Perm {
    /// See the project and its profiles.
    View = 1,
    /// Launch a profile.
    Launch = 2,
    /// Rename, retag, edit notes and start options.
    EditProfile = 4,
    /// Change persona, seed, fingerprint overrides.
    EditFingerprint = 8,
    /// Attach or detach a proxy.
    EditProxy = 16,
    /// See passwords, TOTP secrets and proxy credentials in the clear.
    RevealSecrets = 32,
    /// Export cookies or the whole bundle. Also gates CDP access, because
    /// anyone with CDP can read every cookie anyway.
    ExportCookies = 64,
    CreateProfile = 128,
    DeleteProfile = 256,
    /// Grant access to others, and force-unlock a stuck profile.
    ManageAccess = 512,
}

impl Perm {
    pub const ALL: [Perm; 10] = [
        Perm::View,
        Perm::Launch,
        Perm::EditProfile,
        Perm::EditFingerprint,
        Perm::EditProxy,
        Perm::RevealSecrets,
        Perm::ExportCookies,
        Perm::CreateProfile,
        Perm::DeleteProfile,
        Perm::ManageAccess,
    ];
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PermSet(pub i64);

impl PermSet {
    pub const NONE: PermSet = PermSet(0);

    /// Everything. Only ever produced from an owner/admin role ceiling.
    pub fn all() -> Self {
        PermSet(Perm::ALL.iter().fold(0, |acc, p| acc | *p as i64))
    }

    /// The set every anti-detect team hands its operators: open the profile,
    /// work in the logged-in account, take nothing home.
    pub fn operator() -> Self {
        Self::from_iter([Perm::View, Perm::Launch])
    }

    /// A profile owner who manages their own accounts end to end.
    pub fn full_profile_work() -> Self {
        Self::from_iter([
            Perm::View,
            Perm::Launch,
            Perm::EditProfile,
            Perm::EditFingerprint,
            Perm::EditProxy,
            Perm::RevealSecrets,
            Perm::ExportCookies,
            Perm::CreateProfile,
        ])
    }

    pub fn from_iter<I: IntoIterator<Item = Perm>>(perms: I) -> Self {
        PermSet(perms.into_iter().fold(0, |acc, p| acc | p as i64))
    }

    pub fn has(self, p: Perm) -> bool {
        self.0 & (p as i64) != 0
    }

    pub fn with(self, p: Perm) -> Self {
        PermSet(self.0 | p as i64)
    }

    pub fn without(self, p: Perm) -> Self {
        PermSet(self.0 & !(p as i64))
    }

    pub fn intersect(self, other: PermSet) -> Self {
        PermSet(self.0 & other.0)
    }

    pub fn to_vec(self) -> Vec<Perm> {
        Perm::ALL.iter().copied().filter(|p| self.has(*p)).collect()
    }
}

/// The maximum a role can ever hold, regardless of what a grant says.
///
/// The one hard rule: a plain `Member` can never be handed `ManageAccess`.
/// Otherwise a manager could quietly delegate access control downwards and
/// the org owner would lose the ability to reason about who can reach what.
pub fn role_ceiling(role: OrgRole) -> PermSet {
    match role {
        OrgRole::Owner | OrgRole::Admin | OrgRole::Manager => PermSet::all(),
        OrgRole::Member => PermSet::all().without(Perm::ManageAccess),
    }
}

/// Owners and admins reach every project in the org without an explicit grant.
pub fn has_implicit_project_access(role: OrgRole) -> bool {
    matches!(role, OrgRole::Owner | OrgRole::Admin)
}

/// Effective permissions for a user on a project.
///
/// `grant` is `None` when no explicit grant row exists.
pub fn effective(role: OrgRole, grant: Option<PermSet>) -> PermSet {
    if has_implicit_project_access(role) {
        return PermSet::all();
    }
    match grant {
        Some(g) => g.intersect(role_ceiling(role)),
        None => PermSet::NONE,
    }
}

/// Hardening the agent must apply when launching for this permission set.
///
/// This is the technical half of the "operator without secrets" model: the UI
/// hiding a button is not a control, these flags are.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LaunchRestrictions {
    /// Refuse to hand out the CDP WebSocket endpoint over the local API.
    pub deny_cdp: bool,
    /// Pass `--fury-lock-devtools` to the core.
    pub lock_devtools: bool,
    /// Block chrome://settings/passwords, bookmark export, profile export.
    pub lock_data_export: bool,
    /// Wipe the unpacked user-data-dir on exit instead of keeping a local cache.
    pub wipe_on_exit: bool,
    /// Fetch credentials via one-shot autofill tokens rather than handing the
    /// encrypted blob to the client.
    pub autofill_only: bool,
}

impl LaunchRestrictions {
    pub fn for_perms(p: PermSet) -> Self {
        let can_export = p.has(Perm::ExportCookies);
        let can_reveal = p.has(Perm::RevealSecrets);
        LaunchRestrictions {
            deny_cdp: !can_export,
            lock_devtools: !can_export,
            lock_data_export: !can_export,
            wipe_on_exit: !can_export,
            autofill_only: !can_reveal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_cannot_take_data_home() {
        let p = effective(OrgRole::Member, Some(PermSet::operator()));
        assert!(p.has(Perm::Launch));
        assert!(!p.has(Perm::RevealSecrets));
        assert!(!p.has(Perm::ExportCookies));

        let r = LaunchRestrictions::for_perms(p);
        assert!(r.deny_cdp, "CDP access equals full cookie access");
        assert!(r.lock_devtools);
        assert!(r.wipe_on_exit);
        assert!(r.autofill_only);
    }

    #[test]
    fn member_without_grant_sees_nothing() {
        assert_eq!(effective(OrgRole::Member, None), PermSet::NONE);
    }

    #[test]
    fn admin_bypasses_grants() {
        assert_eq!(effective(OrgRole::Admin, None), PermSet::all());
    }

    #[test]
    fn grant_cannot_exceed_role_ceiling() {
        let p = effective(OrgRole::Member, Some(PermSet::all()));
        assert_eq!(p, PermSet::all().intersect(role_ceiling(OrgRole::Member)));
    }
}
