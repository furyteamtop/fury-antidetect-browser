// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Copying a browser from one directory to another.
//!
//! A 544 MB tree, and the two systems need it done differently for a reason
//! that is not stylistic.
//!
//! On macOS the tree is an application bundle, and a bundle is built out of
//! symlinks: `Versions/Current` points at `Versions/150.0.7871.187`,
//! `Frameworks/…/Libraries` points into it, and the framework's top level is a
//! set of links standing in for the current version's contents. Follow them
//! instead of copying them and the result is a tree that is larger, that has
//! several copies of the same 300 MB framework, and that does not launch —
//! macOS checks a bundle's structure and a flattened one is not a bundle. The
//! executable bits go the same way. `cp -R` gets all of this right, and a
//! hand-written walk is exactly where it goes wrong, so on Unix this shells out.
//!
//! On Windows the tree is a directory: `chrome.exe`, a pile of DLLs, a
//! `Locales` folder. No symlinks — creating one needs a privilege ordinary
//! accounts do not have, so Chromium's Windows packaging does not use them. No
//! mode bits to preserve. There is also no `cp`, and the two candidates for
//! replacing it are both traps: `xcopy` is deprecated and prompts on ambiguity,
//! and `robocopy` returns 1 for success, 3 for "copied with extras" and 8 for
//! failure — an exit-code convention that reads as failure to every `status
//! .success()` ever written. So it is done in Rust.
//!
//! The Rust walk is compiled and tested on BOTH platforms even though only
//! Windows uses it. That is the point: a Windows-only implementation is one
//! nobody runs until Windows exists, and this one is exercised by the macOS
//! test run on every commit.

use std::io;
use std::path::Path;

/// Copies `src` into `into`, keeping whatever this platform needs kept.
pub fn copy_tree(src: &Path, into: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let status = std::process::Command::new("cp")
            .arg("-R")
            .arg(src)
            .arg(into)
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "cp -R could not copy {} into {}",
                src.display(),
                into.display()
            )));
        }
        Ok(())
    }

    #[cfg(windows)]
    {
        let target = match src.file_name() {
            Some(name) => into.join(name),
            None => into.to_path_buf(),
        };
        copy_tree_portable(src, &target)
    }
}

/// A plain recursive copy, `src` to `dst`, creating `dst`.
///
/// Not cfg'd out anywhere. It is what Windows uses and what the tests on every
/// platform exercise — see the module comment.
pub fn copy_tree_portable(src: &Path, dst: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;

    if meta.is_dir() {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_tree_portable(&entry.path(), &dst.join(entry.file_name()))?;
        }
        return Ok(());
    }

    if meta.file_type().is_symlink() {
        // Reachable on Unix when the tests run there, and left as an explicit
        // refusal rather than a silent follow: copying the TARGET of a link
        // is how a 544 MB bundle becomes 1.5 GB, and finding that out from a
        // disk-full error later is worse than finding it out here.
        return Err(io::Error::other(format!(
            "{} is a symlink, which this copy does not follow",
            src.display()
        )));
    }

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, dst)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("fury-tree-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_nested_tree_arrives_whole() {
        let root = scratch("whole");
        let src = root.join("core");
        std::fs::create_dir_all(src.join("Locales")).unwrap();
        std::fs::write(src.join("chrome.exe"), b"MZ").unwrap();
        std::fs::write(src.join("Locales/en-GB.pak"), b"pak").unwrap();

        let dst = root.join("out");
        copy_tree_portable(&src, &dst).unwrap();

        assert_eq!(std::fs::read(dst.join("chrome.exe")).unwrap(), b"MZ");
        assert_eq!(std::fs::read(dst.join("Locales/en-GB.pak")).unwrap(), b"pak");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// An empty directory in the tree must survive. Chromium ships some, and a
    /// walk that only creates parents-of-files silently drops them.
    #[test]
    fn an_empty_directory_survives() {
        let root = scratch("empty");
        let src = root.join("core");
        std::fs::create_dir_all(src.join("swiftshader")).unwrap();
        std::fs::write(src.join("chrome.exe"), b"MZ").unwrap();

        let dst = root.join("out");
        copy_tree_portable(&src, &dst).unwrap();
        assert!(dst.join("swiftshader").is_dir());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The macOS shape, checked through the real entry point: a framework
    /// bundle's symlinks have to come out the other side as symlinks.
    #[cfg(unix)]
    #[test]
    fn a_bundle_keeps_its_symlinks() {
        let root = scratch("bundle");
        let src = root.join("Fury.app");
        std::fs::create_dir_all(src.join("Versions/150")).unwrap();
        std::fs::write(src.join("Versions/150/Fury"), b"macho").unwrap();
        std::os::unix::fs::symlink("150", src.join("Versions/Current")).unwrap();

        let into = root.join("out");
        std::fs::create_dir_all(&into).unwrap();
        copy_tree(&src, &into).unwrap();

        let link = into.join("Fury.app/Versions/Current");
        assert!(
            std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
            "the link was followed — a flattened bundle does not launch"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_portable_walk_refuses_a_symlink_rather_than_following_it() {
        let root = scratch("refuse");
        let src = root.join("core");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("real"), b"x").unwrap();
        std::os::unix::fs::symlink("real", src.join("link")).unwrap();

        let err = copy_tree_portable(&src, &root.join("out")).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
