//! Apple TSS requests and Image4 personalization owned by this project.

use super::{
    Error, Result,
    firmware::Firmware,
    plist::{self, Value},
    usb::Identity,
};
use t1_platform::recovery_io::{self, AppleEndpoint};

const INVALID: Error = Error("Apple signing response or Image4 component is invalid");

/// A ticket is retained with every component personalized from that response.
/// Neither type implements Debug: the ticket identifies the device.
pub(super) struct SignedFirmware {
    pub ticket: Vec<u8>,
    pub combined: Vec<u8>,
    components: std::collections::BTreeMap<String, Vec<u8>>,
}

impl SignedFirmware {
    pub fn request(firmware: &Firmware, device: &Identity) -> Result<Self> {
        use std::io::Read;
        let mut request = signing_request(&firmware.identity, device)?;
        let mut random = [0; 16];
        std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
        random[6] = (random[6] & 15) | 0x40;
        random[8] = (random[8] & 63) | 0x80;
        let hex = format!("{:032x}", u128::from_be_bytes(random));
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        );
        let Value::Dictionary(fields) = &mut request else {
            return Err(INVALID);
        };
        fields.insert("@UUID".into(), plist::string(&uuid));
        let response =
            recovery_io::https(AppleEndpoint::Signing, Some(&plist::encode_xml(&request)?))?;
        let response = signing_response(&response)?;
        let ticket = plist::data(plist::get(&response, "ApImg4Ticket")?)?.to_vec();
        if ticket.len() > 1024 * 1024 {
            return Err(INVALID);
        }
        let mut components = std::collections::BTreeMap::new();
        for name in [
            "OSRamdisk",
            "KernelCache",
            "DeviceTree",
            "SEP",
            "LLB",
            "iBoot",
            "RestoreRamDisk",
            "RestoreKernelCache",
            "RestoreDeviceTree",
            "RestoreSEP",
            "iBEC",
        ] {
            if plist::dictionary(&response)?.contains_key(&format!("{name}-TBM")) {
                return Err(INVALID);
            }
            components.insert(
                name.into(),
                personalize(name, firmware.component(name)?, &ticket)?,
            );
        }
        let mut combined = Vec::new();
        for name in ["OSRamdisk", "KernelCache", "DeviceTree", "SEP"] {
            combined.extend_from_slice(components.get(name).ok_or(INVALID)?);
        }
        Ok(Self {
            ticket,
            combined,
            components,
        })
    }

    pub fn component(&self, name: &str) -> Result<&[u8]> {
        self.components.get(name).map(Vec::as_slice).ok_or(INVALID)
    }
}

fn signing_request(identity: &Value, device: &Identity) -> Result<Value> {
    let Value::Dictionary(mut request) = plist::dict([
        ("@HostPlatformInfo", plist::string("mac")),
        ("@VersionInfo", plist::string("libauthinstall-973.0.1")),
        ("@ApImg4Ticket", Value::Boolean(true)),
        ("ApECID", Value::Integer(device.ecid.into())),
        ("ApNonce", Value::Data(device.ap_nonce.clone())),
        ("SepNonce", Value::Data(device.sep_nonce.clone())),
        ("ApProductionMode", Value::Boolean(true)),
        ("ApSecurityMode", Value::Boolean(true)),
    ]) else {
        return Err(INVALID);
    };
    for key in ["ApChipID", "ApBoardID", "ApSecurityDomain"] {
        request.insert(
            key.into(),
            Value::Integer(plist::integer(plist::get(identity, key)?)?.into()),
        );
    }
    request.insert(
        "UniqueBuildID".into(),
        plist::get(identity, "UniqueBuildID")?.clone(),
    );
    for (name, component) in plist::dictionary(plist::get(identity, "Manifest")?)? {
        let mut entry = plist::dictionary(component)?.clone();
        let info = entry.remove("Info").ok_or(INVALID)?;
        let trusted = entry.get("Trusted") == Some(&Value::Boolean(true));
        let rules = plist::dictionary(&info)?.get("RestoreRequestRules");
        if !trusted && rules.is_none() {
            continue;
        }
        if let Some(rules) = rules {
            apply_rules(&mut entry, rules)?;
        } else {
            entry.insert("EPRO".into(), Value::Boolean(true));
            entry.insert("ESEC".into(), Value::Boolean(true));
        }
        if trusted {
            entry
                .entry("Digest".into())
                .or_insert_with(|| Value::Data(vec![]));
        }
        request.insert(name.clone(), Value::Dictionary(entry));
    }
    Ok(Value::Dictionary(request))
}

