//! Moving a project between machines.
//!
//! # Why this is encrypted and not a zip
//!
//! The file carries the thing the whole product exists to protect: cookie jars
//! for live accounts, and the passwords of the proxies they go out through.
//! A plain archive of that is worse than useless — it is the asset, in the
//! clear, in a Downloads folder, on a USB stick, in a chat message. So the
//! export takes a passphrase and the file is unreadable without it.
//!
//! Argon2id derives the key, because what a human types is low-entropy and a
//! fast hash would make the passphrase the weak point. XChaCha20-Poly1305 seals
//! it, because a 24-byte nonce can be random without anyone having to keep a
//! counter — and because it authenticates: a file someone has edited fails to
//! open rather than importing something else.
//!
//! # What it is not
//!
//! Not a substitute for a server. What comes out is a *copy*: whoever receives
//! it has those profiles forever, and nothing you do afterwards reaches them.
//! Sharing that can be withdrawn is what the team server is for (docs/13).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use crate::store::{Profile, Proxy, Store};

/// Magic and version. Read before anything else, so a file from a later
/// version says so instead of failing as "wrong passphrase" — which is the
/// error people spend an hour on.
const MAGIC: &[u8; 8] = b"FURY-EX1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;

#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub project_name: String,
    pub exported_at: String,
    pub profiles: Vec<Profile>,
    /// Only the proxies these profiles actually use.
    pub proxies: Vec<Proxy>,
    /// Whether browser data rode along, so the import can say what it restored.
    pub with_data: bool,
}

/// Write a project to `dest`.
pub async fn export_project(
    store: &Store,
    project_id: &str,
    dest: &Path,
    passphrase: &str,
    with_data: bool,
    profile_dir: impl Fn(&str) -> PathBuf,
) -> anyhow::Result<u64> {
    if passphrase.chars().count() < 8 {
        anyhow::bail!("the passphrase must be at least 8 characters — this file holds live accounts");
    }

    let project = store
        .projects()
        .await?
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| anyhow::anyhow!("no such project"))?;

    let profiles = store.profiles(project_id).await?;
    let used: Vec<Proxy> = {
        let ids: Vec<String> = profiles
            .iter()
            .filter_map(|p| p.proxy.as_ref().map(|x| x.id.clone()))
            .collect();
        // Fetched whole rather than taken from the join, which drops the
        // rotation link and the checker on purpose.
        store
            .proxies()
            .await?
            .into_iter()
            .filter(|x| ids.contains(&x.id))
            .collect()
    };

    let manifest = Manifest {
        project_name: project.name,
        exported_at: now(),
        profiles: profiles.clone(),
        proxies: used,
        with_data,
    };

    // tar in memory, then gzip, then seal. Small enough to hold: without
    // browser data a project is kilobytes, and with it the alternative is
    // streaming encryption, which buys nothing until profiles are gigabytes.
    let mut archive = tar::Builder::new(Vec::new());
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    append_bytes(&mut archive, "manifest.json", &manifest_bytes)?;

    if with_data {
        for profile in &profiles {
            let dir = profile_dir(&profile.id);
            if dir.exists() {
                archive.append_dir_all(format!("data/{}", profile.id), &dir)?;
            }
        }
    }
    let tar_bytes = archive.into_inner()?;

    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar_bytes)?;
    let compressed = gz.finish()?;

    let salt: [u8; SALT_LEN] = rand::random();
    let nonce_bytes: [u8; NONCE_LEN] = rand::random();
    let cipher = XChaCha20Poly1305::new((&derive_key(passphrase, &salt)?).into());
    let sealed = cipher
        .encrypt(XNonce::from_slice(&nonce_bytes), compressed.as_ref())
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;

    let mut out = Vec::with_capacity(MAGIC.len() + SALT_LEN + NONCE_LEN + sealed.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&sealed);

    std::fs::write(dest, &out)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The file is the accounts. Not world-readable, even for the moment it
        // sits in a Downloads folder.
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o600));
    }
    Ok(out.len() as u64)
}

