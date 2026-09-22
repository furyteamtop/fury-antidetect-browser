// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! Where the agent's own account of itself goes.
//!
//! WHY THIS EXISTS. The desktop application starts the agent with all three
//! standard streams on null (desktop/src-tauri/src/agent.rs) — it has to, or
//! Windows draws a console window over the application. Until this file, that
//! meant every line the agent ever wrote went nowhere on the machines that
//! matter: a person whose core would not install had a screenshot and nothing
//! else, and twice on 22.09.2026 that was the whole of the evidence.
//!
//! WHAT IT IS NOT. Not telemetry: nothing here leaves the machine, and there is
//! no code in this program that would send it. The file sits in the data
//! directory, which is owner-only and already holds the profile database —
//! something anyone who can read the log could read instead, and more of it.
//!
//! WHAT IT COSTS, and it is a real cost for this product: a persistent record
//! of when this machine ran profiles. It is capped and it is coarse — `info`,
//! which is "downloading the core", "the profile closed", not a trace of
//! browsing — and it can be deleted at any time; the agent makes a new one and
//! carries on. FURY_LOG=off turns it off entirely.

use std::path::PathBuf;

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Two of these: the one being written and the one before it.
///
/// Four megabytes is thousands of lines at the level this logs, which covers
/// weeks of ordinary use and, more to the point, covers the several days
/// between somebody hitting a problem and somebody asking them for the file.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Starts logging to stderr and, unless turned off, to a file.
///
/// Returns the file it will write to, so the caller can say where it is.
pub fn start() -> Option<PathBuf> {
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "fury_agent=info".into())
    };

    // A terminal still gets what it always got. The CLI is used by people who
    // are watching it run, and a command whose output moved into a file it does
    // not mention would be worse than one that never had a file.
    let to_stderr = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    let path = match std::env::var("FURY_LOG").as_deref() {
        Ok("off") => None,
        _ => open().ok(),
    };

    match path {
        Some(path) => {
            let for_writer = path.clone();
            tracing_subscriber::registry()
                .with(filter())
                .with(to_stderr)
                .with(
                    tracing_subscriber::fmt::layer()
                        // Colour codes in a file are noise in every reader that
                        // is not a terminal, and this file is read in Notepad.
                        .with_ansi(false)
                        // Opened per line rather than held: the agent logs a
                        // handful of lines a minute at this level, and a handle
                        // held for the life of the process is a file nobody can
                        // delete on Windows -- including the person clearing
                        // space, and including the rotation below.
                        .with_writer(move || {
                            std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&for_writer)
                                // A log that cannot be written must not stop the
                                // agent: a read-only disk is a bad day, not a
                                // reason to refuse to hold profiles.
                                .map(BoxedWrite::File)
                                .unwrap_or(BoxedWrite::Sink)
                        }),
                )
                .init();
            Some(path)
        }
        None => {
            tracing_subscriber::registry()
                .with(filter())
                .with(to_stderr)
                .init();
            None
        }
    }
}

/// Makes the directory, rotates what is there, and hands back the path.
fn open() -> std::io::Result<PathBuf> {
    let path = crate::paths::log_file();
    let dir = path.parent().expect("log_file always has a parent");
    std::fs::create_dir_all(dir)?;
    // The data directory is owner-only and this is inside it; said again here
    // because a directory created by this function on a machine where the data
    // directory already existed would otherwise inherit whatever the default is.
    let _ = fury_platform::perms::owner_only_dir(dir);

    // Rotation at startup rather than mid-run: the agent is long-lived but not
    // eternal, the check is one stat, and a rotation that can happen while a
    // line is being written is a race for no benefit.
    if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_BYTES {
        let previous = path.with_extension("log.1");
        let _ = std::fs::remove_file(&previous);
        let _ = std::fs::rename(&path, &previous);
    }
    Ok(path)
}

/// What the writer closure returns: the file, or somewhere to put the line when
/// there is no file to put it in.
enum BoxedWrite {
    File(std::fs::File),
    Sink,
}

impl std::io::Write for BoxedWrite {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::File(f) => f.write(buf),
            Self::Sink => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::File(f) => f.flush(),
            Self::Sink => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_that_has_grown_past_the_cap_is_kept_as_the_previous_one() {
        let _guard = crate::ENV.lock().unwrap_or_else(|e| e.into_inner());
        let home = std::env::temp_dir().join("fury-log-rotate-test");
        std::fs::remove_dir_all(&home).ok();
        // SAFETY: the mutex above is what makes this single-threaded.
        unsafe { std::env::set_var("FURY_HOME", &home) };

        // First run: the directory does not exist yet, which is the ordinary
        // case on a machine that has just installed the application.
        let path = open().expect("a log in a fresh data directory");
        assert_eq!(path, crate::paths::log_file());
        std::fs::write(&path, vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();

        let again = open().expect("a log beside a full one");
        let previous = path.with_extension("log.1");
        assert!(previous.exists(), "the full one is kept as .1");
        assert!(!again.exists() || again.metadata().unwrap().len() == 0,
                "and the next line starts a new file");

        // A third run must not lose the rotated copy to a second rotation of an
        // empty file, and must not stack .1.1 files either.
        let _ = open().expect("a third run");
        assert!(previous.exists());
        assert!(!previous.with_extension("log.1").exists());

        unsafe { std::env::remove_var("FURY_HOME") };
        std::fs::remove_dir_all(&home).ok();
    }
}
