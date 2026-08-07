// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Installing a browser extension into a profile, so that its identity survives.
//!
//! The naive version is four lines: unzip the .crx somewhere and pass
//! `--load-extension`. It works once and then quietly ruins every clone.
//!
//! Chromium derives an unpacked extension's ID from the ABSOLUTE PATH it was
//! loaded from, unless the manifest carries a `key`. So an extension loaded from
//! `…/profiles/abc/ext/ublock` and the same extension in a cloned profile at
//! `…/profiles/def/ext/ublock` are two different extensions as far as the
//! browser is concerned: different IDs, different `chrome-extension://` origin,
//! and therefore different `chrome.storage` — which is where an extension keeps
//! the fact that you are logged in.
//!
//! That is the whole reason this module is not four lines. Clone a profile with
//! an anti-captcha or a wallet extension in it and the clone is signed out of
//! it, with nothing anywhere explaining why.
//!
//! The fix is to put the developer's real public key into the manifest, which
//! pins the ID to the key rather than to the path. The key is already in the
//! .crx — it is what the package is signed with — so nothing has to be invented,
//! only extracted.
//!
//! ## The CRX3 container
//!
//! ```text
//!   [4]  "Cr24"
//!   [4]  version, little-endian, == 3
//!   [4]  header length, little-endian
//!   [n]  CrxFileHeader, protobuf
//!   [..] a ZIP archive — the extension itself
//! ```
//!
//! and inside the header, the two fields that matter:
//!
//! ```text
//!   field 2      repeated AsymmetricKeyProof { field 1: public_key }
//!   field 10000  SignedData { field 1: crx_id }   — 16 bytes
//! ```
//!
//! There can be several proofs. The right key is the one whose
//! `SHA-256(public_key)[..16]` equals `crx_id`, and checking that is not
//! ceremony: it is what makes the extracted key the package's own rather than
//! the first one in a list.
//!
//! ## What this does NOT do
//!
//! It does not verify the signature. Chromium does that itself when it loads a
//! packed extension, and for an unpacked one — which is what `--load-extension`
//! takes — there is nothing to verify against, because the operator supplied
//! the file. Claiming a signature check that consists of comparing a hash to a
//! field inside the same file would be worse than not claiming one.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::Engine;
use sha2::{Digest, Sha256};

/// What a .crx turned out to contain.
#[derive(Debug, Clone)]
pub struct Crx {
    /// The 32-character `aaaa…` identifier Chromium will use.
    pub id: String,
    /// The developer's public key, DER, exactly as it sat in the package.
    pub public_key: Vec<u8>,
    /// Where the ZIP starts.
    zip_at: usize,
}

/// An extension after it has been unpacked into a profile.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Installed {
    pub id: String,
    pub name: String,
    pub version: String,
    pub path: String,
}

const MAGIC: &[u8] = b"Cr24";

