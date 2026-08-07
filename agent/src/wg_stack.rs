// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright 2026 Bogdan Shapovalov and the Fury authors

//! A TCP stack inside the tunnel, so the relay can dial through it.
//!
//! `wg_tunnel` turns IP packets into encrypted datagrams. What nothing yet does
//! is produce those IP packets — because a WireGuard tunnel is a NETWORK, and
//! the relay wants a STREAM. Between the two sits a TCP implementation, and
//! there is no borrowing the operating system's: the kernel's stack is attached
//! to the kernel's interfaces, and the whole point of a userspace tunnel is
//! that there is no interface.
//!
//! So `smoltcp` runs here, over a device whose wire is the tunnel:
//!
//! ```text
//!   relay dials  ->  smoltcp TCP socket  ->  IP packet
//!                                             |
//!                                        wg_tunnel::send_packet
//!                                             |
//!                                        encrypted UDP to the peer
//! ```
//!
//! ## One task owns everything
//!
//! smoltcp's `Interface` and `SocketSet` are not `Sync` and are not meant to be
//! shared. Rather than wrap them in a mutex — which would serialise every
//! packet behind every dial anyway — all of it lives in one task, and callers
//! talk to it over channels. `dial` sends a request and gets back one half of a
//! `tokio::io::duplex` pair; the task pumps the other half.
//!
//! That also settles the shutdown question by construction: drop the handle,
//! the channel closes, the task ends, and the tunnel's socket goes with it.
//! A profile that closes takes its tunnel with it and leaves nothing running.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};

use anyhow::{bail, Context, Result};
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{IpAddress, IpCidr};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

/// Inside a WireGuard tunnel the usable MTU is 1420: 1500 less 60 for the outer
/// IPv6 header, 8 for UDP and 32 for WireGuard's own framing. Providers publish
/// this number and getting it wrong shows up as large responses hanging rather
/// than as an error.
const MTU: usize = 1420;

/// Buffers per connection. A browser opens many, so this is per-socket cost.
const RX_BUF: usize = 64 * 1024;
const TX_BUF: usize = 64 * 1024;

/// The device smoltcp writes to: two queues, drained by the task that owns it.
#[derive(Default)]
struct Wire {
    /// Packets that arrived from the peer, waiting to be given to smoltcp.
    inbound: std::collections::VecDeque<Vec<u8>>,
    /// Packets smoltcp produced, waiting to go into the tunnel.
    outbound: std::collections::VecDeque<Vec<u8>>,
}

impl Device for Wire {
    type RxToken<'a> = RxTok;
    type TxToken<'a> = TxTok<'a>;

    fn receive(&mut self, _now: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let packet = self.inbound.pop_front()?;
        // Both tokens borrow the device in smoltcp's model; the rx one owns its
        // bytes so the outbound queue stays mutably borrowed by the tx token.
        Some((RxTok(packet), TxTok(&mut self.outbound)))
    }

    fn transmit(&mut self, _now: Instant) -> Option<Self::TxToken<'_>> {
        Some(TxTok(&mut self.outbound))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        // IP rather than Ethernet: there are no frames in a tunnel and no ARP.
        // Choosing Ethernet here would have smoltcp emit frames the peer does
        // not expect, and the symptom is a tunnel that establishes and carries
        // nothing.
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = MTU;
        caps
    }
}

struct RxTok(Vec<u8>);
impl RxToken for RxTok {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

struct TxTok<'a>(&'a mut std::collections::VecDeque<Vec<u8>>);
impl TxToken for TxTok<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0u8; len];
        let out = f(&mut buf);
        self.0.push_back(buf);
        out
    }
}

/// A name to resolve, inside the tunnel.
struct Resolve {
    name: String,
    answer: tokio::sync::oneshot::Sender<Result<IpAddr>>,
}

/// What a caller sends to the task that owns the stack.
struct Dial {
    to: SocketAddr,
    /// The task's end of the duplex pair; the caller keeps the other.
    stream: DuplexStream,
    /// Answered once the connection is up or has failed, so `dial` can report
    /// a refusal rather than handing back a stream that silently never works.
    ready: tokio::sync::oneshot::Sender<Result<()>>,
}

