//! Pinned Apple package verification and extraction into bounded memory.

use super::{
    Error, Result,
    plist::{self, Value},
};
use crate::archive::{CpioReader, PbzxEncoding, PbzxReader};
use std::collections::BTreeMap;
use std::io::Read;
use t1_platform::{recovery_io, xz};

const PACKAGE_SIZE: usize = 59_314_427;
const PACKAGE_HASH: &str = "0c97ab746ec635b34b1bdea4e4722cd0173443e2ba6ede54cc6af3b5e220d230";
const BUNDLE: &str = "usr/standalone/firmware/iBridge1_1Customer.bundle/Contents/";
const MAX_EXPANDED: usize = 128 * 1024 * 1024;
const INVALID: Error = Error("firmware package is incomplete, unverified or incompatible");

pub(super) struct Firmware {
    pub identity: Value,
    pub version: Vec<u8>,
    resources: BTreeMap<String, Vec<u8>>,
}

impl Firmware {
    pub fn download() -> Result<Self> {
        Self::from_package(&recovery_io::https(
            recovery_io::AppleEndpoint::Firmware,
            None,
        )?)
    }

    pub fn from_package(package: &[u8]) -> Result<Self> {
        if package.len() != PACKAGE_SIZE || recovery_io::sha256(package)? != hex(PACKAGE_HASH)?[..]
        {
            return Err(INVALID);
        }
        let payload = recovery_io::xar_member(package, "Payload", PACKAGE_SIZE)?;
        let expanded = expand(&payload)?;
        let mut cpio = CpioReader::new(expanded.as_slice()).map_err(|_| INVALID)?;
        let mut files = BTreeMap::new();
        let mut count = 0;
        while let Some(mut entry) = cpio.next_entry().map_err(|_| INVALID)? {
            count += 1;
            if count > 1024 {
                return Err(INVALID);
            }
            let path = std::str::from_utf8(&entry.header().path).map_err(|_| INVALID)?;
            let path = path.strip_prefix("./").unwrap_or(path);
            if let Some(path) = path.strip_prefix(BUNDLE)
                && entry.header().is_regular_file()
            {
                if entry.header().file_size > 32 * 1024 * 1024
                    || path.split('/').any(|p| p == ".." || p.is_empty())
                {
                    return Err(INVALID);
                }
                let name = path.to_owned();
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes)?;
                if files.insert(name, bytes).is_some() {
                    return Err(INVALID);
                }
            }
            entry.drain().map_err(|_| INVALID)?;
        }
        let version = files.remove("version.plist").ok_or(INVALID)?;
        let manifest = plist::decode(files.get("Resources/BuildManifest.plist").ok_or(INVALID)?)?;
        let identity = choose_identity(&manifest)?;
        let firmware = Self {
            identity,
            version,
            resources: files,
        };
        for component in [
            "OSRamdisk",
            "KernelCache",
            "DeviceTree",
            "SEP",
            "RestoreRamDisk",
            "RestoreKernelCache",
            "RestoreDeviceTree",
            "RestoreSEP",
            "iBEC",
        ] {
            firmware.component(component)?;
        }
        firmware.nor_components()?;
        Ok(firmware)
    }

    pub fn component(&self, name: &str) -> Result<&[u8]> {
        let component = plist::get(plist::get(&self.identity, "Manifest")?, name)?;
        let path = plist::text(plist::get(plist::get(component, "Info")?, "Path")?)?;
        self.resources
            .get(&format!("Resources/{path}"))
            .map(Vec::as_slice)
            .ok_or(INVALID)
    }

    pub fn nor_components(&self) -> Result<Vec<&'static str>> {
        let manifest = plist::get(&self.identity, "Manifest")?;
        let component_path = |name| -> Result<&str> {
            plist::text(plist::get(
                plist::get(plist::get(manifest, name)?, "Info")?,
                "Path",
            )?)
        };
        let llb = component_path("LLB")?;
        let (parent, _) = llb.rsplit_once('/').ok_or(INVALID)?;
        let contents = self
            .resources
            .get(&format!("Resources/{parent}/manifest"))
            .ok_or(INVALID)?;
        let names = std::str::from_utf8(contents).map_err(|_| INVALID)?;
        let mut images = Vec::new();
        for filename in names.lines().map(str::trim).filter(|s| !s.is_empty()) {
            let mut matched = None;
            for component in ["LLB", "iBoot", "DeviceTree", "SEP"] {
                if component_path(component)? == format!("{parent}/{filename}") {
                    matched = Some(component);
                    break;
                }
            }
            let component = matched.ok_or(INVALID)?;
            if component != "LLB" {
                if images.contains(&component) {
                    return Err(INVALID);
                }
                images.push(component);
            }
        }
        let index = images.iter().position(|c| *c == "iBoot").ok_or(INVALID)?;
        images.swap(0, index);
        Ok(images)
    }
}

