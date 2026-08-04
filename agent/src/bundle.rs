// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Profile bundles — what a server is allowed to hold.
//!
//! A bundle is one profile's browser data: the cookie jar, local storage, the
//! rest of the user-data directory. It is the asset. Everything about this
//! module exists so that a server can store it without being able to read it,
//! which is the precondition for sync existing at all (docs/06) and for anyone
//! hosting this for other people.
//!
//! # Why a key per profile
//!
//! The obvious construction is to encrypt with the machine key straight from
//! the vault. It is wrong for one reason: sharing. A bundle handed to a
//! colleague has to become readable by them and not by everyone — and with one
//! key per machine, "share this profile" means re-encrypting the whole
//! directory under their key, every time, for every recipient.
//!
//! So each bundle carries its own data key, and only that key is wrapped.
//! Sharing later re-wraps 32 bytes instead of re-encrypting a gigabyte, and
//! revoking someone means they never receive the wrapped key again rather than
//! a re-encryption of everything they ever touched.
//!
//! # What wraps that key, and why it is not always the vault
//!
//! For a profile that lives only on this machine, the vault — the machine key
//! in the OS keychain. Nothing else needs to open it.
//!
//! For a team profile, that is exactly wrong, and wrong in the way that defeats
//! the feature: a bundle packed on one operator's laptop and wrapped under
//! *that machine's* key cannot be opened by the colleague it was uploaded for.
//! The error even said so — "the bundle key does not belong to this machine" —
//! while the whole point of uploading it was that it should belong to someone
//! else too.
//!
//! So a team profile's data key is wrapped under a subkey derived from the
//! organisation key and the profile id. Every member can derive it, no other
//! profile shares it, and the agent is handed only the one it needs rather than
//! the organisation key itself.
//!
//! # What the server sees
//!
//! Ciphertext, a wrapped key it has no way to open, and a digest. The digest is
//! of the *ciphertext*, so integrity can be checked by something that cannot
//! decrypt — which is what lets a resumed or mirrored transfer be verified
//! without a key ever leaving a client.

use std::io::{Read, Write};
use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};

use crate::vault::Vault;

/// What wraps a bundle's data key.
///
/// Two cases and no default: a bundle sealed under the wrong one is a bundle
/// that opens nowhere, and the two are indistinguishable at the point of use
/// unless the caller has to say which it means.
pub enum Sealer<'a> {
    /// A profile that lives only on this machine.
    Machine(&'a Vault),
    /// A team profile: a key derived from the organisation key and this
    /// profile's id, which every member of the organisation can derive.
    ///
    /// The vault comes along for reading only. Bundles uploaded before this
    /// existed are wrapped under the machine key, and refusing them would mean
    /// an operator's own profile stops opening the day they update — on the
    /// machine that packed it, where the key is right there. The prefix says
    /// which is which, so this is a lookup and not a guess. Writing is always
    /// shared, so each such bundle converts itself the first time it is closed.
    Shared { key: [u8; 32], vault: Option<&'a Vault> },
}

/// Marks a key wrapped under a shared key rather than a machine one. Distinct
/// from the vault's own prefix so that neither can be mistaken for the other:
/// silently trying the wrong key would fail as "wrong password" a long way from
/// the cause.
const SHARED_PREFIX: &str = "fury:ork1:";

impl Sealer<'_> {
    fn seal(&self, plain: &str) -> String {
        match self {
            Sealer::Machine(v) => v.seal(plain),
            Sealer::Shared { key, .. } => {
                use base64::Engine;
                format!(
                    "{SHARED_PREFIX}{}",
                    base64::engine::general_purpose::STANDARD
                        .encode(fury_shared::keys::wrap(key, plain.as_bytes()))
                )
            }
        }
    }

    fn open(&self, stored: &str) -> String {
        match self {
            Sealer::Machine(v) => {
                if stored.starts_with(SHARED_PREFIX) {
                    // A team bundle reached a launch that has no organisation
                    // key. Returning nothing is right; pretending otherwise
                    // would produce a corrupt profile directory.
                    return String::new();
                }
                v.open_value(stored)
            }
            Sealer::Shared { key, vault } => {
                use base64::Engine;
                let Some(rest) = stored.strip_prefix(SHARED_PREFIX) else {
                    // Packed before team bundles had their own key. Openable
                    // here if this is the machine that packed it, and nowhere
                    // else — which the caller's error message says.
                    return vault.map(|v| v.open_value(stored)).unwrap_or_default();
                };
                base64::engine::general_purpose::STANDARD
                    .decode(rest)
                    .ok()
                    .and_then(|blob| fury_shared::keys::unwrap(key, &blob).ok())
                    .and_then(|v| String::from_utf8(v).ok())
                    .unwrap_or_default()
            }
        }
    }

    /// Whether sealing will actually seal. A vault with no keychain passes
    /// values through; a shared key always works.
    fn available(&self) -> bool {
        match self {
            Sealer::Machine(v) => v.available(),
            Sealer::Shared { .. } => true,
        }
    }
}

