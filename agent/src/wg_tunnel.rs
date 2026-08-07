// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! A WireGuard tunnel in this process, with no help from the operating system.
//!
//! ## Why userspace at all
//!
//! `wg-quick` configures a system interface, which routes the WHOLE MACHINE.
//! That is the opposite of what a profile needs: one profile through one exit,
//! the operator's own browser untouched, ten profiles through ten different
//! tunnels at once. macOS and Windows will not give an unprivileged application
//! a per-process route, so the tunnel has to live here.
//!
//! ## The shape
//!
//! WireGuard is a function from IP packets to UDP datagrams. `boringtun`'s
//! `Tunn` is exactly that function and nothing more — it holds the handshake
//! state and the session keys, and it has no idea what a socket is:
//!
//! ```text
//!   plaintext IP packet  --encapsulate-->  UDP payload for the peer
//!   UDP payload received --decapsulate-->  plaintext IP packet
//! ```
//!
//! So this module is the part `Tunn` deliberately leaves out: a socket, a
//! timer, and the loop between them. What sits ON TOP — turning those IP
//! packets into a TCP stream the relay can dial — is the next piece, and it is
//! not here yet.
//!
//! ## The two things that are easy to get wrong
//!
//! **The handshake is not request/response.** `encapsulate` on a fresh tunnel
//! does not return the encrypted packet — it returns a HANDSHAKE INITIATION and
//! drops the packet on the floor. The data only flows once the peer has
//! answered. Code that assumes one packet in gives one packet out silently
//! loses the first thing every profile sends.
//!
//! **Timers are not optional.** `update_timers` is what re-keys a session
//! before it expires and what sends keepalives through a NAT. A tunnel with no
//! timer works for about two minutes and then stops, which is the worst
//! possible failure interval: long enough to look like it worked.

use std::net::SocketAddr;

use anyhow::{bail, Context, Result};
use base64::Engine;
use boringtun::noise::{Tunn, TunnResult};

/// The largest datagram this will send or accept.
///
/// WireGuard's overhead is 32 bytes over the inner packet, and the usual MTU
/// inside a tunnel is 1420 for that reason. The buffer is generous rather than
/// exact: `encapsulate` writes into it and a short buffer is a silent truncation.
const MAX_DATAGRAM: usize = 2048;

/// One peer, encrypting and decrypting for one profile.
pub struct Tunnel {
    tunn: Tunn,
    peer: SocketAddr,
    socket: tokio::net::UdpSocket,
}

impl Tunnel {
    /// Binds a local socket and prepares the session. Sends nothing yet.
    pub async fn connect(config: &crate::wireguard::Config) -> Result<Tunnel> {
        let private = key32(&config.private_key).context("PrivateKey")?;
        let public = key32(&config.peer_public_key).context("PublicKey")?;
        let psk = match &config.preshared_key {
            Some(k) => Some(key32(k).context("PresharedKey")?),
            None => None,
        };

        // Resolved through the machine's own resolver, and that is correct
        // rather than a leak: the ENDPOINT is a public host the operator chose,
        // and it has to be reachable before there is any tunnel to resolve
        // anything else through. Every name the profile visits afterwards is
        // resolved on the far side.
        let peer = tokio::net::lookup_host(&config.endpoint)
            .await
            .with_context(|| format!("resolving {}", config.endpoint))?
            .next()
            .with_context(|| format!("{} resolved to nothing", config.endpoint))?;

        let tunn = Tunn::new(
            boringtun::x25519::StaticSecret::from(private),
            boringtun::x25519::PublicKey::from(public),
            psk,
            config.keepalive,
            // The index only has to be unique among tunnels this process runs.
            rand::random(),
            None,
        );

        // Unspecified address and port: the peer learns where to answer from
        // the first datagram, which is how WireGuard works behind NAT.
        let socket = tokio::net::UdpSocket::bind(("0.0.0.0", 0))
            .await
            .context("binding a local UDP socket for the tunnel")?;
        socket
            .connect(peer)
            .await
            .with_context(|| format!("pointing the socket at {peer}"))?;

        Ok(Tunnel { tunn, peer, socket })
    }

    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Hands one plaintext IP packet to the tunnel.
    ///
    /// Returns whether the packet itself went. `false` means a handshake was
    /// sent instead and the caller's packet was dropped — which is WireGuard
    /// working correctly, not a failure, and the caller is expected to retry
    /// once the session is up.
    pub async fn send_packet(&mut self, packet: &[u8]) -> Result<bool> {
        let mut buf = [0u8; MAX_DATAGRAM];
        match self.tunn.encapsulate(packet, &mut buf) {
            TunnResult::WriteToNetwork(datagram) => {
                let sent = datagram.len();
                self.socket.send(datagram).await?;
                // A handshake initiation is 148 bytes and carries no payload.
                // Distinguishing them matters: "sent" and "sent something
                // instead" are different answers to the caller.
                Ok(sent != HANDSHAKE_INITIATION_LEN)
            }
            // Nothing to send is normal — a keepalive that is not due yet.
            TunnResult::Done => Ok(false),
            TunnResult::Err(e) => bail!("wireguard could not encapsulate: {e:?}"),
            _ => bail!("wireguard asked to write to the tunnel while sending"),
        }
    }

