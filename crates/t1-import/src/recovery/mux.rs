//! Apple USB multiplexing with bounded TCP-like channels, owned by one attempt.
//! There is no usbmuxd dependency, Unix socket, or listening TCP port.

use super::{
    Error, Result,
    plist::{self, Value},
    usb::Usb,
};
use std::collections::{BTreeMap, VecDeque};
use std::io;

const INVALID: Error = Error("invalid or unsupported USB multiplexing packet");
const CAPACITY: usize = 32 * 1024 * 1024;
const MAX_PACKET: usize = 128 * 1024;
const PAYLOAD: usize = 16_384 - 36;

pub(super) trait Transport {
    fn send(&self, bytes: &[u8]) -> Result<()>;
    fn receive(&self, bytes: &mut [u8]) -> io::Result<usize>;
}

impl Transport for Usb {
    fn send(&self, bytes: &[u8]) -> Result<()> {
        self.write(bytes, true)
    }
    fn receive(&self, bytes: &mut [u8]) -> io::Result<usize> {
        self.read(bytes)
    }
}

struct Channel {
    destination: u16,
    connected: bool,
    closed: bool,
    fin_sent: bool,
    send_next: u32,
    send_ack: u32,
    receive_next: u32,
    window: u32,
    input: VecDeque<u8>,
    output: VecDeque<u8>,
}

pub(super) struct Mux<T> {
    transport: T,
    version: u32,
    send_sequence: u16,
    receive_sequence: u16,
    bytes: Vec<u8>,
    channels: BTreeMap<u16, Channel>,
    next_port: u16,
}

impl<T: Transport> Mux<T> {
    pub fn new(transport: T) -> Result<Self> {
        let mut mux = Self {
            transport,
            version: 0,
            send_sequence: 0,
            receive_sequence: u16::MAX,
            bytes: vec![],
            channels: BTreeMap::new(),
            next_port: 1,
        };
        mux.packet(0, &[0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0])?;
        Ok(mux)
    }

    pub fn ready(&self) -> bool {
        self.version != 0
    }

    pub fn connect(&mut self, destination: u16) -> Result<u16> {
        if !self.ready()
            || destination == 0
            || self.channels.len() >= 32
            || self.next_port == u16::MAX
        {
            return Err(INVALID);
        }
        let port = self.next_port;
        self.next_port += 1;
        self.channels.insert(
            port,
            Channel {
                destination,
                connected: false,
                closed: false,
                fin_sent: false,
                send_next: 0,
                send_ack: 0,
                receive_next: 0,
                window: 0,
                input: VecDeque::new(),
                output: VecDeque::new(),
            },
        );
        self.tcp(port, 2, &[])?;
        self.channel_mut(port)?.send_next = 1;
        Ok(port)
    }

    pub fn connected(&self, port: u16) -> Result<bool> {
        let channel = self.channel(port)?;
        if channel.closed {
            return Err(Error("restore service refused or closed its channel"));
        }
        Ok(channel.connected)
    }

    pub fn closed(&self, port: u16) -> Result<bool> {
        Ok(self.channel(port)?.closed)
    }

    pub fn close(&mut self, port: u16) -> Result<()> {
        if !self.channel(port)?.closed && !self.channel(port)?.fin_sent {
            self.tcp(port, 4, &[])?;
        }
        self.channels.remove(&port);
        Ok(())
    }

    pub fn finish(&mut self, port: u16) -> Result<()> {
        if !self.drained(port)? || self.channel(port)?.fin_sent {
            return Err(INVALID);
        }
        self.tcp(port, 0x11, &[])?;
        let channel = self.channel_mut(port)?;
        channel.fin_sent = true;
        channel.send_next = channel.send_next.wrapping_add(1);
        Ok(())
    }

    pub fn drained(&self, port: u16) -> Result<bool> {
        let channel = self.channel(port)?;
        Ok(channel.output.is_empty() && channel.send_next == channel.send_ack)
    }