const MAGIC: &[u8; 8] = b"FURY-BN1";
const NONCE_LEN: usize = 24;

/// A sealed profile, ready to be handed to something untrusted.
///
/// Debug prints the digest and the size, never the ciphertext or the wrapped
/// key — a struct that dumps its own payload into a log is how secrets end up
/// in crash reports.
pub struct Sealed {
    pub bytes: Vec<u8>,
    /// The data key, sealed by this machine's vault key. Travels beside the
    /// bundle and is useless without the vault.
    pub wrapped_key: String,
    /// SHA-256 of `bytes` — of the ciphertext, so a server can verify what it
    /// stores without being able to read it.
    pub sha256: String,
}

/// Names inside a profile directory that must never be copied.
///
/// Lock files and sockets describe a running browser on one machine; restoring
/// them elsewhere produces a profile Chromium refuses to open, and the report
/// is always "the profile is corrupt" rather than "there is a stale lock".
const SKIP: &[&str] = &["SingletonLock", "SingletonSocket", "SingletonCookie", "lockfile"];

/// Caches. Not account state, and not worth carrying between machines.
///
/// Measured on a real profile from this machine: 33 MB on disk, of which
/// 19.7 MB was `download_cache`, 5.9 MB `GraphiteDawnCache`, 1.8 MB
/// `component_crx_cache`, 1.6 MB `CertificateRevocation`, and another 2.7 MB of
/// GPU and dictionary caches inside `Default`. What a colleague actually needs
/// — cookies, storage, history, sessions, preferences — came to under 1.5 MB.
///
/// So this is not tidiness: it is the difference between a sync that moves a
/// megabyte and one that moves thirty, on every close, for every profile.
///
/// Two of them would be actively wrong to carry. GPUCache, DawnGraphiteCache,
/// DawnWebGPUCache and GraphiteDawnCache are keyed to the GPU that filled them,
/// and restoring one machine's onto another is asking a driver to read another
/// driver's compiled shaders. Chromium discards them, which is the good case.
///
/// Everything here is rebuilt on demand by design; nothing here is a login.
///
/// The list was chosen by measuring the three real profiles on the machine that
/// wrote this, not by pattern-matching on the word "cache". Two candidates were
/// dropped for the same reason in reverse:
///
///   - `Service Worker` is 20-32 KB and holds registrations, not just scripts.
///     Dropping it can sign somebody out of a site whose auth lives in a worker,
///     which is a bad trade for 30 KB.
///   - `Network Action Predictor` is 52-80 KB of typed-URL history. It is
///     behaviour, not cache, and behaviour is part of what a profile is for.
const SKIP_CACHES: &[&str] = &[
    // Whole-profile, measured at 19.7 MB, 5.9 MB, 1.8 MB and 1.6 MB
    // respectively on a 33 MB profile.
    "download_cache",
    "GraphiteDawnCache",
    "component_crx_cache",
    "CertificateRevocation",
    "Subresource Filter",
    "segmentation_platform",
    // Per-profile, inside Default/. The Dawn and GPU caches are keyed to the
    // GPU that filled them; carrying one machine's to another asks a driver to
    // read another driver's compiled shaders, and Chromium discards them, which
    // is the good outcome rather than the bad one.
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "ShaderCache",
    "GrShaderCache",
    // 1 MB on the largest profile here: compression dictionaries a site sends
    // and the browser re-fetches.
    "Shared Dictionary",
    "optimization_guide_hint_cache_store",
];

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sealed")
            .field("bytes", &self.bytes.len())
            .field("sha256", &self.sha256)
            .finish_non_exhaustive()
    }
}

