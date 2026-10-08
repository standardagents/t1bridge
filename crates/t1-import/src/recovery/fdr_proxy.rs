//! FDR control and bounded device TLS tunnels to Apple's public HTTPS hosts.
//! These outbound sockets exist only while an attended restore owns the USB
//! interface. Device TLS is passed through. Only the legacy gg.apple.com:80
//! request is upgraded to host-verified TLS on port 443; plaintext never leaves
//! the computer. No tunneled payload is logged.

use super::{
    Error, Result,
    mux::{Mux, Transport},
    plist::{self, Value},
};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};
use t1_platform::recovery_https::FdrHttps;

const INVALID: Error = Error("invalid or unsupported FDR protocol message");

enum State {
    Connecting,
    Hello,
    Command,
    Plist,
    SocksGreeting,
    SocksConnect,
    Tunnel,
    Finishing,
}

struct Worker {
    state: State,
    greeted: bool,
    socket: Option<FdrSocket>,
    pending: Vec<u8>,
    written: usize,
    active: Instant,
}

pub(super) struct Fdr {
    control: u16,
    control_started: bool,
    connection_port: Option<u16>,
    workers: BTreeMap<u16, Worker>,
}

impl Fdr {
    pub fn new<T: Transport>(mux: &mut Mux<T>) -> Result<Self> {
        Ok(Self {
            control: mux.connect(1082)?,
            control_started: false,
            connection_port: None,
            workers: BTreeMap::new(),
        })
    }

    pub fn ready(&self) -> bool {
        self.connection_port.is_some()
    }

    pub fn poll<T: Transport>(&mut self, mux: &mut Mux<T>) -> Result<()> {
        if !self.control_started && mux.connected(self.control)? {
            mux.send(self.control, b"BeginCtrl\0")?;
            mux.send_plist(
                self.control,
                &plist::dict([
                    ("Command", plist::string("BeginCtrl")),
                    ("CtrlProtoVersion", Value::Integer(2)),
                ]),
                true,
            )?;
            self.control_started = true;
        }
        if self.connection_port.is_none() {
            if let Some(reply) = mux.receive_plist(self.control, true)? {
                let port = u16::try_from(plist::integer(plist::get(&reply, "ConnPort")?)?)
                    .map_err(|_| INVALID)?;
                if port < 1024 || port == 62078 {
                    return Err(INVALID);
                }
                self.connection_port = Some(port);
            }
        } else {
            while let Some(command) = mux.peek(self.control, 2)? {
                if command != [1, 0] {
                    return Err(INVALID);
                }
                if mux.take(self.control, 4)?.is_none() {
                    break;
                }
                if self.workers.len() >= 16 {
                    return Err(Error("FDR requested too many concurrent channels"));
                }
                let port = mux.connect(self.connection_port.ok_or(INVALID)?)?;
                self.workers.insert(
                    port,
                    Worker {
                        state: State::Connecting,
                        greeted: false,
                        socket: None,
                        pending: vec![],
                        written: 0,
                        active: Instant::now(),
                    },
                );
            }
        }
        let ports: Vec<_> = self.workers.keys().copied().collect();
        for port in ports {
            if mux.closed(port)?
                && mux.available(port)? == 0
                && self
                    .workers
                    .get(&port)
                    .is_some_and(|w| w.written == w.pending.len())
            {
                // A completed per-request worker is closed by the device.
                mux.close(port)?;
                self.workers.remove(&port);
                continue;
            }
            let worker = self.workers.get_mut(&port).ok_or(INVALID)?;
            if worker.active.elapsed() > Duration::from_secs(180) {
                return Err(Error("FDR tunnel timed out"));
            }
            if worker.poll(mux, port)? {
                mux.close(port)?;
                self.workers.remove(&port);
            }
        }
        Ok(())
    }
}

