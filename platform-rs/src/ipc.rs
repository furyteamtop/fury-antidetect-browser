// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! The channel the desktop shell talks to the agent over.
//!
//! Not a TCP port, on either platform. The agent can launch browsers, read
//! cookie jars and decrypt bundles; a `127.0.0.1` listener is reachable from
//! any page the browser has open, and "but it is only localhost" has been the
//! preamble to a long list of vulnerabilities. So: a Unix socket on macOS, a
//! named pipe on Windows, and in both cases the operating system's own access
//! control is what stands between a web page and the vault.
//!
//! The two are not equivalent out of the box, and that is the whole reason this
//! module is longer than a type alias.
//!
//! A Unix socket inherits the umask and is created 0600 by an explicit chmod
//! (see `perms`), and the check is done by the kernel on connect. Simple, and
//! the file is either private or it is not.
//!
//! A named pipe has three differences, each of which has to be answered:
//!
//!   1. The DEFAULT DACL is not 0600. It comes from the process token and in
//!      practice grants the user, SYSTEM and Administrators — which is close
//!      enough that a wrong version would look right for years. It is set
//!      explicitly here, to this user's own SID and SYSTEM, so that what is
//!      granted is a decision rather than a default.
//!
//!   2. Pipe names live in ONE machine-wide namespace. `\\.\pipe\fury-…` is a
//!      name another account can create first, and then the shell connects to
//!      the squatter and believes its answers. Two halves to that: the server
//!      passes `first_pipe_instance(true)` so it refuses to be the second
//!      holder of the name rather than quietly sharing it, and the client reads
//!      the OWNER back off the pipe it just opened and hangs up unless it is
//!      us. The client half is the one that matters — a squatter who wins the
//!      race wins it before the agent ever runs, so the server has nothing to
//!      refuse.
//!
//!   3. There is no listening backlog. Each concurrent connection is a separate
//!      server instance that has to exist BEFORE a client asks for it, so
//!      `accept` creates the next instance as it hands the current one over. If
//!      it did not, the window between two clients would return
//!      ERROR_PIPE_BUSY, which the shell would report as "the agent is not
//!      running" — during a run where it plainly is.
//!
//! Everything here type-checks for `x86_64-pc-windows-msvc` from a Mac; see
//! `lib.rs` for what that does and does not prove. The DACL and the owner check
//! are behaviour, so they are in `tools/verify-windows.ps1`.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The address of the agent, in whatever form this platform uses.
///
/// Opaque on purpose. A caller that can see a `PathBuf` writes code that only
/// compiles on one platform, and the two callers here — the agent and the
/// desktop shell — have to agree on the address exactly. There is a test that
/// they do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint(EndpointInner);

#[cfg(unix)]
type EndpointInner = std::path::PathBuf;

#[cfg(windows)]
type EndpointInner = String;

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        #[cfg(unix)]
        {
            write!(f, "{}", self.0.display())
        }
        #[cfg(windows)]
        {
            f.write_str(&self.0)
        }
    }
}