pub fn pack(profile_dir: &Path, sealer: &Sealer<'_>) -> anyhow::Result<Sealed> {
    let mut archive = tar::Builder::new(Vec::new());
    if profile_dir.exists() {
        append_dir(&mut archive, profile_dir, profile_dir)?;
    }
    let tar_bytes = archive.into_inner()?;

    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar_bytes)?;
    let compressed = gz.finish()?;

    let data_key: [u8; 32] = rand::random();
    let nonce: [u8; NONCE_LEN] = rand::random();
    let sealed = XChaCha20Poly1305::new((&data_key).into())
        .encrypt(XNonce::from_slice(&nonce), compressed.as_ref())
        .map_err(|_| anyhow::anyhow!("sealing the bundle failed"))?;

    let mut bytes = Vec::with_capacity(MAGIC.len() + NONCE_LEN + sealed.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&sealed);

    let wrapped_key = sealer.seal(&hex(&data_key));
    if !sealer.available() {
        // seal() passes through when there is no keychain, which would put the
        // data key next to the data it protects. Better to refuse: a bundle
        // that looks encrypted and ships its own key is worse than none.
        anyhow::bail!(
            "no credential store, so the bundle key cannot be protected — refusing to \
             produce a bundle that carries its own key"
        );
    }

    Ok(Sealed {
        sha256: hex(&Sha256::digest(&bytes)),
        bytes,
        wrapped_key,
    })
}

pub fn unpack(
    sealed: &[u8],
    wrapped_key: &str,
    sealer: &Sealer<'_>,
    dest: &Path,
) -> anyhow::Result<usize> {
    if sealed.len() < MAGIC.len() + NONCE_LEN || &sealed[..MAGIC.len()] != MAGIC {
        anyhow::bail!("this is not a Fury bundle");
    }
    let key = unhex(&sealer.open(wrapped_key)).ok_or_else(|| {
        anyhow::anyhow!(
            "this bundle's key will not open here. A team profile needs the organisation key, \
             and a personal one needs the machine that packed it"
        )
    })?;

    let nonce = &sealed[MAGIC.len()..MAGIC.len() + NONCE_LEN];
    let body = &sealed[MAGIC.len() + NONCE_LEN..];
    let compressed = XChaCha20Poly1305::new((&key).into())
        .decrypt(XNonce::from_slice(nonce), body)
        .map_err(|_| anyhow::anyhow!("the bundle is not readable with this key, or was altered"))?;

    let mut gz = flate2::read::GzDecoder::new(compressed.as_slice());
    let mut tar_bytes = Vec::new();
    gz.read_to_end(&mut tar_bytes)?;

    std::fs::create_dir_all(dest)?;
    let mut written = 0usize;
    for entry in tar::Archive::new(tar_bytes.as_slice()).entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        // A crafted archive with ../.. is the classic way to write outside the
        // destination, and a bundle is exactly the kind of thing that arrives
        // from somewhere else.
        if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            anyhow::bail!("the bundle contains a path that escapes its directory");
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let target = dest.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        std::fs::write(&target, &buf)?;
        written += 1;
    }
    Ok(written)
}