/// A running tunnel with a TCP stack on it.
pub struct Stack {
    dials: tokio::sync::mpsc::Sender<Dial>,
    resolves: tokio::sync::mpsc::Sender<Resolve>,
    task: tokio::task::JoinHandle<()>,
}

impl Stack {
    /// Brings the tunnel up and starts the stack.
    pub async fn start(config: &crate::wireguard::Config) -> Result<Stack> {
        let tunnel = crate::wg_tunnel::Tunnel::connect(config).await?;

        // The address the peer assigned. `Address = 10.2.0.2/32` in the config;
        // the prefix is theirs to decide and taking the first is right because
        // a WireGuard client has exactly one.
        let cidr = config
            .addresses
            .first()
            .context("[Interface] has no Address, so there is nothing to be inside the tunnel")?;
        let (addr, prefix) = parse_cidr(cidr)?;

        // The resolvers the peer gave us. A tunnel with no DNS line is a
        // tunnel whose provider expects you to use theirs and did not say so;
        // falling back to the machine's resolver would send every name the
        // profile visits out of the operator's own connection, which is the
        // exact leak this is built to close. So: refuse instead.
        if config.dns.is_empty() {
            bail!(
                "the config has no DNS line, so names could only be resolved \
                 outside the tunnel — which would send every site the profile \
                 visits to this machine's resolver. Ask the provider for their \
                 DNS, or add one to the config."
            );
        }
        let servers: Vec<IpAddress> = config
            .dns
            .iter()
            .filter_map(|s| s.parse::<IpAddr>().ok())
            .map(IpAddress::from)
            .collect();
        if servers.is_empty() {
            bail!("none of the DNS entries {:?} is an address", config.dns);
        }

        let (tx, rx) = tokio::sync::mpsc::channel::<Dial>(32);
        let (rtx, rrx) = tokio::sync::mpsc::channel::<Resolve>(32);
        let task = tokio::spawn(run(tunnel, addr, prefix, servers, rx, rrx));
        Ok(Stack { dials: tx, resolves: rtx, task })
    }

    /// Resolves a name using the peer's resolvers, inside the tunnel.
    ///
    /// The whole reason `dial` refuses names: doing this outside would ask the
    /// operator's own resolver what the profile is about to visit.
    pub async fn resolve(&self, name: &str) -> Result<IpAddr> {
        // An address needs no lookup, and asking anyway would be a query that
        // tells the resolver something for no answer in return.
        if let Ok(addr) = name.parse::<IpAddr>() {
            return Ok(addr);
        }
        let (answer, got) = tokio::sync::oneshot::channel();
        self.resolves
            .send(Resolve { name: name.to_string(), answer })
            .await
            .map_err(|_| anyhow::anyhow!("the tunnel is no longer running"))?;
        got.await
            .map_err(|_| anyhow::anyhow!("the tunnel stopped while resolving"))?
    }

    /// Opens a TCP connection through the tunnel.
    ///
    /// Takes an address rather than a name: DNS inside the tunnel is a separate
    /// problem and pretending otherwise here would make a resolver appear by
    /// accident, resolving through the machine rather than through the exit —
    /// which is the leak the whole tunnel exists to prevent.
    pub async fn dial(&self, to: SocketAddr) -> Result<DuplexStream> {
        let (mine, theirs) = tokio::io::duplex(RX_BUF);
        let (ready, done) = tokio::sync::oneshot::channel();
        self.dials
            .send(Dial { to, stream: theirs, ready })
            .await
            .map_err(|_| anyhow::anyhow!("the tunnel is no longer running"))?;
        done.await
            .map_err(|_| anyhow::anyhow!("the tunnel stopped while connecting"))??;
        Ok(mine)
    }
}