    /// Waits for one datagram and returns the plaintext IP packet inside it,
    /// or `None` when it was protocol traffic with no payload.
    ///
    /// Handshake replies, cookie replies and keepalives all arrive here and all
    /// produce `None` after being answered — the answer has to go back out on
    /// the same socket, which is why this both reads and writes.
    pub async fn recv_packet(&mut self, out: &mut Vec<u8>) -> Result<bool> {
        let mut datagram = [0u8; MAX_DATAGRAM];
        let n = self.socket.recv(&mut datagram).await?;

        // The result borrows the scratch buffer, and sending is an await —
        // so whatever comes back is COPIED before the borrow has to survive a
        // suspension point. Trying to hold the borrow across `.await` is what
        // the first version did, and it does not compile for a good reason.
        enum Step {
            Reply(Vec<u8>),
            Packet(Vec<u8>),
            Nothing,
        }
        fn step(tunn: &mut Tunn, input: &[u8]) -> Result<Step> {
            let mut scratch = [0u8; MAX_DATAGRAM];
            Ok(match tunn.decapsulate(None, input, &mut scratch) {
                TunnResult::WriteToNetwork(reply) => Step::Reply(reply.to_vec()),
                TunnResult::WriteToTunnelV4(p, _) | TunnResult::WriteToTunnelV6(p, _) => {
                    Step::Packet(p.to_vec())
                }
                TunnResult::Done => Step::Nothing,
                TunnResult::Err(e) => bail!("wireguard could not decapsulate: {e:?}"),
            })
        }

        let mut input: &[u8] = &datagram[..n];
        // decapsulate can ask for several writes in a row, and the documented
        // pattern is to keep calling it with an EMPTY input until it stops.
        // Skipping that is how a handshake completes on one side only — the
        // tunnel then looks established and carries nothing.
        loop {
            match step(&mut self.tunn, input)? {
                Step::Reply(reply) => {
                    self.socket.send(&reply).await?;
                    input = &[];
                }
                Step::Packet(packet) => {
                    out.clear();
                    out.extend_from_slice(&packet);
                    return Ok(true);
                }
                Step::Nothing => return Ok(false),
            }
        }
    }

    /// Re-keys and sends keepalives. Call about four times a second.
    ///
    /// Not optional and not a nicety: without it a session expires after about
    /// two minutes and the tunnel goes quiet. Two minutes is the worst interval
    /// there is, because everything looks fine for long enough to be believed.
    pub async fn tick(&mut self) -> Result<()> {
        let mut buf = [0u8; MAX_DATAGRAM];
        match self.tunn.update_timers(&mut buf) {
            TunnResult::WriteToNetwork(datagram) => {
                self.socket.send(datagram).await?;
                Ok(())
            }
            TunnResult::Done => Ok(()),
            TunnResult::Err(e) => bail!("wireguard timer: {e:?}"),
            _ => Ok(()),
        }
    }
}

/// A handshake initiation, in bytes. Used only to tell "I sent your packet"
/// from "I sent a handshake instead".
const HANDSHAKE_INITIATION_LEN: usize = 148;

