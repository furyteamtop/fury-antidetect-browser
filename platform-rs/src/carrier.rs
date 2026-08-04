// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! How a persona reaches the browser without being written down.
//!
//! The core is told what to claim about itself — screen size, fonts, timezone,
//! the scrollbar width, forty other things — and that description must not
//! travel in the command line. `ps` on macOS shows every argument of every
//! process on the machine to any user; Task Manager and WMI do the same on
//! Windows. A persona in argv is a persona that any other program can read, and
//! for a product whose whole claim is that the browser is not distinguishable,
//! leaving the answer sheet in a place the page's own helper process can read
//! would be an odd way to start.
//!
//! So the config travels as an OPEN FILE the child inherits, and only the slot
//! it lands in is visible in argv.
//!
//! On Unix the slot is a descriptor number. The file is created 0600, unlinked
//! before anything is written to it, and `dup2`ed onto fd 3 in the forked child
//! before exec. There is no name at any point, so there is no window in which
//! another process could open it.
//!
//! On Windows the slot is a HANDLE value, and the three differences are worth
//! stating rather than glossing:
//!
//!   - There is no unlink-while-open. The name stays in `%TEMP%` until the last
//!     handle closes. The share mode is 0, which is the compensation: while the
//!     agent holds it, an attempt to open that name fails for EVERYONE,
//!     including the agent. An inherited handle is the same handle rather than
//!     a fresh open, so the child is unaffected.
//!
//!   - The handle is made inheritable at creation and `std::process::Command`
//!     passes `bInheritHandles = TRUE`, which is the classic all-inheritable-
//!     handles rule. It follows that any OTHER process spawned from this
//!     process while a carrier is alive also receives it. Carriers are created
//!     immediately before `spawn` and dropped immediately after, which makes
//!     the window short, and everything spawned in it is a browser this agent
//!     launched. It is not nothing, and it is written here so the next person
//!     to widen that window sees why it was narrow.
//!
//!   - If a future Rust ever restricts inheritance to an explicit handle list,
//!     this stops working. It stops LOUDLY: the core aborts when the slot it is
//!     given does not hold a readable config, so the failure is a browser that
//!     refuses to start rather than a browser that quietly has no persona.
//!     `tools/verify-windows.ps1` checks it directly, because "loudly" is a
//!     claim about somebody else's code.
//!
//! The core side is patch 0001 for the config and 0110 for the OS-crypt key.
//! Each reads `--<stem>-fd` on Unix and `--<stem>-file-handle` on Windows. The
//! Windows branch of each is written and applies; neither has been run, because
//! there is no Windows core yet. See `switch` for the name collision that
//! reading those patches turned up.

use std::io;

/// An open file holding bytes for the child, and the slot it will arrive on.
pub struct Carrier(Inner);

#[cfg(unix)]
struct Inner {
    file: std::fs::File,
    fd: i32,
}

#[cfg(windows)]
struct Inner {
    file: std::fs::File,
}

