// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! "Only this user, nobody else" — on two systems that mean different things by
//! it.
//!
//! Fury keeps live cookie jars, a bearer token for its own HTTP API, and the
//! sealed vault under one directory. On Unix that is `chmod 0700` and the story
//! ends; the mode is on the inode, the kernel enforces it, and a wrong answer
//! is visible in `ls -l`.
//!
//! Windows has no mode bits. A newly created file inherits its parent's ACL,
//! which under a user's profile is usually the user, SYSTEM and Administrators
//! — usually being the operative word, because it is whatever the parent had,
//! and a data directory relocated by `FURY_HOME` to a shared drive inherits
//! whatever THAT has. Nothing warns. The file is created, the agent works, and
//! the token that unlocks the local API is readable by anyone with access to
//! the share.
//!
//! So on Windows the ACL is written rather than inherited: one explicit
//! PROTECTED DACL granting this user's own SID and SYSTEM, and nothing else.
//! Protected matters as much as the entries — an unprotected DACL still lets
//! the parent's inheritable entries in, so a directory that grants Everyone
//! would put Everyone back on the file this code just locked down.
//!
//! Which SID is "this user" is asked of the process token rather than assumed.
//! `CREATOR OWNER` in an SDDL string is the tempting shortcut and it is wrong
//! here: the substitution it relies on happens for INHERITED entries, so on a
//! directly applied DACL it stays as the literal S-1-3-0 and grants nobody.
//! That is a mistake that looks like it works — the file gets an ACL, no call
//! fails — until somebody checks who can read it.

use std::io;
use std::path::Path;

// ---------------------------------------------------------------------------
// Unix
// ---------------------------------------------------------------------------

/// 0600, or the Windows equivalent.
#[cfg(unix)]
pub fn owner_only_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

/// 0700, or the Windows equivalent.
#[cfg(unix)]
pub fn owner_only_dir(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// Marks a file executable. A no-op where the concept does not exist.
#[cfg(unix)]
pub fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o111);
    std::fs::set_permissions(path, perms)
}