fn key32(b64: &str) -> Result<[u8; 32]> {
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("not base64")?;
    raw.try_into()
        .map_err(|v: Vec<u8>| anyhow::anyhow!("{} bytes, expected 32", v.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use boringtun::x25519::{PublicKey, StaticSecret};

    /// Two peers, no sockets, no provider.
    ///
    /// This is the whole reason the crypto is tested at all here: a real
    /// tunnel needs somebody's VPN, and a test that needs somebody's VPN is a
    /// test nobody runs. `Tunn` is pure — packets in, datagrams out — so both
    /// ends fit in one process and the handshake is real.
    fn pair() -> (Tunn, Tunn) {
        let a_priv = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let b_priv = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let a_pub = PublicKey::from(&a_priv);
        let b_pub = PublicKey::from(&b_priv);

        (
            Tunn::new(a_priv, b_pub, None, None, 1, None),
            Tunn::new(b_priv, a_pub, None, None, 2, None),
        )
    }

    /// The property everything else rests on: a packet handed in at one end
    /// comes out the other, byte for byte, after a real handshake.
    #[test]
    fn a_packet_survives_a_real_handshake_between_two_peers() {
        let (mut a, mut b) = pair();
        let mut buf = [0u8; MAX_DATAGRAM];

        // A plausible IPv4 packet. The contents do not matter to WireGuard,
        // but a version nibble that is not 4 or 6 is refused, so this is not
        // arbitrary bytes.
        let mut packet = vec![0x45, 0, 0, 32];
        packet.extend_from_slice(&[0u8; 28]);

        // 1. A first send produces a HANDSHAKE, not the packet. This is the
        //    behaviour that silently loses the first request if assumed away.
        let init = match a.encapsulate(&packet, &mut buf) {
            TunnResult::WriteToNetwork(d) => d.to_vec(),
            other => panic!("expected a handshake initiation, got {other:?}"),
        };
        assert_eq!(init.len(), HANDSHAKE_INITIATION_LEN, "not an initiation");

        // 2. B answers it.
        let mut buf_b = [0u8; MAX_DATAGRAM];
        let response = match b.decapsulate(None, &init, &mut buf_b) {
            TunnResult::WriteToNetwork(d) => d.to_vec(),
            other => panic!("expected a handshake response, got {other:?}"),
        };

        // 3. A takes the answer. Now the session exists.
        let mut buf_a = [0u8; MAX_DATAGRAM];
        match a.decapsulate(None, &response, &mut buf_a) {
            TunnResult::WriteToNetwork(_) | TunnResult::Done => {}
            other => panic!("handshake did not complete: {other:?}"),
        }

        // 4. And now the packet actually travels.
        let mut buf2 = [0u8; MAX_DATAGRAM];
        let encrypted = match a.encapsulate(&packet, &mut buf2) {
            TunnResult::WriteToNetwork(d) => d.to_vec(),
            other => panic!("expected data, got {other:?}"),
        };
        assert_ne!(encrypted.len(), HANDSHAKE_INITIATION_LEN);
        assert!(
            !encrypted.windows(4).any(|w| w == [0x45, 0, 0, 32]),
            "the plaintext header is visible in the datagram"
        );

        let mut buf3 = [0u8; MAX_DATAGRAM];
        match b.decapsulate(None, &encrypted, &mut buf3) {
            TunnResult::WriteToTunnelV4(got, _) => assert_eq!(got, &packet[..]),
            other => panic!("the packet did not come out: {other:?}"),
        }
    }

    /// A peer configured against the wrong public key must not establish
    /// anything. Without this the test above would also pass on a build where
    /// the keys were being ignored.
    #[test]
    fn a_wrong_peer_key_does_not_produce_a_session() {
        let a_priv = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let stranger = PublicKey::from(&StaticSecret::random_from_rng(rand::rngs::OsRng));
        let b_priv = StaticSecret::random_from_rng(rand::rngs::OsRng);
        let a_pub = PublicKey::from(&a_priv);

        let mut a = Tunn::new(a_priv, stranger, None, None, 1, None);
        let mut b = Tunn::new(b_priv, a_pub, None, None, 2, None);

        let mut buf = [0u8; MAX_DATAGRAM];
        let mut packet = vec![0x45, 0, 0, 32];
        packet.extend_from_slice(&[0u8; 28]);
        let init = match a.encapsulate(&packet, &mut buf) {
            TunnResult::WriteToNetwork(d) => d.to_vec(),
            other => panic!("{other:?}"),
        };

        let mut buf_b = [0u8; MAX_DATAGRAM];
        match b.decapsulate(None, &init, &mut buf_b) {
            TunnResult::Err(_) | TunnResult::Done => {}
            other => panic!("a handshake to the wrong key was accepted: {other:?}"),
        }
    }

    /// Keys are read from the config the operator pasted, so a key that is the
    /// wrong length has to be named rather than truncated.
    #[test]
    fn a_key_of_the_wrong_length_is_named() {
        assert!(key32("QUFB").unwrap_err().to_string().contains("expected 32"));
        assert!(key32("not base64!!").is_err());
        assert!(key32("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=").is_ok());
    }
}