/// Distinct names within a process; the pid separates processes.
static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn unique_name() -> String {
    format!(
        "fury-fp-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

impl Carrier {
    /// Puts `bytes` where the next spawned child can read them.
    ///
    /// `unix_fd` is the descriptor the child will find it on — 3 for the
    /// config, 4 for the OS-crypt key. It is ignored on Windows, where the slot
    /// is a handle value the kernel picks.
    pub fn new(bytes: &[u8], unix_fd: i32) -> io::Result<Carrier> {
        #[cfg(unix)]
        {
            use std::io::{Seek, Write};
            use std::os::unix::fs::OpenOptionsExt;

            let path = std::env::temp_dir().join(unique_name());
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)?;
            // Before the write, not after: between create and unlink the file
            // is empty, so even a process that wins that race reads nothing.
            std::fs::remove_file(&path)?;
            file.write_all(bytes)?;
            file.rewind()?;
            Ok(Carrier(Inner { file, fd: unix_fd }))
        }

        #[cfg(windows)]
        {
            let _ = unix_fd;
            Ok(Carrier(Inner {
                file: win::anonymous_file(bytes)?,
            }))
        }
    }

    /// The switch that tells the core where to look.
    ///
    /// `stem` is the family — `"fury-fp"` yields `--fury-fp-fd=3` on Unix and
    /// `--fury-fp-file-handle=1234` on Windows.
    ///
    /// Three names for two things, and the third one is not tidiness. The core
    /// ALREADY has a `--fury-fp-handle`: patch 0001 uses it to hand children a
    /// read-only shared memory region, which is a completely different object
    /// from the file this carries. Had the carrier reused that name, the agent
    /// would have launched the browser with `--fury-fp-handle=<a file>`, the
    /// browser would have ignored it — its own branch reads `--fury-fp-fd` and
    /// nothing else — and then appended `--fury-fp-handle=<a region>` for each
    /// child on top of a switch that was already set.
    ///
    /// The visible symptom would have been a browser with no persona at all on
    /// Windows and none of the three places involved reporting anything wrong.
    /// Caught by reading patch 0001 while writing the Windows branch for it,
    /// which is the only reason it is a comment rather than a week.
    pub fn switch(&self, stem: &str) -> String {
        #[cfg(unix)]
        {
            format!("--{stem}-fd={}", self.0.fd)
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            format!("--{stem}-file-handle={}", self.0.file.as_raw_handle() as usize)
        }
    }

    /// Arranges for the child to receive it. Call before `spawn`.
    pub fn attach(&self, cmd: &mut std::process::Command) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::process::CommandExt;

            let src = self.0.file.as_raw_fd();
            let target = self.0.fd;
            // SAFETY: this closure runs in the forked child before exec, so it
            // may only call async-signal-safe functions. dup2 is one. It also
            // clears FD_CLOEXEC on the new descriptor, which is exactly why the
            // descriptor survives the exec.
            unsafe {
                cmd.pre_exec(move || {
                    if libc::dup2(src, target) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }

        #[cfg(windows)]
        {
            // Nothing to do: the handle was created inheritable, and
            // `std::process::Command` passes bInheritHandles = TRUE. Kept as a
            // call site so the launcher reads the same on both platforms and so
            // there is somewhere obvious to put an explicit handle list if Rust
            // ever needs one.
            let _ = cmd;
        }
    }
}

#[cfg(windows)]
mod win {
    use std::io;
    use std::ptr;

    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, CREATE_NEW, DELETE, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE,
    };

    /// A file that exists only as a handle, as nearly as Windows allows.
    pub fn anonymous_file(bytes: &[u8]) -> io::Result<std::fs::File> {
        use std::io::{Seek, Write};
        use std::os::windows::io::FromRawHandle;

        let path = std::env::temp_dir().join(super::unique_name());
        let wide: Vec<u16> = path
            .display()
            .to_string()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        // Owner-only, and inheritable — the two properties that make this
        // usable as a carrier at all.
        let security = crate::perms::OwnerOnly::new()?;
        let mut attrs = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: security.descriptor_ptr(),
            bInheritHandle: 1,
        };

        // SAFETY: `wide` is NUL-terminated and outlives the call; `attrs` and
        // the descriptor it points at outlive it too.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE | DELETE,
                // Share mode 0. While this handle is open, opening the name
                // fails for every process on the machine — which is how the
                // absence of unlink-while-open is answered. The child does not
                // open the name; it inherits the handle.
                0,
                &mut attrs,
                // CREATE_NEW so a name that somehow already exists is an error
                // rather than a file somebody else prepared.
                CREATE_NEW,
                FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: a valid handle from CreateFileW, and ownership moves here.
        let mut file = unsafe { std::fs::File::from_raw_handle(handle as _) };
        file.write_all(bytes)?;
        file.rewind()?;
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn the_bytes_are_there_and_rewound() {
        let carrier = Carrier::new(b"{\"schema_version\":1}", 3).unwrap();
        let mut got = String::new();
        (&carrier.0.file).read_to_string(&mut got).unwrap();
        assert_eq!(got, "{\"schema_version\":1}");
    }

    #[test]
    fn the_switch_names_the_slot() {
        let carrier = Carrier::new(b"x", 3).unwrap();
        let switch = carrier.switch("fury-fp");
        assert!(switch.starts_with("--fury-fp-"), "{switch}");
        let (_, slot) = switch.split_once('=').unwrap();
        assert!(!slot.is_empty(), "{switch}");

        #[cfg(unix)]
        assert_eq!(switch, "--fury-fp-fd=3");
        #[cfg(windows)]
        assert!(switch.starts_with("--fury-fp-file-handle="), "{switch}");
    }

    /// The collision that was nearly shipped. `--fury-fp-handle` belongs to
    /// patch 0001's browser-to-child shared memory region; a carrier that took
    /// that name would have been silently ignored by the browser.
    #[test]
    fn the_carrier_does_not_take_the_shared_memory_switch() {
        let switch = Carrier::new(b"x", 3).unwrap().switch("fury-fp");
        let (name, _) = switch.split_once('=').unwrap();
        assert_ne!(
            name, "--fury-fp-handle",
            "that name is patch 0001's child shared-memory switch"
        );
    }

    /// Two carriers alive at once must not be the same file — the config and
    /// the OS-crypt key are, and the first version of this used a fixed name.
    #[test]
    fn two_carriers_hold_different_bytes() {
        let a = Carrier::new(b"config", 3).unwrap();
        let b = Carrier::new(b"key-material", 4).unwrap();
        let mut sa = String::new();
        let mut sb = String::new();
        (&a.0.file).read_to_string(&mut sa).unwrap();
        (&b.0.file).read_to_string(&mut sb).unwrap();
        assert_eq!(sa, "config");
        assert_eq!(sb, "key-material");
    }

    /// What the whole module is for: the bytes must not be in argv.
    #[test]
    fn the_switch_does_not_contain_the_payload() {
        let carrier = Carrier::new(b"timezone-and-fonts-and-everything", 3).unwrap();
        assert!(!carrier.switch("fury-fp").contains("timezone"));
    }
}
