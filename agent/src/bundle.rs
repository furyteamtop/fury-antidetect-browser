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
//! So each bundle carries its own data key, and only that key is wrapped by the
//! vault. Sharing later re-wraps 32 bytes instead of re-encrypting a gigabyte,
//! and revoking someone means they never receive the wrapped key again rather
//! than a re-encryption of everything they ever touched.
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

impl std::fmt::Debug for Sealed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sealed")
            .field("bytes", &self.bytes.len())
            .field("sha256", &self.sha256)
            .finish_non_exhaustive()
    }
}

pub fn pack(profile_dir: &Path, vault: &Vault) -> anyhow::Result<Sealed> {
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

    let wrapped_key = vault.seal(&hex(&data_key));
    if !vault.available() {
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
    vault: &Vault,
    dest: &Path,
) -> anyhow::Result<usize> {
    if sealed.len() < MAGIC.len() + NONCE_LEN || &sealed[..MAGIC.len()] != MAGIC {
        anyhow::bail!("this is not a Fury bundle");
    }
    let key = unhex(&vault.open_value(wrapped_key))
        .ok_or_else(|| anyhow::anyhow!("the bundle key does not belong to this machine"))?;

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
        if SKIP.iter().any(|s| *s == name) {
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

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("fury-bn-{tag}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&d).unwrap();
        d
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

        let sealed = pack(&src, &vault).unwrap();
        assert!(!sealed.bytes.windows(12).any(|w| w == b"warm-account"));
        assert_eq!(sealed.sha256.len(), 64);

        let dest = dir("dest");
        let n = unpack(&sealed.bytes, &sealed.wrapped_key, &vault, &dest).unwrap();
        assert_eq!(n, 2, "the singleton lock rode along");
        assert_eq!(std::fs::read(dest.join("Default/Cookies")).unwrap(), b"warm-account");
        assert!(!dest.join("SingletonLock").exists());
    }

    #[test]
    fn the_digest_is_of_the_ciphertext_so_a_server_can_check_it() {
        let vault = Vault::for_tests([3u8; 32]);
        let src = dir("d");
        std::fs::write(src.join("Cookies"), b"x").unwrap();
        let sealed = pack(&src, &vault).unwrap();
        assert_eq!(hex(&Sha256::digest(&sealed.bytes)), sealed.sha256);
    }

    #[test]
    fn another_machines_key_does_not_open_it() {
        let mine = Vault::for_tests([3u8; 32]);
        let theirs = Vault::for_tests([9u8; 32]);
        let src = dir("d");
        std::fs::write(src.join("Cookies"), b"secret").unwrap();
        let sealed = pack(&src, &mine).unwrap();

        assert!(unpack(&sealed.bytes, &sealed.wrapped_key, &theirs, &dir("out")).is_err());
    }

    #[test]
    fn an_altered_bundle_refuses_rather_than_restoring_something_else() {
        let vault = Vault::for_tests([3u8; 32]);
        let src = dir("d");
        std::fs::write(src.join("Cookies"), b"secret").unwrap();
        let mut sealed = pack(&src, &vault).unwrap();
        let last = sealed.bytes.len() - 1;
        sealed.bytes[last] ^= 0xff;

        assert!(unpack(&sealed.bytes, &sealed.wrapped_key, &vault, &dir("out")).is_err());
    }

    #[test]
    fn without_a_keychain_it_refuses_instead_of_shipping_its_own_key() {
        // seal() passes values through when there is no credential store, which
        // would put the data key beside the data. A bundle that looks encrypted
        // and carries its key is worse than an honest refusal.
        let none = Vault::for_tests_without_key();
        let src = dir("d");
        std::fs::write(src.join("Cookies"), b"secret").unwrap();
        let err = pack(&src, &none).unwrap_err();
        assert!(err.to_string().contains("carries its own key"), "{err}");
    }
}
