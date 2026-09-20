//! Direct Linux usbfs transport. Discovery binds the entire attempt to one
//! physical USB location and refuses ambiguous or functional devices.

use super::{Error, Result, firmware::hex};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::{
    fd::AsFd,
    unix::fs::{FileTypeExt, OpenOptionsExt},
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use t1_platform::recovery_io::{self, Control};

pub(super) const RECOVERY: u16 = 0x1281;
pub(super) const BOOTED: u16 = 0x8600;
const INVALID: Error = Error("T1 USB identity or recovery interface is invalid");

pub(super) struct Identity {
    pub ecid: u64,
    pub ap_nonce: Vec<u8>,
    pub sep_nonce: Vec<u8>,
}

#[derive(Clone)]
pub(super) struct Location(PathBuf);

pub(super) struct Device {
    pub location: Location,
    pub product: u16,
    pub bus: u32,
    pub address: u32,
    pub has_hid: bool,
    configuration: u8,
    descriptors: Vec<u8>,
}

pub(super) fn supported_machine() -> Result<()> {
    let vendor = fs::read_to_string("/sys/class/dmi/id/sys_vendor")?;
    let model = fs::read_to_string("/sys/class/dmi/id/product_name")?;
    if vendor.trim() != "Apple Inc."
        || !matches!(
            model.trim(),
            "MacBookPro13,2" | "MacBookPro13,3" | "MacBookPro14,2" | "MacBookPro14,3"
        )
    {
        return Err(Error(
            "native online recovery supports only T1 MacBook Pro models",
        ));
    }
    Ok(())
}

pub(super) fn discover() -> Result<Option<Device>> {
    let mut found = None;
    for entry in fs::read_dir("/sys/bus/usb/devices")? {
        let path = entry?.path();
        let vendor = match fs::read_to_string(path.join("idVendor")) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        if vendor.trim() != "05ac" {
            continue;
        }
        let product = match fs::read_to_string(path.join("idProduct")) {
            Ok(product) => u16::from_str_radix(product.trim(), 16).map_err(|_| INVALID)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        if product != RECOVERY && product != BOOTED {
            continue;
        }
        if found.is_some() {
            return Err(Error(
                "multiple T1 candidates found; recovery refuses to choose",
            ));
        }
        let inspected = inspect_device(&path, product);
        match inspected {
            Ok(device) => found = Some(device),
            Err(_) if !path.try_exists()? => {}
            Err(error) => return Err(error),
        }
    }
    Ok(found)
}

fn inspect_device(path: &Path, product: u16) -> Result<Device> {
    let mut descriptors = Vec::new();
    File::open(path.join("descriptors"))?
        .take(65_537)
        .read_to_end(&mut descriptors)?;
    if descriptors.len() > 65_536 {
        return Err(INVALID);
    }
    let configuration = fs::read_to_string(path.join("bConfigurationValue"))?;
    let configuration = if configuration.trim().is_empty() {
        0
    } else {
        configuration.trim().parse::<u8>().map_err(|_| INVALID)?
    };
    let interfaces = interfaces(&descriptors, 0)?;
    Ok(Device {
        location: Location(path.canonicalize()?),
        product,
        bus: number(&path.join("busnum"))?,
        address: number(&path.join("devnum"))?,
        has_hid: interfaces.iter().any(|i| i.class == 3),
        configuration,
        descriptors,
    })
}

fn number(path: &Path) -> Result<u32> {
    fs::read_to_string(path)?
        .trim()
        .parse()
        .map_err(|_| INVALID)
}

pub(super) fn wait(
    location: &Location,
    product: u16,
    previous_address: Option<u32>,
    timeout: Duration,
) -> Result<Device> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(device) = discover()? {
            if device.location.0 != location.0 {
                return Err(Error("T1 physical device changed during recovery"));
            }
            if device.product == product && previous_address != Some(device.address) {
                return Ok(device);
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err(Error(
        "T1 did not reach the expected USB state before the deadline",
    ))
}

pub(super) fn prove_boot(location: &Location) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let device = discover()?.ok_or(Error("T1 disappeared during boot verification"))?;
        if device.location.0 != location.0 || device.product != BOOTED || !device.has_hid {
            return Err(Error("T1 did not sustain its functional USB configuration"));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Ok(())
}

struct Interface {
    number: u8,
    alternate: u8,
    class: u8,
    subclass: u8,
    protocol: u8,
    configuration: u8,
    input: u8,
    output: u8,
    packet: u16,
}

fn interfaces(bytes: &[u8], active: u8) -> Result<Vec<Interface>> {
    let mut result: Vec<Interface> = Vec::new();
    let mut configuration = 0;
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes.len() - offset < 2 {
            return Err(INVALID);
        }
        let len = usize::from(bytes[offset]);
        if len < 2 || len > bytes.len() - offset {
            return Err(INVALID);
        }
        let d = &bytes[offset..offset + len];
        match d[1] {
            2 if len >= 9 => configuration = d[5],
            4 if len >= 9 && (active == 0 || configuration == active) => result.push(Interface {
                number: d[2],
                alternate: d[3],
                class: d[5],
                subclass: d[6],
                protocol: d[7],
                input: 0,
                output: 0,
                packet: 0,
                configuration,
            }),
            5 if len >= 7 && (active == 0 || configuration == active) && d[3] & 3 == 2 => {
                let interface = result.last_mut().ok_or(INVALID)?;
                if d[2] & 0x80 != 0 {
                    if interface.input != 0 {
                        return Err(INVALID);
                    }
                    interface.input = d[2];
                } else {
                    if interface.output != 0 {
                        return Err(INVALID);
                    }
                    interface.output = d[2];
                    interface.packet = u16::from_le_bytes([d[4], d[5]]);
                }
            }
            _ => {}
        }
        offset += len;
    }
    Ok(result)
}

pub(super) struct Usb {
    file: File,
    interface: u8,
    pub input: u8,
    pub output: u8,
    packet: usize,
}

impl Usb {
    pub fn open(device: &Device) -> Result<Self> {
        if device.has_hid {
            return Err(Error("functional T1 detected; online recovery refused"));
        }
        let candidates: Vec<_> = interfaces(&device.descriptors, device.configuration)?
            .into_iter()
            .filter(|i| {
                i.alternate == 0
                    && if device.product == RECOVERY {
                        i.output == 0x04
                    } else {
                        i.class == 0xff
                            && i.subclass == 0xfe
                            && i.protocol == 2
                            && i.input != 0
                            && i.output != 0
                    }
            })
            .collect();
        let [interface] = candidates.as_slice() else {
            return Err(INVALID);
        };
        if interface.packet == 0 || interface.packet > 1024 {
            return Err(INVALID);
        }
        let path = format!("/dev/bus/usb/{:03}/{:03}", device.bus, device.address);
        // Linux O_NOFOLLOW: usbfs device nodes must not be redirected by links.
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0o400_000)
            .open(path)?;
        if !file.metadata()?.file_type().is_char_device() {
            return Err(INVALID);
        }
        let mut descriptor = [0; 18];
        let count = recovery_io::usb_control(
            file.as_fd(),
            Control {
                kind: 0x80,
                request: 6,
                value: 0x100,
                index: 0,
            },
            &mut descriptor,
            5_000,
        )?;
        if count != 18
            || descriptor[8..12]
                != [
                    0xac,
                    0x05,
                    device.product.to_le_bytes()[0],
                    device.product.to_le_bytes()[1],
                ]
        {
            return Err(INVALID);
        }
        if device.configuration == 0 {
            recovery_io::usb_configuration(file.as_fd(), interface.configuration)?;
        }
        recovery_io::usb_claim(file.as_fd(), interface.number)?;
        Ok(Self {
            file,
            interface: interface.number,
            input: interface.input,
            output: interface.output,
            packet: interface.packet.into(),
        })
    }

    pub fn identity(&self, device: &Device) -> Result<Identity> {
        let serial = fs::read_to_string(device.location.0.join("serial"))?;
        let nonce = self.string_descriptor(1)?;
        parse_identity(&serial, &nonce)
    }

    fn string_descriptor(&self, index: u8) -> Result<String> {
        let mut bytes = [0; 255];
        let size = self.control(
            Control {
                kind: 0x80,
                request: 6,
                value: 0x300 | u16::from(index),
                index: 0x409,
            },
            &mut bytes,
            10_000,
        )?;
        if size < 2 || bytes[1] != 3 || usize::from(bytes[0]) != size || !size.is_multiple_of(2) {
            return Err(INVALID);
        }
        let chars: Vec<_> = bytes[2..size]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        String::from_utf16(&chars).map_err(|_| INVALID)
    }

    pub fn control(&self, request: Control, bytes: &mut [u8], timeout: u32) -> Result<usize> {
        recovery_io::usb_control(self.file.as_fd(), request, bytes, timeout).map_err(Into::into)
    }

    pub fn command(&self, command: &str, blind: bool) -> Result<()> {
        if command.len() >= 255 || command.contains('\0') {
            return Err(INVALID);
        }
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(0);
        let count = self.control(
            Control {
                kind: 0x40,
                request: u8::from(blind),
                value: 0,
                index: 0,
            },
            &mut bytes,
            10_000,
        )?;
        if count != bytes.len() {
            return Err(Error("short iBoot command transfer"));
        }
        Ok(())
    }

    pub fn upload(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 {
            return Err(INVALID);
        }
        self.control(
            Control {
                kind: 0x41,
                request: 0,
                value: 0,
                index: 0,
            },
            &mut [],
            10_000,
        )?;
        for chunk in bytes.chunks(0x8000) {
            self.write(chunk, false)?;
        }
        Ok(())
    }

    pub fn write(&self, bytes: &[u8], mux: bool) -> Result<()> {
        let mut owned = bytes.to_vec();
        let size = recovery_io::usb_bulk(self.file.as_fd(), self.output, &mut owned, 10_000)?;
        if size != bytes.len() {
            return Err(Error("short USB bulk transfer"));
        }
        if mux && size.is_multiple_of(self.packet) {
            recovery_io::usb_bulk(self.file.as_fd(), self.output, &mut [], 10_000)?;
        }
        Ok(())
    }

    pub fn read(&self, bytes: &mut [u8]) -> io::Result<usize> {
        recovery_io::usb_bulk(self.file.as_fd(), self.input, bytes, 100)
    }
}

impl Drop for Usb {
    fn drop(&mut self) {
        recovery_io::usb_release(self.file.as_fd(), self.interface);
    }
}

fn field<'a>(text: &'a str, name: &str) -> Result<&'a str> {
    let prefix = format!("{name}:");
    let mut values = text
        .split_ascii_whitespace()
        .filter_map(|part| part.strip_prefix(&prefix));
    let value = values.next().ok_or(INVALID)?;
    if values.next().is_some() {
        return Err(INVALID);
    }
    Ok(value)
}