    pub fn send(&mut self, port: u16, bytes: &[u8]) -> Result<()> {
        let channel = self.channel_mut(port)?;
        if channel.closed || channel.fin_sent || bytes.len() > CAPACITY - channel.output.len() {
            return Err(INVALID);
        }
        channel.output.extend(bytes);
        Ok(())
    }

    pub fn send_plist(&mut self, port: u16, value: &Value, little_endian: bool) -> Result<()> {
        let bytes = if little_endian {
            t1_bridge::bplist::encode(value).map_err(|_| INVALID)?
        } else {
            plist::encode_xml(value)?
        };
        let size = u32::try_from(bytes.len()).map_err(|_| INVALID)?;
        self.send(
            port,
            &if little_endian {
                size.to_le_bytes()
            } else {
                size.to_be_bytes()
            },
        )?;
        self.send(port, &bytes)
    }

    pub fn receive_plist(&mut self, port: u16, little_endian: bool) -> Result<Option<Value>> {
        let channel = self.channel(port)?;
        if channel.input.len() < 4 {
            return Ok(None);
        }
        let length: [u8; 4] = channel
            .input
            .iter()
            .take(4)
            .copied()
            .collect::<Vec<_>>()
            .try_into()
            .map_err(|_| INVALID)?;
        let length = if little_endian {
            u32::from_le_bytes(length)
        } else {
            u32::from_be_bytes(length)
        } as usize;
        if length == 0 || length > t1_bridge::bplist::MAX_PLIST_SIZE {
            return Err(INVALID);
        }
        if channel.input.len() < length + 4 {
            return Ok(None);
        }
        let bytes = self.take(port, length + 4)?.ok_or(INVALID)?;
        Ok(Some(plist::decode(&bytes[4..])?))
    }

    pub fn available(&self, port: u16) -> Result<usize> {
        Ok(self.channel(port)?.input.len())
    }

    pub fn peek(&self, port: u16, count: usize) -> Result<Option<Vec<u8>>> {
        let channel = self.channel(port)?;
        Ok((channel.input.len() >= count)
            .then(|| channel.input.iter().take(count).copied().collect()))
    }

    pub fn take(&mut self, port: u16, count: usize) -> Result<Option<Vec<u8>>> {
        if count == 0 {
            return Ok(Some(vec![]));
        }
        let channel = self.channel_mut(port)?;
        if channel.input.len() < count {
            return Ok(None);
        }
        let bytes = channel.input.drain(..count).collect();
        if channel.connected && !channel.closed {
            self.tcp(port, 0x10, &[])?;
        }
        Ok(Some(bytes))
    }

    pub fn pending(&self, port: u16) -> Result<usize> {
        Ok(self.channel(port)?.output.len())
    }

    fn channel(&self, port: u16) -> Result<&Channel> {
        self.channels.get(&port).ok_or(INVALID)
    }
    fn channel_mut(&mut self, port: u16) -> Result<&mut Channel> {
        self.channels.get_mut(&port).ok_or(INVALID)
    }