fn apply_rules(entry: &mut std::collections::BTreeMap<String, Value>, rules: &Value) -> Result<()> {
    let Value::Array(rules) = rules else {
        return Err(INVALID);
    };
    for rule in rules {
        let mut matches = true;
        for (key, value) in plist::dictionary(plist::get(rule, "Conditions")?)? {
            let expected = match key.as_str() {
                "ApRawProductionMode"
                | "ApCurrentProductionMode"
                | "ApRawSecurityMode"
                | "ApRequiresImage4" => Value::Boolean(true),
                "ApInRomDFU" => Value::Boolean(false),
                // An unknown condition must never enable a signing rule.
                _ => {
                    matches = false;
                    continue;
                }
            };
            matches &= value == &expected;
        }
        if matches {
            for (key, value) in plist::dictionary(plist::get(rule, "Actions")?)? {
                if !matches!(value, Value::Boolean(_)) {
                    return Err(INVALID);
                }
                entry.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(())
}

fn signing_response(response: &[u8]) -> Result<Value> {
    let marker = b"&REQUEST_STRING=";
    let start = response
        .windows(marker.len())
        .position(|w| w == marker)
        .ok_or(INVALID)?;
    let header = std::str::from_utf8(&response[..start]).map_err(|_| INVALID)?;
    let fields: Vec<_> = header.split('&').collect();
    if fields.iter().filter(|f| f.starts_with("STATUS=")).count() != 1
        || !fields.contains(&"STATUS=0")
    {
        return Err(Error("Apple declined the firmware signing request"));
    }
    plist::decode(&response[start + marker.len()..])
}

/// Minimal definite-length DER framing. Apple signs the manifest; we preserve
/// its bytes and the payload, changing only the documented restore tag.
fn element(bytes: &[u8], tag: u8) -> Result<(std::ops::Range<usize>, usize)> {
    if bytes.first() != Some(&tag) || bytes.len() < 2 {
        return Err(INVALID);
    }
    let (length, start) = if bytes[1] < 128 {
        (usize::from(bytes[1]), 2)
    } else {
        let count = usize::from(bytes[1] & 127);
        if count == 0 || count > 4 || bytes.len() < count + 2 || bytes[2] == 0 {
            return Err(INVALID);
        }
        let length = bytes[2..2 + count]
            .iter()
            .fold(0_usize, |n, b| (n << 8) | usize::from(*b));
        if length < 128 {
            return Err(INVALID);
        }
        (length, count + 2)
    };
    let end = start + length;
    if end > bytes.len() {
        return Err(INVALID);
    }
    Ok((start..end, end))
}

fn wrap(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut bytes = vec![tag];
    if content.len() < 128 {
        bytes.push(u8::try_from(content.len()).expect("short DER length"));
    } else {
        let length = content.len().to_be_bytes();
        let first = length.iter().position(|b| *b != 0).expect("nonzero length");
        bytes.push(0x80 | u8::try_from(length.len() - first).expect("machine word size"));
        bytes.extend_from_slice(&length[first..]);
    }
    bytes.extend_from_slice(content);
    bytes
}

fn personalize(name: &str, payload: &[u8], ticket: &[u8]) -> Result<Vec<u8>> {
    let (body, consumed) = element(payload, 0x30)?;
    if consumed != payload.len() {
        return Err(INVALID);
    }
    let (magic, end) = element(&payload[body.clone()], 0x16)?;
    if &payload[body.start + magic.start..body.start + magic.end] != b"IM4P" {
        return Err(INVALID);
    }
    let offset = body.start + end;
    let (tag, _) = element(&payload[offset..], 0x16)?;
    if tag.len() != 4 {
        return Err(INVALID);
    }
    let (_, consumed) = element(ticket, 0x30)?;
    if consumed != ticket.len() {
        return Err(INVALID);
    }
    let mut payload = payload.to_vec();
    let replacement = match name {
        "RestoreKernelCache" => Some(b"rkrn"),
        "RestoreDeviceTree" => Some(b"rdtr"),
        "RestoreSEP" => Some(b"rsep"),
        _ => None,
    };
    if let Some(replacement) = replacement {
        payload[offset + tag.start..offset + tag.end].copy_from_slice(replacement);
    }
    let mut image = wrap(0x16, b"IMG4");
    image.extend_from_slice(&payload);
    image.extend_from_slice(&wrap(0xa0, ticket));
    Ok(wrap(0x30, &image))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires Apple's pinned package outside the repository; no network or USB"]
    fn personalize_pinned_package_with_synthetic_ticket() {
        let path = std::env::var_os("T1BRIDGE_RECOVERY_PACKAGE").expect("package path required");
        let firmware = Firmware::from_package(&std::fs::read(path).unwrap()).unwrap();
        let ticket = wrap(0x30, &wrap(0x16, b"synthetic ticket"));
        for name in [
            "OSRamdisk",
            "KernelCache",
            "DeviceTree",
            "SEP",
            "LLB",
            "iBoot",
            "iBEC",
            "RestoreRamDisk",
            "RestoreKernelCache",
            "RestoreDeviceTree",
            "RestoreSEP",
        ] {
            personalize(name, firmware.component(name).unwrap(), &ticket).unwrap();
        }
        let identity = Identity {
            ecid: 0x1234,
            ap_nonce: vec![0xaa; 20],
            sep_nonce: vec![0xbb; 20],
        };
        let request = signing_request(&firmware.identity, &identity).unwrap();
        assert_eq!(
            plist::decode(&plist::encode_xml(&request).unwrap()).unwrap(),
            request
        );
    }

    #[test]
    fn rejects_server_error_duplicate_status_and_missing_ticket() {
        assert!(
            signing_response(b"STATUS=94&MESSAGE=denied&REQUEST_STRING=<plist><dict/></plist>")
                .is_err()
        );
        assert!(
            signing_response(b"STATUS=0&STATUS=1&REQUEST_STRING=<plist><dict/></plist>").is_err()
        );
        assert!(signing_response(b"STATUS=0&MESSAGE=SUCCESS").is_err());
    }

    #[test]
    fn wraps_exact_ticket_and_retags_only_restore_payload() {
        let mut inner = wrap(0x16, b"IM4P");
        inner.extend(wrap(0x16, b"krnl"));
        inner.extend(wrap(4, b"synthetic payload krnl"));
        let payload = wrap(0x30, &inner);
        let ticket = wrap(0x30, &wrap(0x16, b"synthetic ticket"));
        let image = personalize("RestoreKernelCache", &payload, &ticket).unwrap();
        assert!(image.windows(4).any(|w| w == b"rkrn"));
        assert!(image.windows(ticket.len()).any(|w| w == ticket));
        assert!(
            image
                .windows(b"synthetic payload krnl".len())
                .any(|w| w == b"synthetic payload krnl")
        );
        assert!(personalize("SEP", b"not DER", &ticket).is_err());
        let mut trailing = payload.clone();
        trailing.push(0);
        assert!(personalize("SEP", &trailing, &ticket).is_err());
    }
}