fn parse_identity(serial: &str, nonce: &str) -> Result<Identity> {
    let number = |key| u64::from_str_radix(field(serial, key)?, 16).map_err(|_| INVALID);
    if number("CPID")? != 0x8002 || number("BDID")? != 0x12 {
        return Err(INVALID);
    }
    let ecid = number("ECID")?;
    let ap_nonce = hex(field(nonce, "NONC")?)?;
    let sep_nonce = hex(field(nonce, "SNON")?)?;
    if ecid == 0 || !matches!(ap_nonce.len(), 20 | 32) || !matches!(sep_nonce.len(), 20 | 32) {
        return Err(INVALID);
    }
    Ok(Identity {
        ecid,
        ap_nonce,
        sep_nonce,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_requires_t1_board_and_unambiguous_nonces() {
        let nonce = format!("NONC:{} SNON:{}", "aa".repeat(20), "bb".repeat(20));
        let serial = "CPID:8002 BDID:12 ECID:1234"; // Synthetic identifier.
        assert_eq!(parse_identity(serial, &nonce).unwrap().ecid, 0x1234);
        assert!(parse_identity("CPID:8003 BDID:12 ECID:1234", &nonce).is_err());
        assert!(parse_identity(serial, &(nonce.clone() + " NONC:aa")).is_err());
        assert!(parse_identity(serial, "NONC:aa SNON:bb").is_err());
    }

    #[test]
    fn descriptors_reject_truncated_and_ambiguous_endpoints() {
        assert!(interfaces(&[0, 4], 1).is_err());
        assert!(interfaces(&[9, 2], 1).is_err());
        let mut bytes = vec![
            9, 2, 0, 0, 1, 1, 0, 0, 0, 9, 4, 0, 0, 2, 255, 254, 2, 0, 7, 5, 1, 2, 0, 2, 0,
        ];
        assert_eq!(interfaces(&bytes, 1).unwrap()[0].output, 1);
        bytes.extend([7, 5, 2, 2, 0, 2, 0]);
        assert!(interfaces(&bytes, 1).is_err());
    }
}