fn choose_identity(manifest: &Value) -> Result<Value> {
    let Value::Array(identities) = plist::get(manifest, "BuildIdentities")? else {
        return Err(INVALID);
    };
    let mut found = None;
    for identity in identities {
        if plist::integer(plist::get(identity, "ApChipID")?)? != 0x8002
            || plist::integer(plist::get(identity, "ApBoardID")?)? != 0x12
        {
            continue;
        }
        let info = plist::get(identity, "Info")?;
        if plist::text(plist::get(info, "Variant")?)? != "Customer Boot" {
            continue;
        }
        if plist::text(plist::get(info, "DeviceClass")?)? != "x619ap" || found.is_some() {
            return Err(INVALID);
        }
        found = Some(identity.clone());
    }
    found.ok_or(INVALID)
}

fn expand(payload: &[u8]) -> Result<Vec<u8>> {
    let mut reader = PbzxReader::new(payload).map_err(|_| INVALID)?;
    let mut output = Vec::new();
    while let Some(mut chunk) = reader.next_chunk().map_err(|_| INVALID)? {
        let header = chunk.header();
        let length = usize::try_from(header.expanded_size).map_err(|_| INVALID)?;
        if length > MAX_EXPANDED - output.len() || header.archived_size > PACKAGE_SIZE as u64 {
            return Err(INVALID);
        }
        let mut compressed = Vec::new();
        chunk.read_to_end(&mut compressed)?;
        let start = output.len();
        output.resize(start + length, 0);
        match header.encoding {
            PbzxEncoding::Raw if compressed.len() == length => {
                output[start..].copy_from_slice(&compressed);
            }
            PbzxEncoding::Xz => {
                xz::decode_exact(&compressed, &mut output[start..], MAX_EXPANDED as u64)
                    .map_err(|_| INVALID)?;
            }
            PbzxEncoding::Raw => return Err(INVALID),
        }
    }
    Ok(output)
}

pub(super) fn hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) || value.len() > 1024 || !value.is_ascii() {
        return Err(INVALID);
    }
    (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(|_| INVALID))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrong_package_never_reaches_archive_parser() {
        assert!(Firmware::from_package(b"synthetic package").is_err());
    }

    #[test]
    fn rejects_oversize_pbzx_before_allocation() {
        let mut stream = b"pbzx".to_vec();
        stream.extend_from_slice(&(256_u64 * 1024 * 1024).to_be_bytes());
        stream.extend_from_slice(&(256_u64 * 1024 * 1024).to_be_bytes());
        stream.extend_from_slice(&0_u64.to_be_bytes());
        assert!(expand(&stream).is_err());
    }

    #[test]
    #[ignore = "requires an independently downloaded Apple package; never a committed fixture"]
    fn verify_pinned_apple_package() {
        let path = std::env::var_os("T1BRIDGE_RECOVERY_PACKAGE").expect("package path required");
        let firmware = Firmware::from_package(&std::fs::read(path).unwrap()).unwrap();
        assert!(!firmware.version.is_empty());
        // Print only schema and protocol constants, never Apple payload bytes.
        eprintln!(
            "identity keys: {:?}",
            plist::dictionary(&firmware.identity).unwrap().keys()
        );
        for (name, value) in
            plist::dictionary(plist::get(&firmware.identity, "Manifest").unwrap()).unwrap()
        {
            eprintln!(
                "component {name}: {:?}",
                plist::dictionary(value).unwrap().keys()
            );
        }
    }
}