/// Deliberately says nothing.
///
/// `Upstream` derives Debug and is logged on every launch, and the reason that
/// derive is safe at all is a custom Debug one type over that keeps proxy
/// passwords out of the log line. A tunnel is reached through keys rather than
/// a password, and a Debug that grew fields later would put them somewhere
/// nobody thought to look.
impl std::fmt::Debug for Stack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("wireguard tunnel")
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        // Dropping the sender would end the task on its own; aborting makes it
        // immediate. A tunnel outliving the profile that opened it is a live
        // exit nobody is using — the same leak `relay` had with its JoinHandle.
        self.task.abort();
    }
}

async fn run(
    mut tunnel: crate::wg_tunnel::Tunnel,
    addr: IpAddr,
    prefix: u8,
    servers: Vec<IpAddress>,
    mut dials: tokio::sync::mpsc::Receiver<Dial>,
    mut resolves: tokio::sync::mpsc::Receiver<Resolve>,
) {
    let mut wire = Wire::default();
    let mut iface = Interface::new(
        Config::new(smoltcp::wire::HardwareAddress::Ip),
        &mut wire,
        Instant::now(),
    );
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(IpAddress::from(addr), prefix));
    });
    // Everything goes to the peer. The config's AllowedIPs was already checked
    // to be a default route (`wireguard::Config::parse` refuses a split
    // tunnel), so there is exactly one place for a packet to go.
    let _ = iface.routes_mut().add_default_ipv4_route(
        match addr {
            IpAddr::V4(v4) => v4.into(),
            IpAddr::V6(_) => {
                tracing::error!("IPv6-only tunnels are not supported yet");
                return;
            }
        },
    );

    let mut sockets = SocketSet::new(Vec::new());
    let dns = sockets.add(smoltcp::socket::dns::Socket::new(&servers, Vec::new()));
    let mut queries: Vec<(smoltcp::socket::dns::QueryHandle, String, tokio::sync::oneshot::Sender<Result<IpAddr>>)> =
        Vec::new();
    let mut pumps: HashMap<smoltcp::iface::SocketHandle, Pump> = HashMap::new();
    let mut next_port: u16 = 49152;
    let mut packet = Vec::new();

    loop {
        tokio::select! {
            // A new connection wanted.
            Some(dial) = dials.recv() => {
                let socket = tcp::Socket::new(
                    tcp::SocketBuffer::new(vec![0u8; RX_BUF]),
                    tcp::SocketBuffer::new(vec![0u8; TX_BUF]),
                );
                let handle = sockets.add(socket);
                // Ephemeral, and wrapping rather than growing: a long-lived
                // agent that only ever incremented would run out.
                next_port = next_port.checked_add(1).unwrap_or(49152);
                let s = sockets.get_mut::<tcp::Socket>(handle);
                let endpoint = (IpAddress::from(dial.to.ip()), dial.to.port());
                match s.connect(iface.context(), endpoint, next_port) {
                    Ok(()) => {
                        pumps.insert(handle, Pump { stream: dial.stream, ready: Some(dial.ready), out: Vec::new() });
                    }
                    Err(e) => {
                        let _ = dial.ready.send(Err(anyhow::anyhow!("{e:?}")));
                        sockets.remove(handle);
                    }
                }
            }

            // A name to look up.
            Some(req) = resolves.recv() => {
                let socket = sockets.get_mut::<smoltcp::socket::dns::Socket>(dns);
                match socket.start_query(iface.context(), &req.name, smoltcp::wire::DnsQueryType::A) {
                    Ok(handle) => queries.push((handle, req.name, req.answer)),
                    Err(e) => {
                        let _ = req.answer.send(Err(anyhow::anyhow!("{e:?}")));
                    }
                }
            }

            // A datagram from the peer.
            got = tunnel.recv_packet(&mut packet) => {
                match got {
                    Ok(true) => wire.inbound.push_back(std::mem::take(&mut packet)),
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, "the tunnel failed");
                        return;
                    }
                }
            }

            // smoltcp's timers: retransmits, delayed ACKs, keepalives.
            _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
        }

        iface.poll(Instant::now(), &mut wire, &mut sockets);

        // Everything smoltcp produced goes into the tunnel.
        while let Some(out) = wire.outbound.pop_front() {
            if let Err(e) = tunnel.send_packet(&out).await {
                tracing::warn!(error = %e, "could not send through the tunnel");
                return;
            }
        }
        if let Err(e) = tunnel.tick().await {
            tracing::warn!(error = %e, "the tunnel's timers failed");
            return;
        }

        // Answers that have arrived.
        //
        // Walked backwards with swap_remove rather than retain_mut: a oneshot
        // sender has to be MOVED to be used, and retain_mut only lends it. The
        // first version tried to work around that and was nonsense.
        {
            let socket = sockets.get_mut::<smoltcp::socket::dns::Socket>(dns);
            let mut i = queries.len();
            while i > 0 {
                i -= 1;
                use smoltcp::socket::dns::GetQueryResultError as E;
                let outcome = match socket.get_query_result(queries[i].0) {
                    Err(E::Pending) => continue,
                    Ok(addrs) => match addrs.first() {
                        Some(IpAddress::Ipv4(v4)) => Ok(IpAddr::from(std::net::Ipv4Addr::from(*v4))),
                        Some(IpAddress::Ipv6(v6)) => Ok(IpAddr::from(std::net::Ipv6Addr::from(*v6))),
                        // A successful query with no address is NXDOMAIN's
                        // quieter cousin, and it has to read as "no such host"
                        // rather than as a lookup that is still running.
                        None => Err(anyhow::anyhow!("{} has no address", queries[i].1)),
                    },
                    Err(e) => Err(anyhow::anyhow!("resolving {}: {e:?}", queries[i].1)),
                };
                let (_, _, answer) = queries.swap_remove(i);
                let _ = answer.send(outcome);
            }
        }

        // Move bytes between each socket and its caller.
        let mut finished = Vec::new();
        for (handle, pump) in pumps.iter_mut() {
            let socket = sockets.get_mut::<tcp::Socket>(*handle);

            if let Some(ready) = pump.ready.take() {
                if socket.may_send() {
                    let _ = ready.send(Ok(()));
                } else if !socket.is_open() {
                    let _ = ready.send(Err(anyhow::anyhow!("the peer refused the connection")));
                    finished.push(*handle);
                    continue;
                } else {
                    // Still connecting; put it back.
                    pump.ready = Some(ready);
                }
            }

            // Tunnel -> caller.
            if socket.can_recv() {
                let mut buf = [0u8; 8192];
                if let Ok(n) = socket.recv_slice(&mut buf) {
                    if n > 0 && pump.stream.write_all(&buf[..n]).await.is_err() {
                        finished.push(*handle);
                        continue;
                    }
                }
            }

            // Caller -> tunnel. Read without blocking the loop: a socket with
            // nothing to say must not stall every other connection.
            if socket.can_send() {
                let mut buf = [0u8; 8192];
                match tokio::time::timeout(
                    std::time::Duration::from_millis(1),
                    pump.stream.read(&mut buf),
                )
                .await
                {
                    Ok(Ok(0)) => {
                        socket.close();
                        finished.push(*handle);
                    }
                    Ok(Ok(n)) => {
                        let _ = socket.send_slice(&buf[..n]);
                    }
                    Ok(Err(_)) => finished.push(*handle),
                    Err(_) => {}
                }
            }

            if !socket.is_active() && pump.ready.is_none() {
                finished.push(*handle);
            }
        }
        for handle in finished {
            pumps.remove(&handle);
            sockets.remove(handle);
        }
    }
}

