// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Asking a browser to close, rather than making it stop.
//!
//! The difference is a session. Chromium writes its cookie jar, its local
//! storage and its session file on the way out; a process that is killed
//! outright leaves whatever it last happened to flush. That was measured, on
//! macOS, and it was not subtle: a profile that had two cookies written over
//! CDP and was then closed exported zero of them afterwards. Every local
//! profile was quietly losing the tail of every session.
//!
//! `std::process::Child::kill` is `SIGKILL` on Unix and `TerminateProcess` on
//! Windows. Neither asks. So the agent asks first, waits, and only then kills —
//! and "asks" is spelled differently on the two systems:
//!
//!   Unix    — `SIGTERM`. Chromium installs a handler and runs its normal
//!             shutdown.
//!
//!   Windows — there are no signals. `TerminateProcess` is what `kill` maps to,
//!             and `GenerateConsoleCtrlEvent` does not reach a GUI process. The
//!             equivalent is `WM_CLOSE` posted to the browser's top-level
//!             window, which is what clicking the close button sends and what
//!             `taskkill` without `/F` does. Chromium treats it as a request to
//!             quit and takes the same path it takes for a menu Quit.
//!
//! This is the difference that a `#[cfg(unix)]` around the SIGTERM would have
//! hidden: on Windows the call would simply not happen, the wait loop would
//! time out, the kill would land, and profiles would lose their last session
//! with nothing anywhere saying why. It compiles either way, which is why it is
//! here as a function that both platforms have to implement rather than a
//! branch one platform can omit.
//!
//! A browser with no window — headless, or one that has not finished starting —
//! cannot be asked. `false` comes back and the caller falls through to its
//! timeout and its kill, which is the honest outcome rather than a wait that
//! was never going to end differently.

/// Asks a child to shut down cleanly. Returns whether the request was delivered.
///
/// Not a promise that it exited: the caller still waits, and still kills if the
/// wait runs out.
pub fn ask_to_close(child: &std::process::Child) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill(2) with a pid this process owns. The child has not been
        // reaped — `Child` is still alive — so the pid cannot have been reused.
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) == 0 }
    }

    #[cfg(windows)]
    {
        win::post_close(child.id())
    }
}

#[cfg(windows)]
mod win {
    // windows-sys 0.61 dropped the BOOL/TRUE aliases; the callback signature
    // is now plainly i32. Named here rather than discovered on a rented
    // machine.
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
    };

    struct Search {
        pid: u32,
        posted: bool,
    }

    /// Posts WM_CLOSE to every visible top-level window belonging to `pid`.
    ///
    /// Every one rather than the first: Chromium's browser process owns a
    /// window per open browser window, and closing one of three leaves the
    /// other two — and the wait loop then times out and kills, losing exactly
    /// what this was meant to save.
    ///
    /// Visible only. Chromium keeps hidden message-only windows for its own
    /// plumbing, and WM_CLOSE to one of those is either ignored or worse.
    unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> i32 {
        let search = unsafe { &mut *(param as *mut Search) };
        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == search.pid && unsafe { IsWindowVisible(hwnd) } != 0 {
            if unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) } != 0 {
                search.posted = true;
            }
        }
        // Keep enumerating. Returning FALSE here would stop at the first
        // window, which is the bug described above.
        1
    }

    pub fn post_close(pid: u32) -> bool {
        let mut search = Search { pid, posted: false };
        // SAFETY: `visit` is a valid callback and `search` outlives the call —
        // EnumWindows is synchronous and does not retain the pointer.
        unsafe { EnumWindows(Some(visit), &mut search as *mut Search as isize) };
        search.posted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Closing must be a request the child can act on, not a kill.
    ///
    /// A shell that traps and writes a file is the smallest thing that can tell
    /// the two apart: SIGKILL leaves no file, and a passing test therefore
    /// proves the signal arrived and the handler ran.
    #[cfg(unix)]
    #[test]
    fn a_child_gets_a_chance_to_clean_up() {
        let marker = std::env::temp_dir().join(format!("fury-term-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);

        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "trap 'echo tidied > {}; exit 0' TERM; while true; do sleep 0.05; done",
                marker.display()
            ))
            .spawn()
            .unwrap();

        // The trap has to be installed before the signal, or the shell dies on
        // the default action and the test passes or fails by timing.
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert!(ask_to_close(&child), "the request was not delivered");

        for _ in 0..50 {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = child.kill();
        let _ = child.wait();

        assert_eq!(
            std::fs::read_to_string(&marker).unwrap_or_default().trim(),
            "tidied",
            "the child was killed rather than asked — this is the bug that lost \
             the tail of every session"
        );
        let _ = std::fs::remove_file(&marker);
    }

    /// A process with nothing to ask must say so rather than claim success,
    /// because the caller's fallback depends on the answer.
    #[test]
    fn a_child_that_cannot_be_asked_reports_false_or_exits() {
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" })
            .arg(if cfg!(windows) { "/c" } else { "-c" })
            .arg("exit 0")
            .spawn()
            .unwrap();
        let _ = child.wait();
        // On Unix the pid is still un-reused and kill() succeeds against the
        // zombie, so this asserts only that the call is safe to make on a
        // process that is already gone — which is exactly when `stop` races it.
        let _ = ask_to_close(&child);
    }
}