/// Where the agent listens, for an installation identified by `tag`.
///
/// The tag separates installations, so a test run does not talk to the
/// developer's real agent — which it otherwise would, and would then launch
/// browsers in the developer's own profiles.
///
/// `FURY_SOCKET` overrides it, which is how the tests get their own.
///
/// On Unix the address is a path, and it is deliberately NOT under the data
/// directory. `sockaddr_un.sun_path` holds about 104 bytes on macOS and bind()
/// fails with "path must be shorter than SUN_LEN" — a message that sounds like
/// a programming error rather than "your home directory is deep". A long
/// username or a data directory under a build tree reaches it. So the socket
/// lives in the per-user temp directory, which is short by construction and, on
/// macOS, already private to the user (`/var/folders/…/T/`).
///
/// On Windows the address is a pipe name, and none of the above applies: the
/// limit is 256 characters, the location is a namespace rather than a
/// directory, and privacy comes from the DACL rather than from where it sits.
pub fn endpoint(tag: &str) -> Endpoint {
    if let Ok(explicit) = std::env::var("FURY_SOCKET") {
        if !explicit.is_empty() {
            return Endpoint(explicit.into());
        }
    }

    #[cfg(unix)]
    {
        Endpoint(std::env::temp_dir().join(format!("fury-{tag}.sock")))
    }

    #[cfg(windows)]
    {
        // The tag is in the name for the same reason as on Unix, and it is NOT
        // a security boundary: pipe names are guessable and the DACL is what
        // keeps other accounts out.
        Endpoint(format!(r"\\.\pipe\fury-{tag}"))
    }
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

/// One connection, either end.
///
/// On Windows the accepting and connecting ends are genuinely different types
/// (`NamedPipeServer` and `NamedPipeClient`), which is why this is an enum
/// there and a newtype on Unix.
pub struct Stream(StreamInner);

#[cfg(unix)]
struct StreamInner(tokio::net::UnixStream);

#[cfg(windows)]
enum StreamInner {
    Server(tokio::net::windows::named_pipe::NamedPipeServer),
    Client(tokio::net::windows::named_pipe::NamedPipeClient),
}

impl Stream {
    /// Connects to the agent.
    ///
    /// `ErrorKind::NotFound` means nothing is listening, on both platforms —
    /// the callers key off that to decide whether to start the agent, so it is
    /// part of the contract rather than an implementation detail. Windows
    /// reports a missing pipe as ERROR_FILE_NOT_FOUND, which maps to the same
    /// kind, so no translation is needed; the case that DOES need handling is
    /// ERROR_PIPE_BUSY, which means the agent is there but every instance is
    /// taken. That is a retry, not a failure.
    pub async fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
        #[cfg(unix)]
        {
            Ok(Stream(StreamInner(
                tokio::net::UnixStream::connect(&endpoint.0).await?,
            )))
        }

        #[cfg(windows)]
        {
            use std::time::Duration;
            use tokio::net::windows::named_pipe::ClientOptions;
            use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;

            // Bounded: an agent that is wedged has to surface as an error the
            // operator can act on, not as a window that never finishes loading.
            let mut waited = Duration::ZERO;
            let step = Duration::from_millis(50);
            let client = loop {
                match ClientOptions::new().open(&endpoint.0) {
                    Ok(c) => break c,
                    Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                        if waited >= Duration::from_secs(5) {
                            return Err(io::Error::new(
                                io::ErrorKind::WouldBlock,
                                "every agent connection is busy",
                            ));
                        }
                        tokio::time::sleep(step).await;
                        waited += step;
                    }
                    Err(e) => return Err(e),
                }
            };

            // The squatting check, and the reason this is not one line. See the
            // module comment: winning the name is a race that happens before
            // the agent runs, so refusing to be second is not enough on its
            // own — the client has to look at who it reached.
            crate::perms::assert_owned_by_this_user(&client)?;

            Ok(Stream(StreamInner::Client(client)))
        }
    }

    /// Splits into halves that can move to different tasks.
    ///
    /// `tokio::io::split` rather than `UnixStream::into_split`, so that one
    /// signature serves both platforms — named pipes have no `into_split`. The
    /// cost is a mutex per stream, which for a line-at-a-time protocol with one
    /// client is not a cost.
    #[allow(clippy::type_complexity)]
    pub fn into_split(self) -> (tokio::io::ReadHalf<Stream>, tokio::io::WriteHalf<Stream>) {
        tokio::io::split(self)
    }
}

macro_rules! dispatch {
    ($self:expr, $inner:ident => $body:expr) => {{
        #[cfg(unix)]
        {
            let $inner = &mut $self.0 .0;
            $body
        }
        #[cfg(windows)]
        {
            match &mut $self.0 {
                StreamInner::Server($inner) => $body,
                StreamInner::Client($inner) => $body,
            }
        }
    }};
}

impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        dispatch!(me, s => Pin::new(s).poll_read(cx, buf))
    }
}

impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        dispatch!(me, s => Pin::new(s).poll_write(cx, buf))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        dispatch!(me, s => Pin::new(s).poll_flush(cx))
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        dispatch!(me, s => Pin::new(s).poll_shutdown(cx))
    }
}

// ---------------------------------------------------------------------------
// Listener
// ---------------------------------------------------------------------------

/// The agent's end.
pub struct Listener {
    endpoint: Endpoint,
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    #[cfg(windows)]
    next: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    #[cfg(windows)]
    security: crate::perms::OwnerOnly,
}