impl Worker {
    fn poll<T: Transport>(&mut self, mux: &mut Mux<T>, port: u16) -> Result<bool> {
        match self.state {
            State::Connecting if mux.connected(port)? => {
                mux.send(port, b"HelloConn\0")?;
                self.state = State::Hello;
            }
            State::Hello => {
                if let Some(reply) = mux.receive_plist(port, true)? {
                    if plist::text(plist::get(&reply, "Command")?)? != "HelloConn" {
                        return Err(INVALID);
                    }
                    self.state = State::Command;
                    self.active = Instant::now();
                }
            }
            State::Command => {
                if let Some(command) = mux.take(port, 2)? {
                    self.state = match command.as_slice() {
                        [0xaa, 0xbb] => State::Plist,
                        [5, 1] if self.greeted => State::SocksConnect,
                        [5, 1] => State::SocksGreeting,
                        _ => return Err(INVALID),
                    };
                    self.active = Instant::now();
                }
            }
            State::Plist => {
                if let Some(message) = mux.receive_plist(port, true)? {
                    if plist::text(plist::get(&message, "Command")?)? != "Ping" {
                        return Err(INVALID);
                    }
                    mux.send_plist(port, &plist::dict([("Pong", Value::Boolean(true))]), true)?;
                    self.state = State::Command;
                    self.active = Instant::now();
                }
            }
            State::SocksGreeting => {
                if let Some(method) = mux.take(port, 1)? {
                    if method != [0] {
                        return Err(INVALID);
                    }
                    mux.send(port, &[5, 0])?;
                    self.greeted = true;
                    self.state = State::Command;
                }
            }
            State::SocksConnect => {
                if let Some(header) = mux.peek(port, 3)? {
                    if header[..2] != [0, 3] || header[2] == 0 {
                        return Err(INVALID);
                    }
                    if let Some(request) = mux.take(port, usize::from(header[2]) + 5)? {
                        let host = std::str::from_utf8(&request[3..request.len() - 2])
                            .map_err(|_| INVALID)?;
                        let destination = u16::from_be_bytes([
                            request[request.len() - 2],
                            request[request.len() - 1],
                        ]);
                        self.socket = Some(connect_apple(host, destination)?);
                        mux.send(port, &[5, 0])?;
                        mux.send(port, &request)?;
                        self.state = State::Tunnel;
                        self.active = Instant::now();
                    }
                }
            }
            State::Tunnel => return self.tunnel(mux, port),
            State::Finishing => return mux.drained(port),
            State::Connecting => {}
        }
        Ok(false)
    }