    pub fn poll(&mut self) -> Result<()> {
        self.flush()?;
        let mut bytes = vec![0; MAX_PACKET];
        match self.transport.receive(&mut bytes) {
            Ok(size) => self.bytes.extend_from_slice(&bytes[..size]),
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::TimedOut
                        | io::ErrorKind::WouldBlock
                        | io::ErrorKind::Interrupted
                ) =>
            {
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }
        if self.bytes.len() > MAX_PACKET * 2 {
            return Err(INVALID);
        }
        while self.bytes.len() >= 8 {
            let length = be32(&self.bytes[4..8]) as usize;
            let header = if self.version >= 2 { 16 } else { 8 };
            if length < header || length > MAX_PACKET {
                return Err(INVALID);
            }
            if self.bytes.len() < length {
                break;
            }
            let packet: Vec<_> = self.bytes.drain(..length).collect();
            if self.version >= 2 {
                if be32(&packet[8..12]) != 0xfeed_face {
                    return Err(INVALID);
                }
                self.receive_sequence = be16(&packet[12..14]);
            }
            match be32(&packet[..4]) {
                0 if self.version == 0 && length == 20 => {
                    self.version = be32(&packet[8..12]);
                    if !matches!(self.version, 1 | 2) {
                        return Err(INVALID);
                    }
                    if self.version == 2 {
                        self.packet(2, &[7])?;
                    }
                }
                1 => {
                    if packet.get(header) == Some(&3) {
                        return Err(Error("T1 reported a USB multiplexing error"));
                    }
                }
                6 if self.version != 0 => self.input_tcp(&packet[header..])?,
                _ => return Err(INVALID),
            }
        }
        if self
            .channels
            .values()
            .map(|c| c.input.len() + c.output.len())
            .sum::<usize>()
            > 64 * 1024 * 1024
        {
            return Err(INVALID);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        let ports: Vec<_> = self.channels.keys().copied().collect();
        for port in ports {
            let channel = self.channel_mut(port)?;
            if !channel.connected || channel.closed {
                continue;
            }
            let in_flight = channel.send_next.wrapping_sub(channel.send_ack);
            let sendable = channel.window.saturating_sub(in_flight) as usize;
            let count = sendable.min(PAYLOAD).min(channel.output.len());
            if count != 0 {
                let data: Vec<_> = channel.output.drain(..count).collect();
                self.tcp(port, 0x10, &data)?;
                let channel = self.channel_mut(port)?;
                channel.send_next = channel
                    .send_next
                    .wrapping_add(u32::try_from(count).map_err(|_| INVALID)?);
            }
        }
        Ok(())
    }

    fn input_tcp(&mut self, packet: &[u8]) -> Result<()> {
        if packet.len() < 20 {
            return Err(INVALID);
        }
        let header = usize::from(packet[12] >> 4) * 4;
        if header < 20 || header > packet.len() {
            return Err(INVALID);
        }
        let source = be16(&packet[..2]);
        let port = be16(&packet[2..4]);
        let sequence = be32(&packet[4..8]);
        let ack = be32(&packet[8..12]);
        let flags = packet[13];
        let Some(channel) = self.channels.get_mut(&port) else {
            return Ok(());
        };
        if source != channel.destination {
            return Err(INVALID);
        }
        if flags & 4 != 0 {
            channel.closed = true;
            return Ok(());
        }
        if channel.closed {
            return Ok(());
        }
        channel.window = u32::from(be16(&packet[14..16])) << 8;
        if !channel.connected {
            if flags != 0x12 || ack != 1 || packet.len() != header {
                return Err(INVALID);
            }
            channel.receive_next = sequence.wrapping_add(1);
            channel.send_ack = ack;
            channel.connected = true;
            return self.tcp(port, 0x10, &[]);
        }
        if flags & 0x10 == 0
            || sequence != channel.receive_next
            || ack.wrapping_sub(channel.send_ack) > channel.send_next.wrapping_sub(channel.send_ack)
        {
            return Err(INVALID);
        }
        channel.send_ack = ack;
        let data = &packet[header..];
        if data.len() > CAPACITY - channel.input.len() {
            return Err(INVALID);
        }
        channel.input.extend(data);
        channel.receive_next = channel
            .receive_next
            .wrapping_add(u32::try_from(data.len()).map_err(|_| INVALID)?);
        if flags & 1 != 0 {
            channel.receive_next = channel.receive_next.wrapping_add(1);
            channel.closed = true;
        }
        if !data.is_empty() || flags & 1 != 0 {
            self.tcp(port, 0x10, &[])?;
        }
        Ok(())
    }

    fn tcp(&mut self, port: u16, flags: u8, data: &[u8]) -> Result<()> {
        let channel = self.channel(port)?;
        let mut packet = Vec::with_capacity(20 + data.len());
        packet.extend(port.to_be_bytes());
        packet.extend(channel.destination.to_be_bytes());
        packet.extend(channel.send_next.to_be_bytes());
        packet.extend(channel.receive_next.to_be_bytes());
        packet.extend([0x50, flags]);
        let window = ((CAPACITY - channel.input.len()) >> 8).min(usize::from(u16::MAX));
        packet.extend(u16::try_from(window).map_err(|_| INVALID)?.to_be_bytes());
        packet.extend([0, 0, 0, 0]);
        packet.extend(data);
        self.packet(6, &packet)
    }

    fn packet(&mut self, protocol: u32, data: &[u8]) -> Result<()> {
        let header = if self.version >= 2 { 16 } else { 8 };
        let mut packet = Vec::with_capacity(header + data.len());
        packet.extend(protocol.to_be_bytes());
        packet.extend(
            u32::try_from(header + data.len())
                .map_err(|_| INVALID)?
                .to_be_bytes(),
        );
        if self.version >= 2 {
            packet.extend(0xfeed_face_u32.to_be_bytes());
            packet.extend(self.send_sequence.to_be_bytes());
            packet.extend(self.receive_sequence.to_be_bytes());
            self.send_sequence = self.send_sequence.wrapping_add(1);
        }
        packet.extend(data);
        self.transport.send(&packet)
    }
}

fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}
fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Wire {
        output: RefCell<Vec<Vec<u8>>>,
    }
    impl Transport for Wire {
        fn send(&self, bytes: &[u8]) -> Result<()> {
            self.output.borrow_mut().push(bytes.to_vec());
            Ok(())
        }
        fn receive(&self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::TimedOut.into())
        }
    }

    fn packet(port: u16, sequence: u32, ack: u32, flags: u8, data: &[u8]) -> Vec<u8> {
        let mut p = vec![];
        p.extend(62078_u16.to_be_bytes());
        p.extend(port.to_be_bytes());
        p.extend(sequence.to_be_bytes());
        p.extend(ack.to_be_bytes());
        p.extend([0x50, flags, 0x10, 0, 0, 0, 0, 0]);
        p.extend(data);
        p
    }

    #[test]
    fn fragmented_channel_frames_and_sequence_errors() {
        let mut mux = Mux::new(Wire {
            output: RefCell::new(vec![]),
        })
        .unwrap();
        mux.version = 2;
        let port = mux.connect(62078).unwrap();
        mux.input_tcp(&packet(port, 0, 1, 0x12, &[])).unwrap();
        assert!(mux.connected(port).unwrap());
        let bytes = plist::encode_xml(&plist::dict([("Synthetic", Value::Boolean(true))])).unwrap();
        let mut frame = u32::try_from(bytes.len()).unwrap().to_be_bytes().to_vec();
        frame.extend(bytes);
        mux.input_tcp(&packet(port, 1, 1, 0x10, &frame[..2]))
            .unwrap();
        assert!(mux.receive_plist(port, false).unwrap().is_none());
        mux.input_tcp(&packet(port, 3, 1, 0x10, &frame[2..]))
            .unwrap();
        assert!(mux.receive_plist(port, false).unwrap().is_some());
        assert!(
            mux.input_tcp(&packet(port, 1, 1, 0x10, b"duplicate"))
                .is_err()
        );
    }

    #[test]
    fn refuses_oversize_frame_before_allocation() {
        let mut mux = Mux::new(Wire {
            output: RefCell::new(vec![]),
        })
        .unwrap();
        mux.version = 1;
        let port = mux.connect(62078).unwrap();
        mux.input_tcp(&packet(port, 0, 1, 0x12, &[])).unwrap();
        mux.input_tcp(&packet(port, 1, 1, 0x10, &u32::MAX.to_be_bytes()))
            .unwrap();
        assert!(mux.receive_plist(port, false).is_err());
    }
}