fn append_dir<W: Write>(
    archive: &mut tar::Builder<W>,
    root: &Path,
    dir: &Path,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if SKIP.iter().any(|s| *s == name) || SKIP_CACHES.iter().any(|s| *s == name) {
            continue;
        }
        if path.is_dir() {
            append_dir(archive, root, &path)?;
        } else if path.is_file() {
            let rel = path.strip_prefix(root)?;
            let mut file = std::fs::File::open(&path)?;
            archive.append_file(rel, &mut file)?;
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::tmp::TempDir;

    fn dir(tag: &str) -> TempDir {
        TempDir::new(&format!("bn-{tag}"))
    }

    #[test]
    fn a_team_bundle_opens_on_a_machine_that_never_packed_it() {
        // The bug this replaces defeated the whole feature: the data key was
        // wrapped under the packing machine's vault, so a colleague pulling the
        // bundle was told "the bundle key does not belong to this machine" —
        // which was true, and was the point of uploading it.
        let ork = fury_shared::keys::new_org_key();
        let key = fury_shared::keys::subkey(&ork, "fury-profile-v1", "profile-a");

        let src = dir("src");
        std::fs::create_dir_all(src.join("Default")).unwrap();
        std::fs::write(src.join("Default/Cookies"), b"warm-account").unwrap();
        let sealed = pack(&src, &Sealer::Shared { key, vault: None }).unwrap();

        // Another machine: a different vault entirely, and only the
        // organisation key in common.
        let dest = dir("dest");
        let same = fury_shared::keys::subkey(&ork, "fury-profile-v1", "profile-a");
        assert_eq!(
            unpack(&sealed.bytes, &sealed.wrapped_key, &Sealer::Shared { key: same, vault: None }, &dest).unwrap(),
            1
        );
        assert_eq!(std::fs::read(dest.join("Default/Cookies")).unwrap(), b"warm-account");
    }

    #[test]
    fn a_bundle_packed_before_sharing_still_opens_where_it_was_packed() {
        // Updating must not strand an operator's own profiles. The prefix says
        // which key wrapped it, so this is a lookup rather than a guess — and
        // the next close re-seals it shared.
        let vault = Vault::for_tests([3u8; 32]);
        let src = dir("src");
        std::fs::create_dir_all(src.join("Default")).unwrap();
        std::fs::write(src.join("Default/Cookies"), b"warm-account").unwrap();
        let old = pack(&src, &Sealer::Machine(&vault)).unwrap();

        let ork = fury_shared::keys::new_org_key();
        let shared = Sealer::Shared {
            key: fury_shared::keys::subkey(&ork, "fury-profile-v1", "p"),
            vault: Some(&vault),
        };
        let dest = dir("dest");
        assert_eq!(unpack(&old.bytes, &old.wrapped_key, &shared, &dest).unwrap(), 1);

        // And what it writes from now on is shared, so it converts itself.
        assert!(pack(&src, &shared).unwrap().wrapped_key.starts_with(SHARED_PREFIX));
    }

    #[test]
    fn a_team_bundle_does_not_open_under_the_wrong_key() {
        let ork = fury_shared::keys::new_org_key();
        let src = dir("src");
        std::fs::create_dir_all(src.join("Default")).unwrap();
        std::fs::write(src.join("Default/Cookies"), b"warm-account").unwrap();
        let sealed = pack(
            &src,
            &Sealer::Shared {
                key: fury_shared::keys::subkey(&ork, "fury-profile-v1", "profile-a"),
                vault: None,
            },
        )
        .unwrap();

        // A different profile in the same organisation.
        let other = fury_shared::keys::subkey(&ork, "fury-profile-v1", "profile-b");
        assert!(unpack(&sealed.bytes, &sealed.wrapped_key, &Sealer::Shared { key: other, vault: None }, &dir("d1")).is_err());

        // And a machine vault, which is what a local launch would bring.
        let vault = Vault::for_tests([3u8; 32]);
        assert!(
            unpack(&sealed.bytes, &sealed.wrapped_key, &Sealer::Machine(&vault), &dir("d2")).is_err(),
            "a team bundle opened under a machine key"
        );
    }

    /// What a real profile's bundle actually weighs.
    ///
    /// Not an assertion about a number — an assertion that the number is
    /// printed, so the quotas on a shared server are sized from a measurement
    /// instead of from somebody's idea of a megabyte. Run with --nocapture and
    /// FURY_MEASURE_PROFILE pointing at a profile directory.
    #[test]
    fn a_real_profile_measures_what_it_measures() {
        let Ok(dir) = std::env::var("FURY_MEASURE_PROFILE") else { return };
        let path = std::path::Path::new(&dir);
        if !path.is_dir() {
            return;
        }
        let on_disk: u64 = walkdir(path);
        let vault = Vault::for_tests([3u8; 32]);
        let sealed = pack(path, &Sealer::Machine(&vault)).unwrap();
        eprintln!(
            "  {}: {:.1} MB on disk -> {:.2} MB sealed ({:.0}x smaller)",
            path.file_name().unwrap().to_string_lossy(),
            on_disk as f64 / 1e6,
            sealed.bytes.len() as f64 / 1e6,
            on_disk as f64 / sealed.bytes.len().max(1) as f64,
        );
    }

    fn walkdir(p: &std::path::Path) -> u64 {
        std::fs::read_dir(p)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| {
                        let path = e.path();
                        if path.is_dir() { walkdir(&path) } else { path.metadata().map(|m| m.len()).unwrap_or(0) }
                    })
                    .sum()
            })
            .unwrap_or(0)
    }

    #[test]
    fn caches_do_not_travel_but_account_state_does() {
        // The measurement that produced SKIP_CACHES, as an assertion. A profile
        // on the machine this was written on was 33 MB, of which 32 was cache;
        // team sync had never worked for a real profile, partly because of that
        // and partly because the server refused anything over 2 MB.
        let d = dir("skip-caches");
        let default = d.join("Default");
        std::fs::create_dir_all(default.join("GPUCache")).unwrap();
        std::fs::create_dir_all(d.join("download_cache")).unwrap();
        std::fs::create_dir_all(default.join("Service Worker")).unwrap();
        std::fs::write(default.join("GPUCache/data_1"), vec![b'c'; 200_000]).unwrap();
        std::fs::write(d.join("download_cache/blob"), vec![b'c'; 200_000]).unwrap();
        std::fs::write(default.join("Cookies"), b"the account").unwrap();
        std::fs::write(default.join("Service Worker/reg.db"), b"a registration").unwrap();

        let vault = Vault::for_tests([3u8; 32]);
        let sealed = pack(&d, &Sealer::Machine(&vault)).unwrap();
        let out = dir("skip-caches-out");
        unpack(&sealed.bytes, &sealed.wrapped_key, &Sealer::Machine(&vault), &out).unwrap();

        assert!(out.join("Default/Cookies").exists(), "the account did not travel");
        // Kept deliberately: a registration is not a cache, and dropping it can
        // sign somebody out of a site whose auth lives in a worker.
        assert!(
            out.join("Default/Service Worker/reg.db").exists(),
            "a service worker registration was dropped"
        );
        assert!(!out.join("Default/GPUCache").exists(), "a GPU cache travelled");
        assert!(!out.join("download_cache").exists(), "the download cache travelled");
        // 400 KB of cache in, and the sealed bundle should be nowhere near it.
        assert!(
            sealed.bytes.len() < 50_000,
            "the bundle is {} bytes — the caches are still in it",
            sealed.bytes.len()
        );
    }

    #[test]
    fn a_profile_survives_a_round_trip() {
        let vault = Vault::for_tests([3u8; 32]);
        let src = dir("src");
        std::fs::create_dir_all(src.join("Default")).unwrap();
        std::fs::write(src.join("Default/Cookies"), b"warm-account").unwrap();
        std::fs::write(src.join("Local State"), b"state").unwrap();
        // Must not travel: it describes a browser running on another machine.
        std::fs::write(src.join("SingletonLock"), b"pid").unwrap();

        let sealed = pack(&src, &Sealer::Machine(&vault)).unwrap();
        assert!(!sealed.bytes.windows(12).any(|w| w == b"warm-account"));
        assert_eq!(sealed.sha256.len(), 64);

        let dest = dir("dest");
        let n = unpack(&sealed.bytes, &sealed.wrapped_key, &Sealer::Machine(&vault), &dest).unwrap();
        assert_eq!(n, 2, "the singleton lock rode along");
        assert_eq!(std::fs::read(dest.join("Default/Cookies")).unwrap(), b"warm-account");
        assert!(!dest.join("SingletonLock").exists());
    }

    #[test]
    fn the_digest_is_of_the_ciphertext_so_a_server_can_check_it() {
        let vault = Vault::for_tests([3u8; 32]);
        let src = dir("d");
        std::fs::write(src.join("Cookies"), b"x").unwrap();
        let sealed = pack(&src, &Sealer::Machine(&vault)).unwrap();
        assert_eq!(hex(&Sha256::digest(&sealed.bytes)), sealed.sha256);
    }

    #[test]
    fn another_machines_key_does_not_open_it() {
        let mine = Vault::for_tests([3u8; 32]);
        let theirs = Vault::for_tests([9u8; 32]);
        let src = dir("d");
        std::fs::write(src.join("Cookies"), b"secret").unwrap();
        let sealed = pack(&src, &Sealer::Machine(&mine)).unwrap();

        assert!(unpack(&sealed.bytes, &sealed.wrapped_key, &Sealer::Machine(&theirs), &dir("out")).is_err());
    }

    #[test]
    fn an_altered_bundle_refuses_rather_than_restoring_something_else() {
        let vault = Vault::for_tests([3u8; 32]);
        let src = dir("d");
        std::fs::write(src.join("Cookies"), b"secret").unwrap();
        let mut sealed = pack(&src, &Sealer::Machine(&vault)).unwrap();
        let last = sealed.bytes.len() - 1;
        sealed.bytes[last] ^= 0xff;

        assert!(unpack(&sealed.bytes, &sealed.wrapped_key, &Sealer::Machine(&vault), &dir("out")).is_err());
    }

    #[test]
    fn without_a_keychain_it_refuses_instead_of_shipping_its_own_key() {
        // seal() passes values through when there is no credential store, which
        // would put the data key beside the data. A bundle that looks encrypted
        // and carries its key is worse than an honest refusal.
        let none = Vault::for_tests_without_key();
        let src = dir("d");
        std::fs::write(src.join("Cookies"), b"secret").unwrap();
        let err = pack(&src, &Sealer::Machine(&none)).unwrap_err();
        assert!(err.to_string().contains("carries its own key"), "{err}");
    }
}