    fn tunnel<T: Transport>(&mut self, mux: &mut Mux<T>, port: u16) -> Result<bool> {
        let socket = self.socket.as_mut().ok_or(INVALID)?;
        if self.written == self.pending.len() {
            self.pending = mux
                .take(port, mux.available(port)?.min(64 * 1024))?
                .ok_or(INVALID)?;
            self.written = 0;
        }
        if self.written < self.pending.len() {
            match socket.write(&self.pending[self.written..]) {
                Ok(0) => {
                    return Err(Error(
                        "FDR outbound connection closed before write completed",
                    ));
                }
                Ok(n) => {
                    self.written += n;
                    self.active = Instant::now();
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
        if mux.closed(port)? {
            return Ok(self.written == self.pending.len() && mux.available(port)? == 0);
        }
        if mux.pending(port)? < 1024 * 1024 {
            let mut bytes = vec![0; 64 * 1024];
            match socket.read(&mut bytes) {
                Ok(0) if mux.drained(port)? => {
                    if self.written != self.pending.len() {
                        return Err(Error("FDR server closed with unsent device data"));
                    }
                    mux.finish(port)?;
                    self.state = State::Finishing;
                }
                Ok(n) => {
                    mux.send(port, &bytes[..n])?;
                    self.active = Instant::now();
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(false)
    }
}

enum FdrSocket {
    DeviceTls(TcpStream),
    HostTls(FdrHttps),
}

impl Read for FdrSocket {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::DeviceTls(socket) => socket.read(bytes),
            Self::HostTls(socket) => socket.read(bytes),
        }
    }
}

impl Write for FdrSocket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::DeviceTls(socket) => socket.write(bytes),
            Self::HostTls(socket) => socket.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn upgrade_to_https(host: &str, port: u16) -> bool {
    host == "gg.apple.com" && port == 80
}

fn allowed_host(host: &str, port: u16) -> bool {
    port == 443
        && host.len() <= 253
        && host.ends_with(".apple.com")
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn public_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && octets[0] != 0
                && octets[0] < 224
                && !(octets[0] == 100 && (64..=127).contains(&octets[1]))
                && !(octets[0] == 198 && matches!(octets[1], 18 | 19))
        }
        IpAddr::V6(ip) => {
            ip.segments()[0] & 0xe000 == 0x2000
                && !(ip.segments()[0] == 0x2001 && ip.segments()[1] == 0x0db8)
        }
    }
}

fn connect_apple(host: &str, port: u16) -> Result<FdrSocket> {
    let upgrade = upgrade_to_https(host, port);
    if !upgrade && !allowed_host(host, port) {
        // Print only protocol metadata and fixed public service names. A
        // device-supplied hostname can contain an identifier; never echo it.
        let normalized = host.trim_end_matches(['\0', '.']).to_ascii_lowercase();
        let known = known_service(&normalized);
        eprintln!("recovery: rejected FDR destination: known_service={known} port={port}");
        eprintln!(
            "recovery: FDR host metadata: bytes={} ascii={} trailing_nul={} trailing_dot={} normalized_apple_https={}",
            host.len(),
            host.is_ascii(),
            host.ends_with('\0'),
            host.ends_with('.'),
            allowed_host(&normalized, 443)
        );
        return Err(Error(
            "FDR requested a destination outside the allowed Apple services",
        ));
    }
    // Every outbound connection uses 443, including the legacy-device request.
    let addresses: Vec<_> = (host, 443).to_socket_addrs()?.take(8).collect();
    if addresses.is_empty() || addresses.iter().any(|a| !public_address(a.ip())) {
        return Err(Error("FDR destination did not resolve to a public address"));
    }
    let known = known_service(host);
    eprintln!(
        "recovery: connecting FDR service: known_service={known} requested_port={port} host_tls={upgrade} public_addresses={}",
        addresses.len()
    );
    connect_candidates(&addresses, Duration::from_secs(30), |address| {
        if upgrade {
            let socket = FdrHttps::connect(address.ip())?;
            println!(
                "recovery: gg.apple.com legacy request upgraded to verified HTTPS on port 443"
            );
            Ok(FdrSocket::HostTls(socket))
        } else {
            let socket = TcpStream::connect_timeout(&address, Duration::from_secs(5))?;
            socket.set_nonblocking(true)?;
            Ok(FdrSocket::DeviceTls(socket))
        }
    })
    .map_err(|error| {
        eprintln!(
            "recovery: FDR connection failed: known_service={known} error_kind={:?} errno={:?}",
            error.kind(),
            error.raw_os_error()
        );
        Error("unable to connect to Apple's FDR service")
    })
}

fn known_service(host: &str) -> &'static str {
    match host {
        "gg.apple.com" => "gg.apple.com",
        "gs.apple.com" => "gs.apple.com",
        "albert.apple.com" => "albert.apple.com",
        "skl.apple.com" => "skl.apple.com",
        "humb.apple.com" => "humb.apple.com",
        "tbsc.apple.com" => "tbsc.apple.com",
        "static.ips.apple.com" => "static.ips.apple.com",
        _ => "unlisted (withheld)",
    }
}

// Retry only before returning a connected socket: no device request bytes have
// been forwarded yet. Never replay a request after a tunnel write/read failure.
fn connect_candidates<S>(
    addresses: &[SocketAddr],
    timeout: Duration,
    mut connect: impl FnMut(SocketAddr) -> io::Result<S>,
) -> io::Result<S> {
    let deadline = Instant::now() + timeout;
    let mut last = io::Error::from(io::ErrorKind::TimedOut);
    for round in 0..2 {
        for address in addresses {
            if Instant::now() >= deadline {
                return Err(last);
            }
            match connect(*address) {
                Ok(socket) => return Ok(socket),
                Err(error) => {
                    eprintln!(
                        "recovery: FDR connection attempt failed: round={} error_kind={:?} errno={:?}",
                        round + 1,
                        error.kind(),
                        error.raw_os_error()
                    );
                    if !retryable_connection_error(error.kind()) {
                        return Err(error);
                    }
                    last = error;
                }
            }
        }
    }
    Err(last)
}

fn retryable_connection_error(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::TimedOut
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::AddrNotAvailable
            | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_retry_is_bounded_and_never_retries_trust_failure() {
        let addresses = ["192.0.2.1:443".parse().unwrap()];
        let mut calls = 0;
        let result = connect_candidates(&addresses, Duration::from_secs(1), |_| {
            calls += 1;
            if calls == 1 {
                Err(io::ErrorKind::TimedOut.into())
            } else {
                Ok(7)
            }
        })
        .unwrap();
        assert_eq!((result, calls), (7, 2));
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::InvalidData] {
            let mut calls = 0;
            let result: io::Result<()> =
                connect_candidates(&addresses, Duration::from_secs(1), |_| {
                    calls += 1;
                    Err(kind.into())
                });
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
        let mut calls = 0;
        let result: io::Result<()> = connect_candidates(&addresses, Duration::from_secs(1), |_| {
            calls += 1;
            Err(io::ErrorKind::ConnectionRefused.into())
        });
        assert!(result.is_err());
        assert_eq!(calls, 2);
        let result: io::Result<()> = connect_candidates(&addresses, Duration::ZERO, |_| {
            panic!("expired connection budget")
        });
        assert!(result.is_err());
        assert_eq!(
            known_service("synthetic-private.apple.com"),
            "unlisted (withheld)"
        );
    }

    #[test]
    fn only_exact_legacy_bootstrap_destination_gets_a_tls_upgrade() {
        assert!(upgrade_to_https("gg.apple.com", 80));
        for host in [
            "gg.apple.com.attacker.invalid",
            "synthetic.gg.apple.com",
            "gg.apple.com.",
            "gg.apple.com\0",
            "GG.apple.com",
            "gs.apple.com",
            "127.0.0.1",
        ] {
            assert!(!upgrade_to_https(host, 80));
        }
        for port in [0, 22, 81, 443, 8080, 8443] {
            assert!(!upgrade_to_https("gg.apple.com", port));
        }
    }

    #[test]
    fn proxy_rejects_plaintext_and_non_apple_destinations() {
        assert!(allowed_host("synthetic.fdr.apple.com", 443));
        assert!(!allowed_host("gg.apple.com", 80));
        assert!(allowed_host("gg.apple.com", 443));
        for host in [
            "gg.apple.com.attacker.invalid",
            "synthetic.gg.apple.com",
            "gg.apple.com.",
            "gg.apple.com\0",
            "GG.apple.com",
            "gs.apple.com",
        ] {
            assert!(!allowed_host(host, 80));
        }
        for port in [0, 22, 81, 8080, 8443] {
            assert!(!allowed_host("gg.apple.com", port));
        }
        for host in [
            "apple.com.attacker.invalid",
            "localhost",
            "127.0.0.1",
            "apple.com",
            ".apple.com",
            "a..apple.com",
            "a/apple.com",
            "a@apple.com",
        ] {
            assert!(!allowed_host(host, 443));
        }
        assert!(!allowed_host("synthetic.apple.com", 80));
        for address in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.1.1",
            "192.0.2.1",
            "100.64.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(!public_address(address.parse().unwrap()));
        }
    }
}