struct Pump {
    stream: DuplexStream,
    ready: Option<tokio::sync::oneshot::Sender<Result<()>>>,
    #[allow(dead_code)]
    out: Vec<u8>,
}

/// `10.2.0.2/32` into an address and a prefix.
fn parse_cidr(s: &str) -> Result<(IpAddr, u8)> {
    let (addr, prefix) = match s.split_once('/') {
        Some((a, p)) => (a, p.parse::<u8>().context("the prefix is not a number")?),
        // A bare address means "just me", which is what /32 says.
        None => (s, 32),
    };
    let addr: IpAddr = addr.trim().parse().with_context(|| format!("{s} is not an address"))?;
    let max = if addr.is_ipv4() { 32 } else { 128 };
    if prefix > max {
        bail!("/{prefix} is not a prefix an address of that family can have");
    }
    Ok((addr, prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_and_its_prefix_are_read_the_way_a_provider_writes_them() {
        assert_eq!(
            parse_cidr("10.2.0.2/32").unwrap(),
            ("10.2.0.2".parse::<IpAddr>().unwrap(), 32)
        );
        // Some providers omit the prefix. A client has one address, so the
        // absence means /32 rather than "unknown".
        assert_eq!(
            parse_cidr("10.2.0.2").unwrap(),
            ("10.2.0.2".parse::<IpAddr>().unwrap(), 32)
        );
        assert_eq!(parse_cidr("fd00::2/128").unwrap().1, 128);

        assert!(parse_cidr("10.2.0.2/33").is_err());
        assert!(parse_cidr("not-an-address/32").is_err());
        assert!(parse_cidr("10.2.0.2/abc").is_err());
    }

    /// A config with no DNS must be REFUSED rather than quietly falling back
    /// to this machine's resolver.
    ///
    /// The fallback is the tempting thing to write and it is the whole leak:
    /// every site the profile opens would be announced to the operator's own
    /// resolver, from the operator's own address, before a single byte went
    /// through the tunnel. The bytes would be private and the intent would not.
    #[tokio::test]
    async fn a_tunnel_with_no_resolver_is_refused_rather_than_leaking_to_the_machine() {
        const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        let text = format!(
            "[Interface]\nPrivateKey = {KEY}\nAddress = 10.2.0.2/32\n\n\
             [Peer]\nPublicKey = {KEY}\nEndpoint = 127.0.0.1:51820\n\
             AllowedIPs = 0.0.0.0/0\n"
        );
        let config = crate::wireguard::Config::parse(&text).expect("a valid config");
        assert!(config.dns.is_empty(), "the fixture must have no DNS line");

        let err = Stack::start(&config).await.unwrap_err().to_string();
        assert!(err.contains("DNS"), "{err}");
        assert!(
            err.contains("resolver"),
            "the message has to say what would leak: {err}"
        );
    }

    /// The device is where a wrong medium turns into a tunnel that establishes
    /// and carries nothing, so the choice is asserted rather than assumed.
    #[test]
    fn the_device_speaks_ip_rather_than_ethernet() {
        let caps = Wire::default().capabilities();
        assert_eq!(caps.medium, Medium::Ip, "a tunnel has no frames and no ARP");
        assert_eq!(caps.max_transmission_unit, 1420);
    }

    /// Packets handed to the device come back out of it in order and intact —
    /// the smallest claim the stack rests on.
    #[test]
    fn the_wire_carries_packets_in_order() {
        let mut wire = Wire::default();
        wire.inbound.push_back(vec![1, 2, 3]);
        wire.inbound.push_back(vec![4, 5]);

        let (rx, _tx) = wire.receive(Instant::now()).expect("a packet was queued");
        assert_eq!(rx.consume(|b| b.to_vec()), vec![1, 2, 3]);
        let (rx, _tx) = wire.receive(Instant::now()).expect("and the second");
        assert_eq!(rx.consume(|b| b.to_vec()), vec![4, 5]);
        assert!(wire.receive(Instant::now()).is_none());

        // And what smoltcp transmits lands in the outbound queue for the
        // tunnel, which is the other half of the contract.
        let tx = wire.transmit(Instant::now()).unwrap();
        tx.consume(4, |b| b.copy_from_slice(&[9, 9, 9, 9]));
        assert_eq!(wire.outbound.pop_front().unwrap(), vec![9, 9, 9, 9]);
    }
}
