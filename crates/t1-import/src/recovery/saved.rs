//! Explicit final-boot retry from protected, successful local restore artifacts.
//! Ticket parsing binds identity; it is not a replacement signature verifier.
use super::{Error, Result, confirm, plist, restore, storage, usb};
use std::{io::Read, os::unix::fs::MetadataExt, path::Path};
use t1_platform::recovery_fs::Directory;

const INVALID: Error = Error("saved restore phases are incomplete, inconsistent, or invalid");

/// Retries final boot without restoring or contacting Apple again.
///
/// Requires two protected successful phases, matching final factory data and
/// ticket-bound images for the same T1. Existing EFI and completed boot markers
/// block retries. Firmware verification remains the device's responsibility.
///
/// # Errors
/// Stops on any failed inspection, identity, boot, durability or import check.
pub fn resume_boot(efi: &Path) -> Result<()> {
    if !t1_platform::preserved_efi_discovery::is_root() {
        return Err(Error("saved recovery requires root authority"));
    }
    usb::supported_machine()?;
    storage::require_local_absence()?;
    let attempt = storage::Attempt::resume()?;
    let provision = attempt.existing_phase("provision")?;
    let personalize = attempt.existing_phase("personalize")?;
    if personalize.exists("boot-verified")? {
        return Err(Error(
            "final boot was already verified; inspect before retrying",
        ));
    }
    require_marker(&provision, b"restore-success\n")?;
    require_marker(&personalize, b"restore-success-and-fdr-match\n")?;
    let first = read(&provision, "FDRData", 16 * 1024 * 1024)?;
    let second = read(&personalize, "FDRData", 16 * 1024 * 1024)?;
    matching_fdr(&first, &second)?;
    println!("recovery: saved boot: validating retained tickets and image pairs");
    let first_ticket = read(&provision, "apticket", 1024 * 1024)?;
    let first_image = read(&provision, "combined.memboot", 64 * 1024 * 1024)?;
    let ticket = read(&personalize, "apticket", 1024 * 1024)?;
    let image = read(&personalize, "combined.memboot", 64 * 1024 * 1024)?;
    let ecid = ticket_ecid(&ticket)?;
    if ticket_ecid(&first_ticket)? != ecid {
        return Err(INVALID);
    }
    println!("recovery: saved boot: ticket identities match; checking provisioning image");
    paired_image(&first_image, &first_ticket)?;
    println!("recovery: saved boot: checking personalization image");
    paired_image(&image, &ticket)?;
    let version = read(&personalize, "version.plist", 1024 * 1024)?;
    if plist::dictionary(&plist::decode(&version)?)?.is_empty() {
        return Err(INVALID);
    }
    let destination = storage::EfiDestination::open(efi)?;
    let device = usb::discover()?.ok_or(Error("no T1 recovery device is present"))?;
    if device.product != usb::RECOVERY || device.has_hid {
        return Err(Error("saved boot requires a T1 already in recovery mode"));
    }
    println!(
        "recovery: saved successful phases, matching factory data and paired tickets verified"
    );
    confirm(true)?;
    restore::boot_saved(&device, ecid, &ticket, &image)?;
    storage::save(
        &personalize,
        "boot-verified",
        b"functional-usb-stable-30s\n",
    )?;
    destination.install(&first, &image, &version)?;
    attempt.finish()?;
    crate::runtime::attempt_protected_import()
        .map_err(|_| Error("EFI recovery completed; local machine-data import needs attention"))?;
    println!("recovery: saved final boot complete; verify status and later warm and cold boot");
    Ok(())
}

fn read(directory: &Directory, name: &str, limit: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let file = directory.file(name, false)?;
    if file.metadata()?.mode() & 0o777 != 0o600 {
        return Err(INVALID);
    }
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() as u64 > limit {
        return Err(INVALID);
    }
    Ok(bytes)
}

fn require_marker(directory: &Directory, expected: &[u8]) -> Result<()> {
    if read(directory, "complete", 64)? != expected {
        return Err(INVALID);
    }
    Ok(())
}

