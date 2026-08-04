// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! The parts of Fury that differ between operating systems.
//!
//! Everything here was in the agent until Windows came along, and moving it out
//! was not tidying. It was the only way to check it.
//!
//! `cargo check -p fury-agent --target x86_64-pc-windows-msvc` does not run on
//! a Mac. It fails in `ring`'s build script, which wants a Windows C toolchain,
//! and `ring` arrives through both sqlx and reqwest, so there is no feature
//! flag that removes it. The consequence is that the riskiest code in the port
//! — named pipes, security descriptors, handle inheritance, three unsafe FFI
//! calls — would have been written blind and first compiled on a rented machine
//! at an hourly rate.
//!
//! So it lives in a crate whose dependency list is short enough to cross-check:
//! tokio, libc on Unix, windows-sys on Windows. That crate does check for
//! `x86_64-pc-windows-msvc` on a Mac, and CI checks it on every commit, so a
//! typo in an FFI signature is a red build here rather than an hour of somebody
//! else's rented Windows.
//!
//! What cross-checking buys and what it does not:
//!
//!   it buys — signatures, argument counts, pointer mutability, feature gates,
//!             the tokio API being the shape the code assumes. Every mistake
//!             this repository has actually made in FFI has been one of these.
//!
//!   it does not buy — that a DACL denies what it is meant to deny, that a
//!             handle survives `CreateProcess`, that a pipe is not squatted.
//!             Those need a running Windows, and `tools/verify-windows.ps1`
//!             is what runs there.
//!
//! Not covered here, on purpose: the vault. `keyring` already speaks to the
//! Windows Credential Manager, so there is nothing to write and nothing to
//! check — see `agent/src/vault.rs`.
//!
//! Linux is not a target. Nothing in this crate is macOS-specific — the Unix
//! side is plain POSIX and would work — but the project builds for macOS and
//! Windows and says so, and a third platform nobody tests is a claim rather
//! than a port.

pub mod carrier;
pub mod dirs;
pub mod ipc;
pub mod perms;
pub mod process;
pub mod tree;

pub use carrier::Carrier;
pub use dirs::{data_dir, ipc_endpoint, short_tag};
pub use ipc::{Endpoint, Listener, Stream};
pub use process::ask_to_close;
pub use tree::copy_tree;