/// Reads the container and works out which key the package belongs to.
pub fn parse(bytes: &[u8]) -> Result<Crx> {
    if bytes.len() < 16 || &bytes[..4] != MAGIC {
        bail!("not a .crx file: it does not start with Cr24");
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if version != 3 {
        // CRX2 was retired by Chromium in 2019 and its header is a different
        // shape entirely, so refusing is honest where guessing would not be.
        bail!("CRX version {version} — only version 3 is supported");
    }
    let header_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let header_at: usize = 12;
    let zip_at = header_at
        .checked_add(header_len)
        .filter(|end| *end <= bytes.len())
        .context("the CRX header length runs past the end of the file")?;
    let header = &bytes[header_at..zip_at];

    // field 10000, the signed header, holding the authoritative id.
    let signed = field(header, 10000).context("no signed header in the CRX")?;
    let crx_id = field(signed, 1).context("the signed header carries no crx_id")?;
    if crx_id.len() != 16 {
        bail!("crx_id is {} bytes, expected 16", crx_id.len());
    }

    // field 2, the RSA proofs. The right key is the one the id was derived
    // from — with several proofs present, taking the first would be a guess.
    let mut chosen = None;
    for proof in fields(header, 2) {
        if let Some(key) = field(proof, 1) {
            if &Sha256::digest(key)[..16] == crx_id {
                chosen = Some(key.to_vec());
                break;
            }
        }
    }
    let public_key = chosen.context(
        "no public key in the CRX hashes to its own crx_id — the package is inconsistent",
    )?;

    Ok(Crx { id: id_from_key(&public_key), public_key, zip_at })
}

/// Chromium's extension id: the first 16 bytes of SHA-256 over the key, with
/// every nibble written as a letter from `a` to `p`.
///
/// Not hex, and not base64. A 16-byte hash becomes 32 characters because each
/// byte contributes its high nibble and then its low one.
pub fn id_from_key(public_key: &[u8]) -> String {
    let digest = Sha256::digest(public_key);
    let mut out = String::with_capacity(32);
    for byte in &digest[..16] {
        out.push((b'a' + (byte >> 4)) as char);
        out.push((b'a' + (byte & 0x0f)) as char);
    }
    out
}

/// Unpacks into `dir` and pins the identity by writing the key into the manifest.
///
/// `dir` is where this extension lives for this profile; it is replaced if it
/// already exists, because installing over a half-unpacked directory is how an
/// extension ends up with two versions of a file.
pub fn install(crx: &[u8], dir: &Path) -> Result<Installed> {
    let parsed = parse(crx)?;

    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir)?;

    unzip(&crx[parsed.zip_at..], dir)?;

    let manifest_path = dir.join("manifest.json");
    if !manifest_path.is_file() {
        bail!("the package has no manifest.json at its top level");
    }
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path)?).context("reading manifest.json")?;

    // The line this whole module exists for.
    let obj = manifest
        .as_object_mut()
        .context("manifest.json is not an object")?;
    obj.insert(
        "key".into(),
        serde_json::Value::String(
            base64::engine::general_purpose::STANDARD.encode(&parsed.public_key),
        ),
    );
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;

    let name = manifest
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("extension")
        .to_string();
    let version = manifest
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or("0")
        .to_string();

    Ok(Installed { id: parsed.id, name, version, path: dir.display().to_string() })
}