fn matching_fdr(first: &[u8], second: &[u8]) -> Result<()> {
    let first = plist::decode(first)?;
    let second = plist::decode(second)?;
    if plist::dictionary(&first)?.is_empty() || first != second {
        return Err(INVALID);
    }
    Ok(())
}

// Bounded definite-length DER, including Apple's private high-tag wrappers.
fn take<'a>(bytes: &mut &'a [u8]) -> Result<(u8, &'a [u8])> {
    let original = *bytes;
    let tag = *original.first().ok_or(INVALID)?;
    let mut offset = 1;
    if tag & 31 == 31 {
        let mut ended = false;
        for _ in 0..6 {
            let byte = *original.get(offset).ok_or(INVALID)?;
            offset += 1;
            if byte & 128 == 0 {
                ended = true;
                break;
            }
        }
        if !ended {
            return Err(INVALID);
        }
    }
    let byte = *original.get(offset).ok_or(INVALID)?;
    offset += 1;
    let length = if byte < 128 {
        usize::from(byte)
    } else {
        let count = usize::from(byte & 127);
        if count == 0 || count > 4 {
            return Err(INVALID);
        }
        let part = original.get(offset..offset + count).ok_or(INVALID)?;
        if part[0] == 0 {
            return Err(INVALID);
        }
        offset += count;
        let size = part.iter().fold(0_usize, |n, b| (n << 8) | usize::from(*b));
        if size < 128 {
            return Err(INVALID);
        }
        size
    };
    let end = offset.checked_add(length).ok_or(INVALID)?;
    let value = original.get(offset..end).ok_or(INVALID)?;
    *bytes = &original[end..];
    Ok((tag, value))
}

fn ticket_ecid(ticket: &[u8]) -> Result<u64> {
    let mut remaining = ticket;
    let (tag, body) = take(&mut remaining)?;
    if tag != 0x30 || !remaining.is_empty() {
        return Err(INVALID);
    }
    let mut values = Vec::new();
    manifest_values(body, &[], 0, &mut values)?;
    let [ecid] = values.as_slice() else {
        return Err(INVALID);
    };
    if *ecid == 0 {
        return Err(INVALID);
    }
    Ok(*ecid)
}

fn manifest_values(
    bytes: &[u8],
    path: &[&[u8]],
    depth: usize,
    values: &mut Vec<u64>,
) -> Result<()> {
    if depth > 24 {
        return Err(INVALID);
    }
    let mut remaining = bytes;
    let mut children = Vec::new();
    while !remaining.is_empty() {
        children.push(take(&mut remaining)?);
    }
    let mut path = path.to_vec();
    if let Some((0x16, name)) = children.first() {
        path.push(name);
    }
    if path == [b"IM4M".as_slice(), b"MANB", b"MANP", b"ECID"] {
        let [(0x16, _), (2, number)] = children.as_slice() else {
            return Err(INVALID);
        };
        if number.is_empty()
            || number.len() > 9
            || number[0] & 128 != 0
            || (number.len() == 9 && number[0] != 0)
        {
            return Err(INVALID);
        }
        values.push(number.iter().fold(0_u64, |n, b| (n << 8) | u64::from(*b)));
        return Ok(());
    }
    for (tag, child) in children {
        if tag & 32 != 0 {
            manifest_values(child, &path, depth + 1, values)?;
        }
    }
    Ok(())
}