/// On Windows, whether a file may be executed is decided by its extension and
/// by SmartScreen, not by a permission bit. There is nothing to set.
#[cfg(windows)]
pub fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::io;
    use std::path::Path;
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetSecurityInfo, SetNamedSecurityInfoW, SE_FILE_OBJECT, SE_KERNEL_OBJECT,
        SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetTokenInformation, TokenUser, DACL_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    };
    // OpenProcessToken lives under Threading, not Security. Caught by the
    // cross-check rather than on a rented Windows machine, which is the whole
    // argument for this crate existing.
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error(what: &str) -> io::Error {
        let e = io::Error::last_os_error();
        io::Error::new(e.kind(), format!("{what}: {e}"))
    }

    /// This process's user SID, as bytes we own.
    ///
    /// Copied out of the token information buffer rather than borrowed from it:
    /// the buffer is a local `Vec` and the SID inside it is a pointer into that
    /// allocation, so keeping the pointer and dropping the Vec is exactly the
    /// use-after-free this returns a copy to avoid.
    fn current_user_sid() -> io::Result<Vec<u8>> {
        // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no
        // close; the token handle does, and is closed on every path below.
        let mut token: HANDLE = ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(last_error("opening this process's token"));
        }

        let mut needed: u32 = 0;
        // The documented two-call pattern: the first asks how big, and is
        // EXPECTED to fail with ERROR_INSUFFICIENT_BUFFER.
        unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
        let mut buf = vec![0u8; needed.max(1) as usize];
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenUser,
                buf.as_mut_ptr() as *mut c_void,
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            let e = last_error("reading the token's user");
            unsafe { CloseHandle(token) };
            return Err(e);
        }
        unsafe { CloseHandle(token) };

        // SAFETY: on success the buffer starts with a TOKEN_USER whose Sid
        // points inside it.
        let info = buf.as_ptr() as *const TOKEN_USER;
        let sid = unsafe { (*info).User.Sid };
        Ok(copy_sid(sid))
    }

    /// A SID is variable-length: 8 fixed bytes plus 4 per sub-authority, and
    /// the count is the second byte. `GetLengthSid` would also do it; this
    /// avoids one more import for arithmetic that is fixed by the format.
    fn copy_sid(sid: *mut c_void) -> Vec<u8> {
        let sub_authorities = unsafe { *(sid as *const u8).add(1) } as usize;
        let len = 8 + 4 * sub_authorities;
        unsafe { std::slice::from_raw_parts(sid as *const u8, len) }.to_vec()
    }

    fn sid_to_string(sid: *mut c_void) -> io::Result<String> {
        let mut out: *mut u16 = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(sid, &mut out) } == 0 {
            return Err(last_error("formatting a SID"));
        }
        let mut len = 0;
        while unsafe { *out.add(len) } != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out, len) });
        unsafe { LocalFree(out as HLOCAL) };
        Ok(s)
    }

    /// A security descriptor granting this user and SYSTEM, and refusing
    /// everything inherited.
    ///
    /// SYSTEM is in there so that backup, antivirus and the service control
    /// manager keep working. Administrators is deliberately NOT: an
    /// administrator can take ownership of anything on the machine and read it
    /// anyway, so granting it buys nothing and costs the ability to say plainly
    /// what the DACL means.
    pub struct OwnerOnly {
        descriptor: PSECURITY_DESCRIPTOR,
        sid: Vec<u8>,
        /// The SDDL used for files and directories, which needs inheritance
        /// flags the pipe's does not.
        sddl_inheritable: String,
    }

    // SAFETY: the descriptor is an immutable LocalAlloc block after
    // construction; nothing here mutates it, and the Windows APIs that read it
    // take a const pointer.
    unsafe impl Send for OwnerOnly {}
    unsafe impl Sync for OwnerOnly {}

    impl Drop for OwnerOnly {
        fn drop(&mut self) {
            unsafe { LocalFree(self.descriptor as HLOCAL) };
        }
    }

    fn descriptor_from(sddl: &str) -> io::Result<PSECURITY_DESCRIPTOR> {
        let mut psd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let ok = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(sddl).as_ptr(),
                SDDL_REVISION_1,
                &mut psd,
                ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(last_error(&format!("building a security descriptor from {sddl}")));
        }
        Ok(psd)
    }

    impl OwnerOnly {
        pub fn new() -> io::Result<OwnerOnly> {
            let sid = current_user_sid()?;
            let text = sid_to_string(sid.as_ptr() as *mut c_void)?;
            // D: the DACL. P: protected, so nothing is inherited into it.
            // A;;GA;;;<sid>: allow generic-all to that SID. SY: local SYSTEM.
            let sddl = format!("D:P(A;;GA;;;{text})(A;;GA;;;SY)");
            // OICI on each entry: object- and container-inherit, so a file
            // created inside the data directory later gets the same DACL
            // without this code having to walk the tree.
            let sddl_inheritable = format!("D:P(A;OICI;GA;;;{text})(A;OICI;GA;;;SY)");
            Ok(OwnerOnly {
                descriptor: descriptor_from(&sddl)?,
                sid,
                sddl_inheritable,
            })
        }

        /// One named-pipe server instance carrying this descriptor.
        ///
        /// `first` maps to `first_pipe_instance`, which makes creation fail if
        /// the name is already taken. True for the first instance and false for
        /// every replacement — passing true twice would make the second
        /// instance fail against the listener's own first one.
        pub fn create_pipe(
            &self,
            name: &str,
            first: bool,
        ) -> io::Result<tokio::net::windows::named_pipe::NamedPipeServer> {
            let mut attrs = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.descriptor,
                // A pipe handle must not travel into a browser process.
                bInheritHandle: 0,
            };
            let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
            options.first_pipe_instance(first);
            // SAFETY: `attrs` outlives the call, and the descriptor it points
            // at outlives `attrs` — both are owned here and the call does not
            // retain either.
            unsafe {
                options.create_with_security_attributes_raw(
                    name,
                    &mut attrs as *mut SECURITY_ATTRIBUTES as *mut c_void,
                )
            }
        }

        fn apply(&self, path: &Path) -> io::Result<()> {
            let psd = descriptor_from(&self.sddl_inheritable)?;
            // Pull the DACL back out of the descriptor. SetNamedSecurityInfoW
            // takes an ACL, not a descriptor, which is the one shape mismatch
            // in this file.
            let mut dacl_present = 0;
            let mut dacl: *mut windows_sys::Win32::Security::ACL = ptr::null_mut();
            let mut defaulted = 0;
            let ok = unsafe {
                windows_sys::Win32::Security::GetSecurityDescriptorDacl(
                    psd,
                    &mut dacl_present,
                    &mut dacl,
                    &mut defaulted,
                )
            };
            if ok == 0 {
                let e = last_error("reading back the DACL");
                unsafe { LocalFree(psd as HLOCAL) };
                return Err(e);
            }

            let mut wide_path = wide(&path.display().to_string());
            let status = unsafe {
                SetNamedSecurityInfoW(
                    wide_path.as_mut_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    dacl,
                    ptr::null_mut(),
                )
            };
            unsafe { LocalFree(psd as HLOCAL) };
            if status != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    format!(
                        "could not restrict {} to this user (error {status})",
                        path.display()
                    ),
                ));
            }
            Ok(())
        }

        pub fn sid(&self) -> &[u8] {
            &self.sid
        }

        /// For a `SECURITY_ATTRIBUTES` built elsewhere.
        ///
        /// The pointer is only valid while this `OwnerOnly` lives, which is why
        /// it is not a `Copy` type and why every caller keeps it in a local
        /// that outlives the call it hands the pointer to.
        pub fn descriptor_ptr(&self) -> PSECURITY_DESCRIPTOR {
            self.descriptor
        }
    }

    /// Restricts a path to this user, replacing whatever it inherited.
    pub fn owner_only(path: &Path) -> io::Result<()> {
        OwnerOnly::new()?.apply(path)
    }

    /// Hangs up unless the thing just opened belongs to us.
    ///
    /// The squatting check from `ipc`. A pipe name is machine-wide and
    /// guessable, so another account can create `\\.\pipe\fury-…` before the
    /// agent starts and answer the shell's questions. The owner of a kernel
    /// object is set at creation from the creator's token and cannot be forged
    /// by a caller who is not us, so comparing it to our own SID settles it.
    pub fn assert_owned_by_this_user<H: std::os::windows::io::AsRawHandle>(
        handle: &H,
    ) -> io::Result<()> {
        let mut owner: *mut c_void = ptr::null_mut();
        let mut psd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let status = unsafe {
            GetSecurityInfo(
                handle.as_raw_handle() as HANDLE,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut psd,
            )
        };
        if status != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!("could not read the owner of the agent's pipe (error {status})"),
            ));
        }

        let ours = current_user_sid()?;
        let same = unsafe { EqualSid(owner, ours.as_ptr() as *mut c_void) } != 0;
        unsafe { LocalFree(psd as HLOCAL) };

        if !same {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "something other than this user's agent is holding the Fury pipe — \
                 refusing to talk to it",
            ));
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use win::{assert_owned_by_this_user, OwnerOnly};

#[cfg(windows)]
pub fn owner_only_file(path: &Path) -> io::Result<()> {
    win::owner_only(path)
}

#[cfg(windows)]
pub fn owner_only_dir(path: &Path) -> io::Result<()> {
    win::owner_only(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_restricted_file_is_still_readable_by_us() {
        // The check that matters on both platforms and the one a wrong DACL
        // fails: locking a file down must not lock US out of it.
        let dir = std::env::temp_dir().join(format!("fury-perms-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        owner_only_dir(&dir).unwrap();

        let file = dir.join("token");
        std::fs::write(&file, b"secret").unwrap();
        owner_only_file(&file).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"secret");

        std::fs::write(&file, b"replaced").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"replaced");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn and_nobody_else_can_read_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("fury-perms2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("token");
        std::fs::write(&file, b"secret").unwrap();
        // Deliberately wrong first, so the assertion is about what the call
        // did rather than about what the umask happened to leave.
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o666)).unwrap();
        owner_only_file(&file).unwrap();
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got {mode:o}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
