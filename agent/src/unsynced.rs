// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! A team profile whose last session never reached the server.
//!
//! Closing a team profile uploads its bundle. When that upload fails, the
//! session -- cookies, saved passwords, history -- exists only on this machine.
//! The next launch then pulled the server's bundle and unpacked it over the
//! profile directory, file by file, and the older copy replaced the newer one.
//! A tester whose team server sat behind Cloudflare, where every upload broke
//! off, reported it as "Chrome offers to save the password and it is not
//! saved", 27.09.2026.
//!
//! So a failed upload leaves a marker beside the profile directory saying which
//! server version the local copy grew from. The next launch compares:
//!
//! - the server still at that version: nobody else has written since, the local
//!   copy is strictly newer, and it is kept. The next close uploads it on top of
//!   that same version, which the server accepts.
//! - the server has moved on: a colleague uploaded in between, and both copies
//!   hold work. The local one is moved aside, not deleted, and the server's is
//!   used. The log says where the local one went.
//!
//! The marker lives beside the directory, not in it, so that it is never packed
//! into a bundle and never unpacked from one.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    /// The server version the unsent session started from.
    base_version: i32,
    /// When the upload failed, for the person reading the file.
    since: String,
}

fn marker_path(profile_id: &str) -> PathBuf {
    crate::paths::profile_dir(profile_id).with_extension("unsynced.json")
}

/// Record that this profile's local copy holds a session the server lacks.
pub fn mark(profile_id: &str, base_version: i32) {
    let marker = Marker {
        base_version,
        since: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
    };
    let path = marker_path(profile_id);
    match serde_json::to_vec_pretty(&marker) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!(profile = %profile_id, error = %e, "could not record the unsent session");
            }
        }
        Err(e) => tracing::warn!(error = %e, "could not encode the unsent-session marker"),
    }
}

/// The server version the local copy grew from, if its last session is unsent.
pub fn base(profile_id: &str) -> Option<i32> {
    let bytes = std::fs::read(marker_path(profile_id)).ok()?;
    serde_json::from_slice::<Marker>(&bytes).ok().map(|m| m.base_version)
}

/// The local copy and the server agree again.
pub fn clear(profile_id: &str) {
    let _ = std::fs::remove_file(marker_path(profile_id));
}

/// Move the local copy out of the way, keeping it. Returns where it went.
pub fn set_aside(profile_id: &str) -> anyhow::Result<PathBuf> {
    let dir = crate::paths::profile_dir(profile_id);
    let aside = dir.with_extension(format!("unsent-{}", time::OffsetDateTime::now_utc().unix_timestamp()));
    std::fs::rename(&dir, &aside)?;
    clear(profile_id);
    let _ = std::fs::remove_file(held_path(profile_id));
    Ok(aside)
}

// The other half of the same question: which server version the local copy IS,
// when it is one. Written after a pull has been unpacked and after a push has
// been accepted, and asked for at the next launch so that a profile this
// machine already holds is not downloaded and unpacked over itself.
//
// A session that never reached the server leaves this at the version it grew
// from, beside the marker above, and that is the right answer for it: the
// server at that version means "keep the local copy", which is what `decide`
// says too. A local copy newer than the record -- the agent killed with a
// browser open, nothing pushed and nothing marked -- is kept rather than
// overwritten with the older bundle it grew from, which is what used to happen.

fn held_path(profile_id: &str) -> PathBuf {
    crate::paths::profile_dir(profile_id).with_extension("version")
}

/// The server version this machine's copy of the profile is, if it has one.
pub fn held(profile_id: &str) -> Option<i32> {
    if !crate::paths::profile_dir(profile_id).is_dir() {
        return None;
    }
    std::fs::read_to_string(held_path(profile_id)).ok()?.trim().parse().ok()
}

/// The local copy and server version `version` are the same profile.
pub fn record_held(profile_id: &str, version: i32) {
    if let Err(e) = std::fs::write(held_path(profile_id), version.to_string()) {
        tracing::warn!(profile = %profile_id, error = %e, "could not record which version is here");
    }
}

/// What a launch does with the server's bundle.
#[derive(Debug, PartialEq, Eq)]
pub enum Pull {
    /// Unpack it: the local copy has nothing the server lacks.
    Unpack,
    /// Keep the local copy: it is the server's version plus a session the
    /// server never received.
    KeepLocal,
    /// Both moved: keep the local copy aside, then unpack the server's.
    SetAsideThenUnpack,
}

/// `local_base` is the marker's version, `server` the version on offer.
pub fn decide(local_base: Option<i32>, server: i32) -> Pull {
    match local_base {
        None => Pull::Unpack,
        Some(base) if base == server => Pull::KeepLocal,
        Some(_) => Pull::SetAsideThenUnpack,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsent_session_is_never_overwritten_by_the_version_it_grew_from() {
        assert_eq!(decide(None, 7), Pull::Unpack);
        // The tester's case: upload failed at version 7, server still at 7.
        assert_eq!(decide(Some(7), 7), Pull::KeepLocal);
        // A colleague uploaded 8 meanwhile: keep ours aside, take theirs.
        assert_eq!(decide(Some(7), 8), Pull::SetAsideThenUnpack);
        // First session of a shared profile, never uploaded (base 0), and
        // somebody has uploaded since.
        assert_eq!(decide(Some(0), 1), Pull::SetAsideThenUnpack);
    }
}