/// Every extension directory installed for a profile, in a stable order.
pub fn installed(root: &Path) -> Vec<Installed> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let manifest = dir.join("manifest.json");
        if !manifest.is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(&manifest) else { continue };
        let Ok(json) = serde_json::from_slice::<serde_json::Value>(&bytes) else { continue };
        // The id comes back from the key rather than from the directory name,
        // so what is reported is what Chromium will actually use.
        let id = json
            .get("key")
            .and_then(|v| v.as_str())
            .and_then(|k| base64::engine::general_purpose::STANDARD.decode(k).ok())
            .map(|k| id_from_key(&k))
            .unwrap_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().to_string());
        out.push(Installed {
            id,
            name: json.get("name").and_then(|v| v.as_str()).unwrap_or("extension").to_string(),
            version: json.get("version").and_then(|v| v.as_str()).unwrap_or("0").to_string(),
            path: dir.display().to_string(),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// Just enough ZIP
// ---------------------------------------------------------------------------
//
// The first version shelled out to `tar`, on a measurement that bsdtar reads
// zip — which is true on macOS and on Windows, and false on Linux, where `tar`
// is GNU tar and answers "This does not look like a tar archive". Linux is not
// a shipped platform but CI runs there, so the first CI run after that commit
// failed on four tests. The measurement was right and the generalisation from
// "bsdtar" to "tar" was not.
//
// Reading it here is better than fixing the fallback, for a reason that has
// nothing to do with platforms: an external unpacker does not check where the
// entries land. A .crx is supplied by the operator, and an archive entry named
// `../../../../etc/cron.d/x` is the oldest trick there is. `safe_join` below is
// the guard, and it is a test rather than a hope.
//
// flate2 is already a dependency of this crate, so this adds nothing.

/// Unpacks a ZIP into `dir`. Stored and deflated entries only, which is what
/// every real .crx contains.
fn unzip(zip: &[u8], dir: &Path) -> Result<()> {
    // The end-of-central-directory record, found by searching backwards for its
    // signature. It is at the very end unless the archive has a comment, which
    // is why this is a search rather than a fixed offset.
    let eocd = (0..zip.len().saturating_sub(21))
        .rev()
        .find(|&i| zip[i..].starts_with(&0x0605_4b50u32.to_le_bytes()))
        .context("not a ZIP archive: no end-of-central-directory record")?;

    let count = u16::from_le_bytes(zip[eocd + 10..eocd + 12].try_into().unwrap()) as usize;
    let mut at = u32::from_le_bytes(zip[eocd + 16..eocd + 20].try_into().unwrap()) as usize;

    for _ in 0..count {
        if !zip[at..].starts_with(&0x0201_4b50u32.to_le_bytes()) {
            bail!("the ZIP central directory is malformed");
        }
        let method = u16::from_le_bytes(zip[at + 10..at + 12].try_into().unwrap());
        let compressed = u32::from_le_bytes(zip[at + 20..at + 24].try_into().unwrap()) as usize;
        let name_len = u16::from_le_bytes(zip[at + 28..at + 30].try_into().unwrap()) as usize;
        let extra_len = u16::from_le_bytes(zip[at + 30..at + 32].try_into().unwrap()) as usize;
        let comment_len = u16::from_le_bytes(zip[at + 32..at + 34].try_into().unwrap()) as usize;
        let local_at = u32::from_le_bytes(zip[at + 42..at + 46].try_into().unwrap()) as usize;
        let name = String::from_utf8_lossy(&zip[at + 46..at + 46 + name_len]).to_string();
        at += 46 + name_len + extra_len + comment_len;

        // The local header repeats the name and extra fields, and its extra
        // field length may DIFFER from the central one — reading the central
        // value here is the classic way a zip reader lands mid-file.
        if !zip[local_at..].starts_with(&0x0403_4b50u32.to_le_bytes()) {
            bail!("a ZIP entry points at no local header");
        }
        let l_name = u16::from_le_bytes(zip[local_at + 26..local_at + 28].try_into().unwrap()) as usize;
        let l_extra = u16::from_le_bytes(zip[local_at + 28..local_at + 30].try_into().unwrap()) as usize;
        let data_at = local_at + 30 + l_name + l_extra;
        let data = zip
            .get(data_at..data_at + compressed)
            .context("a ZIP entry runs past the end of the archive")?;

        // Directory entries end in a slash and carry no data.
        if name.ends_with('/') {
            std::fs::create_dir_all(safe_join(dir, &name)?)?;
            continue;
        }

        let out = safe_join(dir, &name)?;
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match method {
            0 => std::fs::write(&out, data)?,
            8 => {
                use std::io::Read;
                let mut decoder = flate2::read::DeflateDecoder::new(data);
                let mut bytes = Vec::new();
                decoder
                    .read_to_end(&mut bytes)
                    .with_context(|| format!("inflating {name}"))?;
                std::fs::write(&out, bytes)?;
            }
            other => bail!("{name} uses compression method {other}, which this does not read"),
        }
    }
    Ok(())
}

/// Joins an archive entry name onto a directory, refusing anything that would
/// land outside it.
///
/// Absolute paths, `..`, and Windows drive letters and backslashes — a
/// well-formed zip has none of them and an attack has all of them.
fn safe_join(dir: &Path, name: &str) -> Result<PathBuf> {
    if name.starts_with('/') || name.starts_with('\\') || name.contains(':') {
        bail!("the archive entry {name:?} is an absolute path");
    }
    let mut out = dir.to_path_buf();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => bail!("the archive entry {name:?} climbs out of the extension directory"),
            other => out.push(other),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Just enough protobuf
// ---------------------------------------------------------------------------
//
// Two length-delimited fields out of a header that is never more than a few
// kilobytes. A protobuf crate plus a build-time .proto would be a dependency
// and a code generator for that.

fn varint(buf: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = *buf.get(*at)?;
        *at += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// Every length-delimited field with this number, in order.
fn fields(buf: &[u8], want: u64) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < buf.len() {
        let Some(tag) = varint(buf, &mut at) else { break };
        let number = tag >> 3;
        match tag & 0x07 {
            // length-delimited
            2 => {
                let Some(len) = varint(buf, &mut at) else { break };
                let end = match at.checked_add(len as usize) {
                    Some(e) if e <= buf.len() => e,
                    _ => break,
                };
                if number == want {
                    out.push(&buf[at..end]);
                }
                at = end;
            }
            0 => {
                if varint(buf, &mut at).is_none() {
                    break;
                }
            }
            5 => at += 4,
            1 => at += 8,
            // Groups were removed from proto3 and nothing here emits them.
            _ => break,
        }
    }
    out
}

fn field(buf: &[u8], want: u64) -> Option<&[u8]> {
    fields(buf, want).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(number: u64, wire: u64) -> Vec<u8> {
        let mut v = (number << 3) | wire;
        let mut out = Vec::new();
        loop {
            let mut byte = (v & 0x7f) as u8;
            v >>= 7;
            if v != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if v == 0 {
                return out;
            }
        }
    }

    fn len_delim(number: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = tag(number, 2);
        let mut n = payload.len() as u64;
        loop {
            let mut byte = (n & 0x7f) as u8;
            n >>= 7;
            if n != 0 {
                byte |= 0x80;
            }
            out.push(byte);
            if n == 0 {
                break;
            }
        }
        out.extend_from_slice(payload);
        out
    }

    /// A CRX3 whose header is real: the id in the signed data is derived from
    /// the key, so `parse` has something to check rather than to assume.
    fn crx_with(key: &[u8], payload: &[u8], decoys: usize) -> Vec<u8> {
        let crx_id = Sha256::digest(key)[..16].to_vec();

        let mut header = Vec::new();
        // Decoy proofs first, so "take the first key" would pick the wrong one.
        for i in 0..decoys {
            let other = vec![0xAA ^ i as u8; 40];
            header.extend(len_delim(2, &len_delim(1, &other)));
        }
        header.extend(len_delim(2, &len_delim(1, key)));
        header.extend(len_delim(10000, &len_delim(1, &crx_id)));

        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&3u32.to_le_bytes());
        out.extend_from_slice(&(header.len() as u32).to_le_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(payload);
        out
    }

    /// A one-file ZIP with no compression, built by hand so the test needs no
    /// zip tool to produce its own input.
    fn stored_zip(name: &str, body: &[u8]) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut table = [0u32; 256];
            for (i, e) in table.iter_mut().enumerate() {
                let mut c = i as u32;
                for _ in 0..8 {
                    c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
                }
                *e = c;
            }
            let mut c = 0xFFFF_FFFFu32;
            for b in data {
                c = table[((c ^ *b as u32) & 0xff) as usize] ^ (c >> 8);
            }
            c ^ 0xFFFF_FFFF
        }

        let crc = crc32(body);
        let n = name.as_bytes();
        let mut out = Vec::new();

        let local_at = out.len() as u32;
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // stored
        out.extend_from_slice(&0u16.to_le_bytes()); // time
        out.extend_from_slice(&0u16.to_le_bytes()); // date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(n);
        out.extend_from_slice(body);

        let central_at = out.len() as u32;
        out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version made by
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&(n.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(&0u16.to_le_bytes()); // comment
        out.extend_from_slice(&0u16.to_le_bytes()); // disk
        out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        out.extend_from_slice(&local_at.to_le_bytes());
        out.extend_from_slice(n);
        let central_size = out.len() as u32 - central_at;

        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&central_size.to_le_bytes());
        out.extend_from_slice(&central_at.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fury-ext-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_id_is_nibbles_written_as_letters() {
        // Not hex and not base64: 16 bytes become 32 characters, each nibble a
        // letter from a to p.
        let id = id_from_key(b"anything");
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|c| (b'a'..=b'p').contains(&c)), "{id}");
        assert_eq!(id, id_from_key(b"anything"), "and it is a function of the key");
        assert_ne!(id, id_from_key(b"anything else"));
    }

    #[test]
    fn the_key_chosen_is_the_one_the_id_was_derived_from() {
        // Three decoy proofs come first. Taking the first key would be wrong,
        // and with one proof in the file nothing would have noticed.
        let key = b"the real developer key, DER in real life".to_vec();
        let crx = crx_with(&key, b"not a zip", 3);
        let parsed = parse(&crx).unwrap();
        assert_eq!(parsed.public_key, key);
        assert_eq!(parsed.id, id_from_key(&key));
    }

    #[test]
    fn a_package_whose_key_does_not_match_its_id_is_refused() {
        let mut crx = crx_with(b"key", b"zip", 0);
        // Corrupt the last byte of the crx_id.
        let n = crx.len();
        let at = n - b"zip".len() - 1;
        crx[at] ^= 0xff;
        let err = parse(&crx).unwrap_err().to_string();
        assert!(err.contains("hashes to its own crx_id"), "{err}");
    }

    #[test]
    fn things_that_are_not_crx3_say_so() {
        assert!(parse(b"too short").unwrap_err().to_string().contains("Cr24"));

        let mut two = Vec::new();
        two.extend_from_slice(MAGIC);
        two.extend_from_slice(&2u32.to_le_bytes());
        two.extend_from_slice(&0u32.to_le_bytes());
        two.extend_from_slice(&[0; 8]);
        let err = parse(&two).unwrap_err().to_string();
        assert!(err.contains("version 2"), "{err}");

        let mut lying = Vec::new();
        lying.extend_from_slice(MAGIC);
        lying.extend_from_slice(&3u32.to_le_bytes());
        lying.extend_from_slice(&9999u32.to_le_bytes());
        lying.extend_from_slice(&[0; 8]);
        assert!(parse(&lying).unwrap_err().to_string().contains("past the end"));
    }

    /// The behaviour the module exists for, end to end.
    #[test]
    fn installing_pins_the_id_to_the_key_and_not_to_the_path() {
        let key = b"a developer public key".to_vec();
        let manifest = br#"{"manifest_version":3,"name":"Test","version":"1.2.3"}"#;
        let crx = crx_with(&key, &stored_zip("manifest.json", manifest), 1);

        let root = scratch("install");
        // Two different paths — which is exactly what a clone produces.
        let a = root.join("profileA/ublock");
        let b = root.join("profileB/ublock");

        let one = install(&crx, &a).unwrap();
        let two = install(&crx, &b).unwrap();

        assert_eq!(one.id, two.id, "the same extension in two profiles must be one identity");
        assert_eq!(one.id, id_from_key(&key));
        assert_eq!(one.name, "Test");
        assert_eq!(one.version, "1.2.3");

        // And the key is in the manifest, which is the mechanism.
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(a.join("manifest.json")).unwrap()).unwrap();
        let stored = written.get("key").and_then(|v| v.as_str()).unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD.decode(stored).unwrap(),
            key
        );
        // Nothing else was disturbed.
        assert_eq!(written.get("name").unwrap(), "Test");
        assert_eq!(written.get("manifest_version").unwrap(), 3);

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn listing_reports_the_id_from_the_key_rather_than_the_directory_name() {
        let key = b"another key".to_vec();
        let manifest = br#"{"manifest_version":3,"name":"Listed","version":"9"}"#;
        let crx = crx_with(&key, &stored_zip("manifest.json", manifest), 0);

        let root = scratch("list");
        // A directory named nothing like the id.
        install(&crx, &root.join("whatever-the-operator-called-it")).unwrap();

        let found = installed(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, id_from_key(&key));
        assert_eq!(found[0].name, "Listed");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reinstalling_replaces_rather_than_merges() {
        let key = b"k".to_vec();
        let crx = crx_with(
            &key,
            &stored_zip("manifest.json", br#"{"name":"A","version":"1"}"#),
            0,
        );
        let root = scratch("replace");
        let dir = root.join("ext");
        install(&crx, &dir).unwrap();
        std::fs::write(dir.join("leftover.js"), "stale").unwrap();

        install(&crx, &dir).unwrap();
        assert!(!dir.join("leftover.js").exists(), "a stale file survived a reinstall");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The reason the tar shell-out was replaced rather than patched. An
    /// external unpacker does not check where entries land, and a .crx comes
    /// from whoever the operator downloaded it from.
    #[test]
    fn an_archive_entry_cannot_climb_out_of_the_extension_directory() {
        let root = scratch("escape");
        let dir = root.join("ext");
        std::fs::create_dir_all(&dir).unwrap();

        for evil in ["../../../../tmp/pwned", "/etc/passwd", "a/../../b"] {
            let err = super::unzip(&stored_zip(evil, b"x"), &dir).unwrap_err().to_string();
            assert!(
                err.contains("climbs out") || err.contains("absolute"),
                "{evil} gave {err:?}"
            );
        }
        // And the ordinary case still works, so the guard is not just refusing
        // everything.
        super::unzip(&stored_zip("sub/dir/file.js", b"ok"), &dir).unwrap();
        assert_eq!(std::fs::read(dir.join("sub/dir/file.js")).unwrap(), b"ok");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_package_with_no_manifest_at_the_top_is_refused() {
        let crx = crx_with(b"k", &stored_zip("src/manifest.json", b"{}"), 0);
        let root = scratch("nomanifest");
        let err = install(&crx, &root.join("ext")).unwrap_err().to_string();
        assert!(err.contains("manifest.json"), "{err}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