impl Listener {
    /// Starts listening, refusing to share the address with anything else.
    ///
    /// A stale Unix socket file is removed first — the file outlives the
    /// process that made it, and an agent that was killed leaves one behind, so
    /// without this the agent never starts again after a crash. The caller is
    /// expected to have checked that nothing answers on it; see the agent's
    /// `serve`.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Listener> {
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&endpoint.0);
            let inner = tokio::net::UnixListener::bind(&endpoint.0)?;
            // After bind, not before: the path does not exist until bind makes
            // it, and a chmod on a path that is about to be created is a race
            // with nothing in it.
            crate::perms::owner_only_file(&endpoint.0)?;
            Ok(Listener { endpoint: endpoint.clone(), inner })
        }

        #[cfg(windows)]
        {
            let security = crate::perms::OwnerOnly::new()?;
            // first_pipe_instance: fail rather than join. If something already
            // holds this name the honest outcome is an error naming the
            // address, not an agent that shares its channel with a stranger.
            let next = security.create_pipe(&endpoint.0, true)?;
            Ok(Listener {
                endpoint: endpoint.clone(),
                next: Some(next),
                security,
            })
        }
    }

    /// The address this is listening on, for log lines and error messages.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Waits for the next client.
    pub async fn accept(&mut self) -> io::Result<Stream> {
        #[cfg(unix)]
        {
            let (stream, _addr) = self.inner.accept().await?;
            Ok(Stream(StreamInner(stream)))
        }

        #[cfg(windows)]
        {
            let server = self
                .next
                .take()
                .expect("the next pipe instance is always created before accept returns");
            server.connect().await?;

            // Create the replacement BEFORE handing this one over. A client
            // that arrives while the previous connection is still being set up
            // gets ERROR_PIPE_BUSY otherwise, and the shell reports a busy pipe
            // as "the agent is not running".
            //
            // If this fails the connection is still good, so it is returned and
            // the listener is left empty rather than dropping a client on the
            // floor — but the next accept would then panic, so the error wins.
            // It only fails if the machine is out of handles, in which case
            // stopping is right.
            self.next = Some(self.security.create_pipe(&self.endpoint.0, false)?);

            Ok(Stream(StreamInner::Server(server)))
        }
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        // The socket file outlives the process; the pipe name does not.
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.endpoint.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// The whole point of the abstraction: the same test passes on both
    /// platforms, so the Windows transport is exercised by the suite the moment
    /// it runs on Windows rather than by a separate script somebody remembers
    /// to run.
    #[tokio::test]
    async fn a_client_and_a_server_exchange_a_line() {
        let ep = super::endpoint(&format!("test-{}", std::process::id()));
        let mut listener = Listener::bind(&ep).unwrap();

        let server = tokio::spawn(async move {
            let stream = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let got = lines.next_line().await.unwrap().unwrap();
            write.write_all(format!("echo:{got}\n").as_bytes()).await.unwrap();
            // Held until the client has read the answer.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        });

        let stream = Stream::connect(&ep).await.unwrap();
        let (read, mut write) = stream.into_split();
        write.write_all(b"hello\n").await.unwrap();
        let mut lines = BufReader::new(read).lines();
        assert_eq!(lines.next_line().await.unwrap().unwrap(), "echo:hello");

        server.await.unwrap();
    }

    /// Two clients in a row, which is the case the Windows implementation gets
    /// wrong if `accept` forgets to create the next instance. On Unix it cannot
    /// fail; it is here so that the version that can is covered by a test that
    /// already exists.
    #[tokio::test]
    async fn a_second_client_is_served_after_the_first() {
        let ep = super::endpoint(&format!("test2-{}", std::process::id()));
        let mut listener = Listener::bind(&ep).unwrap();

        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let stream = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let (read, mut write) = stream.into_split();
                    let mut lines = BufReader::new(read).lines();
                    if let Ok(Some(got)) = lines.next_line().await {
                        let _ = write.write_all(format!("{got}!\n").as_bytes()).await;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                });
            }
        });

        for word in ["one", "two"] {
            let stream = Stream::connect(&ep).await.unwrap();
            let (read, mut write) = stream.into_split();
            write.write_all(format!("{word}\n").as_bytes()).await.unwrap();
            let mut lines = BufReader::new(read).lines();
            assert_eq!(lines.next_line().await.unwrap().unwrap(), format!("{word}!"));
        }

        server.await.unwrap();
    }

    #[tokio::test]
    async fn nothing_listening_is_not_found() {
        let ep = super::endpoint(&format!("absent-{}", std::process::id()));
        let err = match Stream::connect(&ep).await {
            Ok(_) => panic!("something answered on an address nothing is listening on"),
            Err(e) => e,
        };
        assert_eq!(
            err.kind(),
            io::ErrorKind::NotFound,
            "the shell decides whether to start the agent from this kind: {err}"
        );
    }

    #[test]
    fn two_installations_do_not_share_an_address() {
        assert_ne!(super::endpoint("aaaaaaaa"), super::endpoint("bbbbbbbb"));
    }
}
