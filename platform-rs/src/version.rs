// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! What version is this binary, asked without running it.
//!
//! Only Windows has this, and only because Windows made asking the obvious way
//! impossible.
//!
//! `install-core` proves a freshly unpacked core is usable by running it with
//! `--version` and reading what it says. On macOS that is exactly what happens:
//! the bundle prints one line and exits, and a core with a missing framework or
//! a signature the system rejects fails there, in two seconds, next to the
//! command that caused it.
//!
//! On Windows the same call starts a BROWSER. `--version` is not a fast path in
//! chrome.exe on this platform: the process goes all the way through startup,
//! opens a window, takes the process singleton, and never exits — so
//! `Command::output()`, which waits for the process to end, waits forever. An
//! install that hangs at the last step, with a browser window nobody asked for
//! sitting on top of it.
//!
//! Found on the build server on 16.08.2026, and found only because a browser was
//! ALREADY running there and the second one failed loudly instead of hanging:
//!
//!   ERROR:chrome\browser\process_singleton_win.cc:410]
//!   Lock file can not be created! Error code: 32
//!
//! Error 32 is ERROR_SHARING_VIOLATION. On a clean machine there is no first
//! browser, nothing collides, and the hang is silent — which is the version of
//! this bug that would have reached users.
//!
//! So Windows reads the version out of the file's VERSIONINFO resource instead.
//! Every Chromium binary carries one; it is what Explorer shows in Properties →
//! Details, and what the installer reads.
//!
//! WHAT THIS DOES NOT PROVE, said plainly because the macOS path proves more and
//! the two are called from the same place: executing a binary proves it can
//! load. Reading a resource proves the file is a well-formed PE that claims a
//! version. A core whose chrome.dll is missing, or whose side-by-side manifest
//! is absent — a real failure, met on this port already — still answers this
//! call correctly and fails later, at first launch. The alternative is an
//! install that hangs, and a wrong version number is not on offer either way.

#[cfg(windows)]
mod imp {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO,
    };

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        s.encode_wide().chain(std::iter::once(0)).collect()
    }

    /// The four-part file version of a PE binary: "150.0.7871.187".
    pub fn file_version(path: &Path) -> io::Result<String> {
        let path_w = wide(path.as_os_str());

        // SAFETY: path_w is NUL-terminated and outlives every call below. The
        // buffer handed to GetFileVersionInfoW is exactly the size that
        // GetFileVersionInfoSizeW asked for, and VerQueryValueW returns a
        // pointer INTO that buffer — which is why `buf` is still alive when the
        // struct is read, and why the string is built before it is dropped.
        unsafe {
            let mut ignored = 0u32;
            let size = GetFileVersionInfoSizeW(path_w.as_ptr(), &mut ignored);
            if size == 0 {
                return Err(io::Error::other(format!(
                    "{} has no version resource - it is not a Chromium binary, \
                     or the file is truncated",
                    path.display()
                )));
            }

            let mut buf = vec![0u8; size as usize];
            if GetFileVersionInfoW(path_w.as_ptr(), 0, size, buf.as_mut_ptr().cast()) == 0 {
                return Err(io::Error::other(format!(
                    "could not read the version resource of {}",
                    path.display()
                )));
            }

            // "\" is the root of the resource, where the fixed-size block lives.
            let root = wide(std::ffi::OsStr::new("\\"));
            let mut value: *mut core::ffi::c_void = core::ptr::null_mut();
            let mut len = 0u32;
            if VerQueryValueW(buf.as_ptr().cast(), root.as_ptr(), &mut value, &mut len) == 0
                || value.is_null()
                || (len as usize) < core::mem::size_of::<VS_FIXEDFILEINFO>()
            {
                return Err(io::Error::other(format!(
                    "the version resource of {} has no fixed-info block",
                    path.display()
                )));
            }

            let info = &*(value as *const VS_FIXEDFILEINFO);
            Ok(format!(
                "{}.{}.{}.{}",
                info.dwFileVersionMS >> 16,
                info.dwFileVersionMS & 0xFFFF,
                info.dwFileVersionLS >> 16,
                info.dwFileVersionLS & 0xFFFF,
            ))
        }
    }
}

#[cfg(windows)]
pub use imp::file_version;