/// Read a project back in, into a new project of its own.
///
/// Always a new project rather than a merge: importing twice must not
/// half-overwrite what is already there, and a profile that arrives with the
/// same id as a live one is a different account with a coincidence, not the
/// same account.
pub async fn import_project(
    store: &Store,
    src: &Path,
    passphrase: &str,
    profile_dir: impl Fn(&str) -> PathBuf,
) -> anyhow::Result<(String, usize)> {
    let raw = std::fs::read(src)?;
    if raw.len() < MAGIC.len() + SALT_LEN + NONCE_LEN || &raw[..MAGIC.len()] != MAGIC {
        anyhow::bail!("this is not a Fury export file");
    }

    let salt = &raw[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce = &raw[MAGIC.len() + SALT_LEN..MAGIC.len() + SALT_LEN + NONCE_LEN];
    let sealed = &raw[MAGIC.len() + SALT_LEN + NONCE_LEN..];

    let cipher = XChaCha20Poly1305::new((&derive_key(passphrase, salt)?).into());
    let compressed = cipher
        .decrypt(XNonce::from_slice(nonce), sealed)
        .map_err(|_| anyhow::anyhow!("wrong passphrase, or the file has been altered"))?;

    let mut gz = flate2::read::GzDecoder::new(compressed.as_slice());
    let mut tar_bytes = Vec::new();
    gz.read_to_end(&mut tar_bytes)?;

    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    let mut manifest: Option<Manifest> = None;
    let mut data: Vec<(String, PathBuf, Vec<u8>)> = Vec::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        if path == Path::new("manifest.json") {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf)?;
            manifest = Some(serde_json::from_slice(&buf)?);
        } else if let Ok(rest) = path.strip_prefix("data") {
            let mut parts = rest.components();
            let Some(id) = parts.next() else { continue };
            let id = id.as_os_str().to_string_lossy().to_string();
            let inner: PathBuf = parts.as_path().to_path_buf();
            // Refuse anything that would climb out of the profile directory. A
            // crafted archive with ../.. in a path is the classic way to write
            // outside the destination.
            if inner.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                anyhow::bail!("the archive contains a path that escapes its directory");
            }
            if entry.header().entry_type().is_file() {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                data.push((id, inner, buf));
            }
        }
    }

    let manifest = manifest.ok_or_else(|| anyhow::anyhow!("the archive has no manifest"))?;
    // Marked, because an import lands beside the project it came from as often
    // as not — and two identical names in a sidebar is a question the operator
    // has to answer by clicking.
    let taken: Vec<String> = store.projects().await?.into_iter().map(|p| p.name).collect();
    let name = if taken.contains(&manifest.project_name) {
        format!("{} (imported)", manifest.project_name)
    } else {
        manifest.project_name.clone()
    };
    let project_id = store.create_project(&name, "").await?;

    // Proxies first: a profile references one, so it has to exist. Ids are
    // remapped, because the receiving machine may already have a proxy under
    // the same id from an earlier import of the same file.
    let mut proxy_ids = std::collections::HashMap::new();
    for proxy in &manifest.proxies {
        let mut fresh = proxy.clone();
        fresh.id = String::new();
        let new_id = store.upsert_proxy(&fresh).await?;
        proxy_ids.insert(proxy.id.clone(), new_id);
    }

    let mut ids = std::collections::HashMap::new();
    for profile in &manifest.profiles {
        let mut fresh = profile.clone();
        fresh.id = String::new();
        fresh.project_id = project_id.clone();
        fresh.proxy = profile.proxy.as_ref().and_then(|x| {
            proxy_ids.get(&x.id).map(|id| Proxy {
                id: id.clone(),
                ..x.clone()
            })
        });
        // The seed rides along deliberately: a profile that comes back with a
        // different fingerprint is a different machine, and the account it
        // holds would notice.
        let new_id = store.upsert_profile(&fresh).await?;
        ids.insert(profile.id.clone(), new_id);
    }

    for (old_id, inner, bytes) in data {
        let Some(new_id) = ids.get(&old_id) else { continue };
        let target = profile_dir(new_id).join(&inner);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, &bytes)?;
    }

    Ok((project_id, manifest.profiles.len()))
}

fn derive_key(passphrase: &str, salt: &[u8]) -> anyhow::Result<[u8; 32]> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("deriving the key failed: {e}"))?;
    Ok(key)
}

