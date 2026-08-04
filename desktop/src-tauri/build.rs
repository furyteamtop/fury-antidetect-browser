// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors
//
// Why tauri.conf.json builds no .dmg
//
// A release ships .tar.xz from tools/release/package.sh, so the disk image was
// a build artifact nobody used — and it was not free. Every dmg build mounts a
// volume to lay the window out and unmounts it, and macOS registers the
// application inside that volume and never forgets. Thirty-nine dead
// /Volumes/dmg.*/Fury.app entries had accumulated on the development machine,
// every one of them offered in the Applications view as something you could
// apparently launch. Not building a dmg stops it at the source.
//
// The Windows "nsis" target stays: there is no equivalent problem there and an
// installer is what people expect.
//
// The note lives here because tauri.conf.json is validated against a schema
// that rejects any key it does not know, including a comment-shaped one.

/// Pack the server kit into the binary.
///
/// The self-hosting instructions used to begin "from a clone of the
/// repository", which is a thing somebody holding a .app does not have. What
/// the server is actually built from is 410 KB — server/, shared-rs/, shared/,
/// deploy/ and the workspace manifests — so the application can simply carry it
/// and hand it over. No clone, no network, and no chance of the kit drifting
/// from the app that wrote it, which a download would allow.
///
/// Everything else in the repository is a 39 GB Chromium checkout and this
/// desktop app, neither of which has any business on a server. The exclusions
/// mirror deploy/push.sh, which is the script this kit exists to let somebody
/// run.

fn pack_server_kit() {
    use std::path::Path;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("server-kit.tar.gz");

    let file = std::fs::File::create(&out).expect("create server-kit.tar.gz");
    let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);

    for dir in ["server", "shared-rs", "shared", "deploy"] {
        let path = root.join(dir);
        assert!(path.is_dir(), "{dir} is missing — the server kit would ship incomplete");
        append_without_target(&mut tar, &path, dir);
        println!("cargo:rerun-if-changed=../../{dir}");
    }
    // The workspace manifest is deliberately NOT here, and the omission took a
    // test to find. deploy/server-install.sh writes its own at line 102 —
    // members = ["server", "shared-rs"] — because the repository's lists agent,
    // desktop/src-tauri and tools/detect-suite, none of which travel and none of
    // which belong on a server. Shipping the repository's manifest would put a
    // file in the kit that cargo refuses to read until the installer replaces
    // it, which is a confusing thing to hand somebody. This set is exactly what
    // deploy/push.sh sends, and that is the set known to install.
    tar.into_inner().expect("finish tar").finish().expect("finish gzip");
}

fn append_without_target<W: std::io::Write>(
    tar: &mut tar::Builder<W>,
    dir: &std::path::Path,
    prefix: &str,
) {
    for entry in std::fs::read_dir(dir).expect("read kit dir").flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // A build directory is not source, and shared-rs/target alone is larger
        // than everything else here put together.
        if name == "target" || name.starts_with('.') {
            continue;
        }
        let rel = format!("{prefix}/{name}");
        if path.is_dir() {
            append_without_target(tar, &path, &rel);
        } else {
            tar.append_path_with_name(&path, &rel).expect("append file");
        }
    }
}

fn main() {
    pack_server_kit();
    tauri_build::build()
}