fn paired_image(mut image: &[u8], ticket: &[u8]) -> Result<()> {
    for expected in [b"osrd", b"krnl", b"dtre", b"sepi"] {
        let (tag, mut body) = take(&mut image)?;
        if tag != 0x30 || take(&mut body)? != (0x16, b"IMG4".as_slice()) {
            return Err(INVALID);
        }
        let (tag, mut payload) = take(&mut body)?;
        if tag != 0x30
            || take(&mut payload)? != (0x16, b"IM4P".as_slice())
            || take(&mut payload)? != (0x16, expected.as_slice())
        {
            return Err(INVALID);
        }
        if take(&mut body)? != (0xa0, ticket) || !body.is_empty() {
            return Err(INVALID);
        }
    }
    if !image.is_empty() {
        return Err(INVALID);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn der(tag: u8, value: &[u8]) -> Vec<u8> {
        let mut result = vec![tag];
        if value.len() < 128 {
            result.push(u8::try_from(value.len()).unwrap());
        } else {
            result.extend([
                0x82,
                u8::try_from(value.len() >> 8).unwrap(),
                u8::try_from(value.len() & 255).unwrap(),
            ]);
            if result[2] == 0 {
                result.remove(2);
                result[1] = 0x81;
            }
        }
        result.extend(value);
        result
    }
    fn node(name: &[u8], child: &[u8]) -> Vec<u8> {
        der(0x30, &[der(0x16, name), child.to_vec()].concat())
    }
    fn ticket(number: &[u8]) -> Vec<u8> {
        node(
            b"IM4M",
            &node(b"MANB", &node(b"MANP", &node(b"ECID", &der(2, number)))),
        )
    }
    fn image(ticket: &[u8]) -> Vec<u8> {
        [b"osrd", b"krnl", b"dtre", b"sepi"]
            .iter()
            .flat_map(|name| {
                node(
                    b"IMG4",
                    &[
                        node(b"IM4P", &der(0x16, name.as_slice())),
                        der(0xa0, ticket),
                    ]
                    .concat(),
                )
            })
            .collect()
    }
    #[test]
    fn saved_identity_and_pair_are_bound_and_unambiguous() {
        let first = ticket(&[0x12, 0x34]);
        assert_eq!(ticket_ecid(&first).unwrap(), 0x1234);
        assert!(ticket_ecid(&ticket(&[0])).is_err());
        assert!(ticket_ecid(&ticket(&[0xff])).is_err());
        assert!(ticket_ecid(&[first.clone(), first.clone()].concat()).is_err());
        let duplicate = node(
            b"IM4M",
            &node(
                b"MANB",
                &node(
                    b"MANP",
                    &[node(b"ECID", &der(2, &[1])), node(b"ECID", &der(2, &[1]))].concat(),
                ),
            ),
        );
        assert!(ticket_ecid(&duplicate).is_err());
        let image = image(&first);
        paired_image(&image, &first).unwrap();
        assert!(paired_image(&image, &ticket(&[0x56])).is_err());
        assert!(paired_image(&image[..image.len() - 1], &first).is_err());
        assert!(paired_image(&[image, vec![0]].concat(), &first).is_err());
    }
    #[test]
    #[ignore = "requires Apple's pinned public package; no network or USB"]
    fn pinned_package_has_expected_combined_image_tags() {
        let path = std::env::var_os("T1BRIDGE_RECOVERY_PACKAGE").expect("package path required");
        let package = std::fs::read(path).unwrap();
        let firmware = super::super::firmware::Firmware::from_package(&package).unwrap();
        let signed = super::super::signing::SignedFirmware::synthetic(&firmware).unwrap();
        paired_image(&signed.combined, &signed.ticket).unwrap();
    }

    #[test]
    fn protected_reads_and_success_markers_are_required() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let path =
            std::env::temp_dir().join(format!("t1-saved-boot-synthetic-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        let directory = Directory::open(&path, false).unwrap();
        assert!(require_marker(&directory, b"restore-success\n").is_err());
        storage::save(&directory, "complete", b"restore-success\n").unwrap();
        require_marker(&directory, b"restore-success\n").unwrap();
        assert!(require_marker(&directory, b"restore-success-and-fdr-match\n").is_err());
        assert!(read(&directory, "complete", 2).is_err());
        fs::set_permissions(path.join("complete"), fs::Permissions::from_mode(0o644)).unwrap();
        assert!(require_marker(&directory, b"restore-success\n").is_err());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn factory_data_must_be_nonempty_and_match() {
        let first = plist::encode_xml(&plist::dict([("synthetic", plist::string("one"))])).unwrap();
        let second =
            plist::encode_xml(&plist::dict([("synthetic", plist::string("two"))])).unwrap();
        assert!(matching_fdr(&first, &first).is_ok());
        assert!(matching_fdr(&first, &second).is_err());
        assert!(matching_fdr(b"invalid", &first).is_err());
        let empty = plist::encode_xml(&plist::Value::Dictionary(
            std::collections::BTreeMap::default(),
        ))
        .unwrap();
        assert!(matching_fdr(&empty, &empty).is_err());
    }
}