fn append_bytes<W: Write>(
    archive: &mut tar::Builder<W>,
    name: &str,
    bytes: &[u8],
) -> anyhow::Result<()> {
    let mut header = tar::Header::new_gnu();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o600);
    header.set_cksum();
    archive.append_data(&mut header, name, bytes)?;
    Ok(())
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The directory comes back as a guard, not a path: every test here already
    /// holds it for the whole body, and now holding it is what cleans it up.
    async fn store() -> (Store, crate::tmp::TempDir) {
        let dir = crate::tmp::TempDir::new("x");
        (Store::open_for_tests(&dir.join("t.db")).await.unwrap(), dir)
    }

    fn profile(project: &str, name: &str) -> Profile {
        Profile {
            id: String::new(),
            project_id: project.into(),
            name: name.into(),
            notes: String::new(),
            tags: vec!["de".into()],
            persona_id: "win11-rtx4060-1920x1080".into(),
            fp_seed: 0,
            proxy: None,
            timezone: Some("Europe/Berlin".into()),
            languages: Some(vec!["de-DE".into()]),
            start_urls: vec![],
            last_opened_at: None,
        }
    }

    #[tokio::test]
    async fn a_project_survives_a_round_trip_with_its_fingerprints() {
        let (s, dir) = store().await;
        let project = s.default_project().await.unwrap();
        let proxy_id = s
            .upsert_proxy(&Proxy {
                id: String::new(),
                name: "eu".into(),
                kind: "socks5".into(),
                host: "exit.example".into(),
                port: 1080,
                username: Some("bob".into()),
                password: Some("s3cr3t".into()),
                last_country: Some("DE".into()),
                last_ip: None,
                rotate_url: None,
                checker_url: None,
            })
            .await
            .unwrap();

        let mut p = profile(&project, "Shop DE");
        p.proxy = Some(Proxy {
            id: proxy_id,
            name: String::new(),
            kind: String::new(),
            host: String::new(),
            port: 0,
            username: None,
            password: None,
            last_country: None,
            last_ip: None,
            rotate_url: None,
            checker_url: None,
        });
        let id = s.upsert_profile(&p).await.unwrap();
        let seed = s.profile(&id).await.unwrap().unwrap().fp_seed;

        let dirs = dir.to_path_buf();
        let profile_dir = move |pid: &str| dirs.join("profiles").join(pid);
        std::fs::create_dir_all(profile_dir(&id)).unwrap();
        std::fs::write(profile_dir(&id).join("Cookies"), b"warm").unwrap();

        let out = dir.join("export.fury");
        export_project(&s, &project, &out, "correct horse battery", true, profile_dir.clone())
            .await
            .unwrap();

        let (new_project, count) = import_project(&s, &out, "correct horse battery", profile_dir.clone())
            .await
            .unwrap();
        assert_eq!(count, 1);
        // Landing beside the project it came from, so the name has to say so.
        let names: Vec<String> = s.projects().await.unwrap().into_iter().map(|p| p.name).collect();
        assert!(names.iter().any(|n| n.ends_with("(imported)")), "{names:?}");

        let restored = s.profiles(&new_project).await.unwrap();
        assert_eq!(restored.len(), 1);
        let r = &restored[0];
        assert_eq!(r.name, "Shop DE");
        // The seed is the machine. A profile that comes back with a different
        // one is a different device, and the account would notice.
        assert_eq!(r.fp_seed, seed);
        assert_eq!(r.tags, vec!["de".to_string()]);
        // The proxy came with it, credentials included, under a new id.
        let px = r.proxy.as_ref().expect("proxy did not survive");
        assert_eq!(px.host, "exit.example");
        assert_ne!(px.id, "", "proxy id was not remapped");
        // And the browser data.
        assert_eq!(
            std::fs::read(profile_dir(&r.id).join("Cookies")).unwrap(),
            b"warm"
        );
    }

    #[tokio::test]
    async fn the_wrong_passphrase_does_not_open_it() {
        let (s, dir) = store().await;
        let project = s.default_project().await.unwrap();
        s.upsert_profile(&profile(&project, "a")).await.unwrap();
        let out = dir.join("x.fury");
        let nowhere = |_: &str| dir.join("nope");

        export_project(&s, &project, &out, "the right one", false, nowhere)
            .await
            .unwrap();
        let err = import_project(&s, &out, "the wrong one", nowhere)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("passphrase"), "{err}");
    }

    #[tokio::test]
    async fn a_tampered_file_fails_to_open_rather_than_importing_something_else() {
        let (s, dir) = store().await;
        let project = s.default_project().await.unwrap();
        s.upsert_profile(&profile(&project, "a")).await.unwrap();
        let out = dir.join("x.fury");
        let nowhere = |_: &str| dir.join("nope");
        export_project(&s, &project, &out, "a good passphrase", false, nowhere)
            .await
            .unwrap();

        // Flip a byte in the ciphertext. AEAD is the reason this is detected
        // rather than silently importing corrupted rows.
        let mut raw = std::fs::read(&out).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0xff;
        std::fs::write(&out, &raw).unwrap();

        assert!(import_project(&s, &out, "a good passphrase", nowhere).await.is_err());
    }

    #[tokio::test]
    async fn a_short_passphrase_is_refused_before_anything_is_written() {
        let (s, dir) = store().await;
        let project = s.default_project().await.unwrap();
        let out = dir.join("short.fury");
        let nowhere = |_: &str| dir.join("nope");
        assert!(export_project(&s, &project, &out, "abc", false, nowhere).await.is_err());
        assert!(!out.exists(), "a refused export still wrote a file");
    }

    #[tokio::test]
    async fn a_foreign_file_is_named_as_such() {
        let (s, dir) = store().await;
        let out = dir.join("notours.fury");
        std::fs::write(&out, b"PK\x03\x04 this is a zip").unwrap();
        let err = import_project(&s, &out, "whatever", |_| dir.join("nope"))
            .await
            .unwrap_err();
        // Not "wrong passphrase" — that is the error people spend an hour on.
        assert!(err.to_string().contains("not a Fury export"), "{err}");
    }
}
