//! The `EmbeddedOS` restore exchange and its strict completion gates.

use super::{
    Error, Result,
    fdr_proxy::Fdr,
    firmware::Firmware,
    mux::{Mux, Transport},
    plist::{self, Value},
    signing::SignedFirmware,
    usb::{self, Device, Identity, Usb},
};
use std::time::{Duration, Instant};

const INVALID: Error = Error("invalid or unsupported EmbeddedOS restore message");
const BOOT_ARGS: &str = "rd=md0 -restore IOUSBDeviceController-configuration=standardMuxOnly";

pub(super) struct Completed {
    pub fdr: Vec<u8>,
    pub signed: SignedFirmware,
    pub device: Device,
}

pub(super) fn run(
    device: &Device,
    expected_ecid: Option<u64>,
    firmware: &Firmware,
    replay: Option<&[u8]>,
    mut persist: impl FnMut(&str, &[u8]) -> Result<()>,
) -> Result<(Completed, u64)> {
    let (device, identity, signed) = enter_restore(device, expected_ecid, firmware)?;
    println!("recovery: preserving signed restore artifacts");
    // Retain the exact ticket/image pair before entering the transaction. An
    // incomplete attempt never becomes eligible to boot or install these files.
    persist("combined.memboot", &signed.combined)?;
    persist("apticket", &signed.ticket)?;
    let (mut mux, control, mut fdr_proxy) = start_restore(&device, &identity)?;
    let deadline = Instant::now() + Duration::from_secs(20 * 60);
    let mut committed = FdrCommits::default();
    let mut requests = 0;
    loop {
        if Instant::now() > deadline {
            return Err(Error("EmbeddedOS restore exceeded its deadline"));
        }
        mux.poll()?;
        fdr_proxy.poll(&mut mux).inspect_err(|_| {
            eprintln!("recovery: failure while processing factory-data proxy traffic");
        })?;
        let Some(message) = mux.receive_plist(control, false)? else {
            if mux.closed(control)? {
                return Err(Error(
                    "restore disconnected without successful final status",
                ));
            }
            continue;
        };
        requests += 1;
        if requests > 10_000 {
            return Err(INVALID);
        }
        let kind = plist::text(plist::get(&message, "MsgType")?)?;
        match kind {
            "DataRequestMsg" => {
                let response = data_reply(
                    &message,
                    firmware,
                    &signed,
                    replay,
                    &mut committed,
                    &mut persist,
                )?;
                reply(&mut mux, &mut fdr_proxy, control, &message, &response)?;
            }
            "StatusMsg" => {
                committed.finish(&message, replay, &mut persist)?;
                mux.send_plist(
                    control,
                    &plist::dict([("MsgType", plist::string("ReceivedFinalStatusMsg"))]),
                    false,
                )?;
                until(Duration::from_secs(10), || {
                    mux.poll()?;
                    Ok(mux.pending(control)? == 0)
                })?;
                return Ok((
                    Completed {
                        fdr: committed.latest.take().ok_or(INVALID)?,
                        signed,
                        device,
                    },
                    identity.ecid,
                ));
            }
            "CheckpointMsg" => {
                let fields = plist::dictionary(&message)?;
                if fields.contains_key("CHECKPOINT_ERROR")
                    || fields
                        .get("CHECKPOINT_RESULT")
                        .is_some_and(|v| v != &Value::Integer(0))
                {
                    return Err(Error("EmbeddedOS restore checkpoint failed"));
                }
            }
            "ProgressMsg" | "PreviousRestoreLogMsg" => {} // Never expose device-supplied log contents.
            _ => return Err(INVALID),
        }
    }
}

fn start_restore(device: &Device, identity: &Identity) -> Result<(Mux<Usb>, u16, Fdr)> {
    println!("recovery: negotiating restore USB transport");
    let usb = Usb::open(device)?;
    let mut mux = Mux::new(usb)?;
    until(Duration::from_secs(30), || {
        mux.poll()?;
        Ok(mux.ready())
    })?;
    println!("recovery: connecting to the restore service");
    let control = mux.connect(62078)?;
    until(Duration::from_secs(30), || {
        mux.poll()?;
        mux.connected(control)
    })?;
    println!("recovery: querying restore service type");
    mux.send_plist(
        control,
        &plist::dict([("Request", plist::string("QueryType"))]),
        false,
    )?;
    let kind = receive(&mut mux, control, None)?;
    if plist::text(plist::get(&kind, "Type")?)? != "com.apple.mobile.restored" {
        return Err(INVALID);
    }
    let version = plist::integer(plist::get(&kind, "RestoreProtocolVersion")?)?;
    println!("recovery: verifying restore service hardware identity");
    mux.send_plist(
        control,
        &plist::dict([
            ("Request", plist::string("QueryValue")),
            ("QueryKey", plist::string("HardwareInfo")),
        ]),
        false,
    )?;
    let hardware = receive(&mut mux, control, None)?;
    let ecid = plist::integer(plist::get(
        plist::get(&hardware, "HardwareInfo")?,
        "UniqueChipID",
    )?)?;
    if ecid != identity.ecid {
        return Err(Error("restore service belongs to a different T1"));
    }
    println!("recovery: connecting the factory-data proxy");
    let mut fdr_proxy = Fdr::new(&mut mux)?;
    until(Duration::from_secs(30), || {
        mux.poll()?;
        fdr_proxy.poll(&mut mux)?;
        Ok(fdr_proxy.ready())
    })?;
    println!("recovery: starting the restore transaction");
    mux.send_plist(
        control,
        &plist::dict([
            ("Request", plist::string("StartRestore")),
            ("RestoreProtocolVersion", Value::Integer(version.into())),
            ("RestoreOptions", options()),
        ]),
        false,
    )?;
    println!("recovery: StartRestore request queued; awaiting restore messages");
    Ok((mux, control, fdr_proxy))
}

fn enter_restore(
    device: &Device,
    expected_ecid: Option<u64>,
    firmware: &Firmware,
) -> Result<(Device, Identity, SignedFirmware)> {
    if device.product != usb::RECOVERY {
        return Err(Error("T1 must already be in recovery mode"));
    }
    println!("recovery: opening initial recovery USB interface");
    let transport = Usb::open(device)?;
    println!("recovery: reading initial recovery identity");
    let identity = transport.identity(device)?;
    if expected_ecid.is_some_and(|ecid| ecid != identity.ecid) {
        return Err(Error("T1 identity changed between restore passes"));
    }
    println!("recovery: requesting Apple firmware signature");
    let mut signed = SignedFirmware::request(firmware, &identity)?;
    println!("recovery: uploading signed iBEC");
    transport.upload(signed.component("iBEC")?)?;
    println!("recovery: sending iBEC go command");
    transport.command("go", true)?;
    drop(transport);
    println!("recovery: waiting for iBEC USB re-enumeration");
    let device = usb::wait(
        &device.location,
        usb::RECOVERY,
        Some(device.address),
        Duration::from_secs(90),
    )?;
    println!("recovery: opening iBEC USB interface");
    let transport = Usb::open(&device)?;
    println!("recovery: reading iBEC identity");
    let current = transport.identity(&device)?;
    if current.ecid != identity.ecid {
        return Err(Error("T1 identity changed after iBEC"));
    }
    if current.ap_nonce != identity.ap_nonce || current.sep_nonce != identity.sep_nonce {
        println!("recovery: requesting Apple firmware signature for changed nonces");
        signed = SignedFirmware::request(firmware, &current)?;
    }
    println!("recovery: setting auto-boot policy");
    transport.command("setenv auto-boot false", false)?;
    println!("recovery: saving auto-boot policy");
    transport.command("saveenv", false)?;
    for (component, command) in [
        ("RestoreRamDisk", "ramdisk"),
        ("RestoreDeviceTree", "devicetree"),
        ("RestoreSEP", "rsepfirmware"),
    ] {
        println!("recovery: uploading {component}");
        transport.upload(signed.component(component)?)?;
        println!("recovery: sending {command} command");
        transport.command(command, false)?;
        if command == "ramdisk" {
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    println!("recovery: uploading RestoreKernelCache");
    transport.upload(signed.component("RestoreKernelCache")?)?;
    println!("recovery: setting restore boot arguments");
    transport.command(&format!("setenv boot-args {BOOT_ARGS}"), false)?;
    println!("recovery: sending restore bootx command");
    transport.command("bootx", true)?;
    drop(transport);
    println!("recovery: waiting for restore USB re-enumeration");
    let device = usb::wait(
        &device.location,
        usb::BOOTED,
        None,
        Duration::from_secs(180),
    )?;
    if device.has_hid {
        return Err(Error(
            "T1 booted into its functional personality instead of restored",
        ));
    }
    Ok((device, current, signed))
}

pub(super) fn boot(device: &Device, ecid: u64, signed: &SignedFirmware) -> Result<()> {
    boot_saved(device, ecid, &signed.ticket, &signed.combined)
}

pub(super) fn boot_saved(device: &Device, ecid: u64, ticket: &[u8], combined: &[u8]) -> Result<()> {
    println!("recovery: final boot: opening USB interface");
    let transport = Usb::open(device)?;
    println!("recovery: final boot: reading identity and nonces");
    if transport.identity(device)?.ecid != ecid {
        return Err(Error("T1 identity changed before final boot"));
    }
    println!("recovery: final boot: setting auto-boot policy");
    transport.command("setenv auto-boot false", false)?;
    println!("recovery: final boot: saving auto-boot policy");
    transport.command("saveenv", false)?;
    println!("recovery: final boot: uploading ticket");
    transport.upload(ticket)?;
    println!("recovery: final boot: sending ticket command");
    transport.command("ticket", false)?;
    println!("recovery: final boot: uploading combined image");
    transport.upload(combined)?;
    println!("recovery: final boot: setting boot arguments");
    transport.command("setenv boot-args rd=md0", false)?;
    println!("recovery: final boot: sending memboot command");
    transport.command("memboot", true)?;
    drop(transport);
    println!("recovery: final boot: waiting for functional USB");
    let booted = usb::wait(
        &device.location,
        usb::BOOTED,
        None,
        Duration::from_secs(120),
    )?;
    if !booted.has_hid {
        return Err(Error(
            "final memboot produced a restore-only USB personality",
        ));
    }
    println!("recovery: final boot: checking functional USB stability");
    usb::prove_boot(&device.location)
}

fn receive<T: Transport>(mux: &mut Mux<T>, port: u16, mut fdr: Option<&mut Fdr>) -> Result<Value> {
    let mut output = None;
    until(Duration::from_secs(30), || {
        mux.poll()?;
        if let Some(fdr) = fdr.as_mut() {
            fdr.poll(mux)?;
        }
        output = mux.receive_plist(port, false)?;
        if output.is_none() && mux.closed(port)? {
            return Err(Error("restore service closed before replying"));
        }
        Ok(output.is_some())
    })?;
    output.ok_or(INVALID)
}

fn reply<T: Transport>(
    mux: &mut Mux<T>,
    fdr: &mut Fdr,
    control: u16,
    request: &Value,
    response: &Value,
) -> Result<()> {
    let port = if let Some(port) = plist::dictionary(request)?.get("DataPort") {
        let destination = u16::try_from(plist::integer(port)?).map_err(|_| INVALID)?;
        if destination < 1024 {
            return Err(INVALID);
        }
        let port = mux.connect(destination)?;
        until(Duration::from_secs(30), || {
            mux.poll()?;
            fdr.poll(mux)?;
            mux.connected(port)
        })?;
        port
    } else {
        control
    };
    mux.send_plist(port, response, false)?;
    until(Duration::from_secs(120), || {
        mux.poll()?;
        fdr.poll(mux)?;
        Ok(mux.pending(port)? == 0)
    })?;
    if port != control {
        mux.close(port)?;
    }
    Ok(())
}

fn data_reply(
    message: &Value,
    firmware: &Firmware,
    signed: &SignedFirmware,
    replay: Option<&[u8]>,
    committed: &mut FdrCommits,
    persist: &mut impl FnMut(&str, &[u8]) -> Result<()>,
) -> Result<Value> {
    let kind = plist::text(plist::get(message, "DataType")?)?;
    if matches!(
        kind,
        "RootTicket"
            | "FDRTrustData"
            | "FDRMemoryCommit"
            | "FUDData"
            | "KernelCache"
            | "DeviceTree"
            | "NORData"
            | "BuildIdentityDict"
    ) {
        println!("recovery: preparing {kind} reply");
    }
    match kind {
        "RootTicket" => {
            let mut values = std::collections::BTreeMap::new();
            values.insert("RootTicketData".into(), Value::Data(signed.ticket.clone()));
            if let Some(replay) = replay {
                values.insert("FDRMemoryStoreData".into(), plist::decode(replay)?);
            }
            Ok(Value::Dictionary(values))
        }
        "FDRTrustData" => Ok(plist::dict([])),
        "FDRMemoryCommit" => committed.accept(message, persist),
        "FUDData" => fud_data(message, &firmware.identity, |name| signed.component(name)),
        "KernelCache" | "DeviceTree" => Ok(component_reply(kind, signed.component(kind)?)),
        "NORData" => nor_data(message, firmware, signed),
        "BuildIdentityDict" => {
            let variant = plist::dictionary(message)?
                .get("Arguments")
                .and_then(|a| plist::dictionary(a).ok())
                .and_then(|a| a.get("Variant"))
                .cloned()
                .unwrap_or_else(|| plist::string("Erase"));
            if plist::text(&variant)?.len() > 128 {
                return Err(INVALID);
            }
            Ok(plist::dict([
                ("BuildIdentityDict", firmware.identity.clone()),
                ("Variant", variant),
            ]))
        }
        _ => {
            if let Some(name) = unsupported_data_name(kind) {
                eprintln!("recovery: unsupported public restore data type: {name}");
            }
            Err(Error(
                "restore requested a data type outside the native T1 implementation",
            ))
        }
    }
}

// FUDData uses only manifest entries explicitly tagged IsFUDFirmware. An empty
// match is a valid empty list/dictionary, not permission to send other images.
// Wire shape follows idevicerestore's restore_send_image_data FUD exchange.
fn fud_data<'a>(
    message: &Value,
    identity: &Value,
    mut component: impl FnMut(&str) -> Result<&'a [u8]>,
) -> Result<Value> {
    let arguments = plist::dictionary(message)?
        .get("Arguments")
        .map(plist::dictionary)
        .transpose()?;
    let argument = |key| arguments.and_then(|fields| fields.get(key));
    let want_list = match argument("FUDImageList") {
        None | Some(Value::Boolean(false)) => false,
        Some(Value::Boolean(true)) => true,
        _ => return Err(INVALID),
    };
    let image_name = argument("ImageName").map(plist::text).transpose()?;
    let mut names = Vec::new();
    for (name, entry) in plist::dictionary(plist::get(identity, "Manifest")?)? {
        let info = plist::dictionary(plist::get(entry, "Info")?)?;
        match info.get("IsFUDFirmware") {
            Some(Value::Boolean(true)) => names.push(name.as_str()),
            None | Some(Value::Boolean(false)) => {}
            _ => return Err(INVALID),
        }
    }
    if want_list {
        return Ok(plist::dict([(
            "FUDImageList",
            Value::Array(names.into_iter().map(plist::string).collect()),
        )]));
    }
    if let Some(name) = image_name {
        if !names.contains(&name) {
            return Err(Error(
                "requested FUD image is absent from the verified manifest",
            ));
        }
        return Ok(plist::dict([
            ("ImageName", plist::string(name)),
            ("FUDImageData", Value::Data(component(name)?.to_vec())),
        ]));
    }
    let images = names
        .into_iter()
        .map(|name| Ok((name.into(), Value::Data(component(name)?.to_vec()))))
        .collect::<Result<_>>()?;
    Ok(plist::dict([("FUDImageData", Value::Dictionary(images))]))
}

// Only fixed public protocol names can enter diagnostics. Never print arbitrary
// device-supplied strings, arguments, restore logs, or factory-data contents.
fn unsupported_data_name(kind: &str) -> Option<&'static str> {
    [
        "SystemImageData",
        "RecoveryOSRootTicketData",
        "SystemImageRootHash",
        "SystemImageCanonicalMetadata",
        "BasebandData",
        "BasebandBootData",
        "BasebandStackData",
        "FirmwareUpdaterPreflight",
        "FirmwareUpdaterData",
        "DeviceRestoreInfoPreflight",
        "PersonalizedData",
        "EANData",
        "BootabilityBundle",
        "BootabilityBundleV2",
        "ReceiptManifest",
        "BasebandUpdaterOutputData",
        "URLAsset",
        "StreamedImageDecryptionKey",
        "RecoveryOSFileAssetImage",
        "RecoveryOSIBootFWFilesImages",
        "RecoveryOSImage",
        "RecoveryOSVersionData",
        "FDRMemoryStoreData",
    ]
    .into_iter()
    .find(|name| *name == kind)
}

// Restored can flush successive memory-store snapshots during one transaction.
// Keep a bounded immutable journal; only successful final status selects FDRData.
#[derive(Default)]
struct FdrCommits {
    latest: Option<Vec<u8>>,
    snapshots: usize,
    bytes: usize,
}

impl FdrCommits {
    const MAX_SNAPSHOTS: usize = 16;
    const MAX_BYTES: usize = 32 * 1024 * 1024;

    fn accept(
        &mut self,
        message: &Value,
        persist: &mut impl FnMut(&str, &[u8]) -> Result<()>,
    ) -> Result<Value> {
        let bytes = fdr_payload(message)?;
        if self.snapshots >= Self::MAX_SNAPSHOTS || bytes.len() > Self::MAX_BYTES - self.bytes {
            return Err(Error("FDR snapshot journal exceeded its limit"));
        }
        let name = format!("FDRData-commit-{:04}", self.snapshots + 1);
        // The storage callback must sync and readback-verify before any ACK.
        persist(&name, &bytes)?;
        self.snapshots += 1;
        self.bytes += bytes.len();
        self.latest = Some(bytes);
        println!("recovery: factory-data snapshot durably preserved; preparing acknowledgement");
        Ok(plist::dict([]))
    }

    fn finish(
        &self,
        status: &Value,
        replay: Option<&[u8]>,
        persist: &mut impl FnMut(&str, &[u8]) -> Result<()>,
    ) -> Result<()> {
        check_final_status(status, self.latest.as_deref())?;
        let bytes = self.latest.as_deref().ok_or(INVALID)?;
        if let Some(replay) = replay
            && plist::decode(bytes)? != plist::decode(replay)?
        {
            return Err(Error("final FDR replay differs from the provisioned store"));
        }
        // Never select an intermediate snapshot merely because it matched the
        // replay. The last committed store must match at successful final status.
        persist("FDRData", bytes)?;
        println!("recovery: successful final status and final factory-data store validated");
        Ok(())
    }
}

fn component_reply(kind: &str, bytes: &[u8]) -> Value {
    plist::dict([(&format!("{kind}File"), Value::Data(bytes.to_vec()))])
}

fn nor_data(message: &Value, firmware: &Firmware, signed: &SignedFirmware) -> Result<Value> {
    let version_one = plist::dictionary(message)?
        .get("Arguments")
        .and_then(|a| plist::dictionary(a).ok())
        .is_some_and(|a| a.contains_key("FlashVersion1"));
    let images = firmware.nor_components()?;
    let nor = if version_one {
        Value::Dictionary(
            images
                .into_iter()
                .map(|name| Ok((name.into(), Value::Data(signed.component(name)?.to_vec()))))
                .collect::<Result<_>>()?,
        )
    } else {
        Value::Array(
            images
                .into_iter()
                .map(|name| Ok(Value::Data(signed.component(name)?.to_vec())))
                .collect::<Result<_>>()?,
        )
    };
    Ok(plist::dict([
        (
            "LlbImageData",
            Value::Data(signed.component("LLB")?.to_vec()),
        ),
        ("NorImageData", nor),
        (
            "RestoreSEPImageData",
            Value::Data(signed.component("RestoreSEP")?.to_vec()),
        ),
        (
            "SEPImageData",
            Value::Data(signed.component("SEP")?.to_vec()),
        ),
    ]))
}

fn fdr_payload(message: &Value) -> Result<Vec<u8>> {
    const KEYS: [&str; 5] = [
        "FDRMemoryStoreData",
        "FDRMemoryCommitData",
        "FDRData",
        "MemoryStoreData",
        "Data",
    ];
    let mut candidates = Vec::new();
    let top = plist::dictionary(message)?;
    for key in KEYS {
        if let Some(value) = top.get(key) {
            candidates.push(value);
        }
    }
    if let Some(arguments) = top.get("Arguments") {
        let arguments = plist::dictionary(arguments)?;
        for key in KEYS {
            if let Some(value) = arguments.get(key) {
                candidates.push(value);
            }
        }
        if candidates.is_empty() && arguments.len() == 1 {
            candidates.extend(arguments.values());
        }
    }
    let [value] = candidates.as_slice() else {
        return Err(Error("missing or ambiguous FDR commit payload"));
    };
    let (value, bytes) = match value {
        Value::Data(bytes) => (plist::decode(bytes)?, bytes.clone()),
        Value::Dictionary(_) => (
            (*value).clone(),
            t1_bridge::bplist::encode(value).map_err(|_| INVALID)?,
        ),
        _ => return Err(INVALID),
    };
    if plist::dictionary(&value)?.is_empty() {
        return Err(Error("FDR commit contains an empty store"));
    }
    Ok(bytes)
}

fn check_final_status(message: &Value, fdr: Option<&[u8]>) -> Result<()> {
    let fields = plist::dictionary(message)?;
    if plist::integer(plist::get(message, "Status")?)? != 0
        || fields
            .get("AMRError")
            .is_some_and(|v| v != &Value::Integer(0))
    {
        return Err(Error("EmbeddedOS restore reported failure"));
    }
    if fdr.is_none_or(<[u8]>::is_empty) {
        return Err(Error("restore completed without committing FDR data"));
    }
    Ok(())
}

fn options() -> Value {
    let mut fields = std::collections::BTreeMap::new();
    for name in [
        "ApBootstrapOnly",
        "ShouldRestoreSystemImage",
        "CreateFilesystemPartitions",
        "SystemImage",
        "RootToInstall",
        "UpdateBaseband",
        "DataImage",
        "InstallDiags",
    ] {
        fields.insert(name.into(), Value::Boolean(false));
    }
    for name in ["PersonalizedDuringPreflight", "FlashNOR"] {
        fields.insert(name.into(), Value::Boolean(true));
    }
    for (name, value) in [
        ("FDRMemoryStorePath", "/tmp/FDRMemoryStore"),
        ("RestoreBootArgs", BOOT_ARGS),
        ("BootImageType", "User"),
        ("DFUFileType", "RELEASE"),
        ("FirmwareDirectory", "."),
        ("KernelCacheType", "Release"),
        ("NORImageType", "production"),
        ("RestoreBundlePath", "/tmp/Per2.tmp"),
        ("SystemImageType", "User"),
    ] {
        fields.insert(name.into(), plist::string(value));
    }
    fields.insert("AutoBootDelay".into(), Value::Integer(0));
    fields.insert(
        "SupportedDataTypes".into(),
        Value::Dictionary(
            [
                "RootTicket",
                "FDRTrustData",
                "FDRMemoryCommit",
                "FUDData",
                "KernelCache",
                "DeviceTree",
                "NORData",
                "BuildIdentityDict",
            ]
            .into_iter()
            .map(|name| (name.into(), Value::Boolean(true)))
            .collect(),
        ),
    );
    fields.insert(
        "SupportedMessageTypes".into(),
        Value::Dictionary(
            [
                "DataRequestMsg",
                "ProgressMsg",
                "StatusMsg",
                "CheckpointMsg",
                "PreviousRestoreLogMsg",
            ]
            .into_iter()
            .map(|name| (name.into(), Value::Boolean(true)))
            .collect(),
        ),
    );
    Value::Dictionary(fields)
}

pub(super) fn until(timeout: Duration, mut step: impl FnMut() -> Result<bool>) -> Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if step()? {
            return Ok(());
        }
    }
    Err(Error("recovery protocol operation timed out"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fud_responses_filter_manifest_and_distinguish_list_named_and_bulk() {
        let identity = plist::dict([(
            "Manifest",
            plist::dict([
                (
                    "SyntheticFud",
                    plist::dict([(
                        "Info",
                        plist::dict([("IsFUDFirmware", Value::Boolean(true))]),
                    )]),
                ),
                (
                    "SyntheticOther",
                    plist::dict([(
                        "Info",
                        plist::dict([("IsFUDFirmware", Value::Boolean(false))]),
                    )]),
                ),
            ]),
        )]);
        let request = |args| plist::dict([("Arguments", args)]);
        let list = request(plist::dict([("FUDImageList", Value::Boolean(true))]));
        assert_eq!(
            fud_data(&list, &identity, |_| panic!("list must not load data")).unwrap(),
            plist::dict([(
                "FUDImageList",
                Value::Array(vec![plist::string("SyntheticFud")])
            )])
        );
        let named = request(plist::dict([("ImageName", plist::string("SyntheticFud"))]));
        let bytes = b"synthetic signed image";
        assert_eq!(
            fud_data(&named, &identity, |name| {
                assert_eq!(name, "SyntheticFud");
                Ok(bytes.as_slice())
            })
            .unwrap(),
            plist::dict([
                ("ImageName", plist::string("SyntheticFud")),
                ("FUDImageData", Value::Data(bytes.to_vec())),
            ])
        );
        assert_eq!(
            fud_data(&plist::dict([]), &identity, |name| {
                assert_eq!(name, "SyntheticFud");
                Ok(bytes.as_slice())
            })
            .unwrap(),
            plist::dict([(
                "FUDImageData",
                plist::dict([("SyntheticFud", Value::Data(bytes.to_vec())),])
            )])
        );
        for name in ["SyntheticOther", "Missing", "../SyntheticFud"] {
            let bad = request(plist::dict([("ImageName", plist::string(name))]));
            assert!(
                fud_data(&bad, &identity, |_| panic!("unmatched image must not load")).is_err()
            );
        }
        assert!(
            fud_data(&named, &identity, |_| Err(Error(
                "synthetic missing signature"
            )))
            .is_err()
        );
        let bad = request(plist::dict([("FUDImageList", Value::Integer(1))]));
        assert!(fud_data(&bad, &identity, |_| panic!("invalid list flag")).is_err());
    }

    #[test]
    fn no_fud_entries_yield_empty_protocol_containers() {
        let identity = plist::dict([(
            "Manifest",
            plist::dict([("SyntheticOther", plist::dict([("Info", plist::dict([]))]))]),
        )]);
        assert_eq!(
            fud_data(&plist::dict([]), &identity, |_| panic!("no FUD images")).unwrap(),
            plist::dict([("FUDImageData", plist::dict([]))])
        );
        let list = plist::dict([(
            "Arguments",
            plist::dict([("FUDImageList", Value::Boolean(true))]),
        )]);
        assert_eq!(
            fud_data(&list, &identity, |_| panic!("no FUD images")).unwrap(),
            plist::dict([("FUDImageList", Value::Array(vec![]))])
        );
    }

    #[test]
    fn unsupported_data_diagnostic_never_echoes_unrecognized_device_text() {
        for name in ["EANData", "FirmwareUpdaterData", "SystemImageData"] {
            assert_eq!(unsupported_data_name(name), Some(name));
        }
        for text in [
            "SyntheticPrivateIdentifier",
            "FUDData\nSyntheticPrivateIdentifier",
            "FUDData\0",
            "fuddata",
            "",
        ] {
            assert_eq!(unsupported_data_name(text), None);
        }
    }

    fn synthetic_store(byte: u8) -> Value {
        plist::dict([("SyntheticRecord", Value::Data(vec![byte; 3]))])
    }

    #[test]
    fn commits_are_durable_immutable_snapshots_before_ack() {
        let mut state = FdrCommits::default();
        let first = plist::dict([("FDRData", synthetic_store(1))]);
        assert!(
            state
                .accept(&first, &mut |_, _| Err(Error("synthetic disk full")))
                .is_err()
        );
        assert!(state.latest.is_none());
        assert_eq!(state.snapshots, 0);
        let mut journal = std::collections::BTreeMap::new();
        for byte in [1, 2, 2] {
            let message = plist::dict([("FDRData", synthetic_store(byte))]);
            let ack = state
                .accept(&message, &mut |name, bytes| {
                    assert!(journal.insert(name.to_owned(), bytes.to_vec()).is_none());
                    Ok(())
                })
                .unwrap();
            assert_eq!(ack, plist::dict([]));
        }
        assert_eq!(journal.len(), 3);
        assert_eq!(
            plist::decode(&journal["FDRData-commit-0001"]).unwrap(),
            synthetic_store(1)
        );
        assert_eq!(
            plist::decode(&journal["FDRData-commit-0002"]).unwrap(),
            synthetic_store(2)
        );
        assert_eq!(
            plist::decode(state.latest.as_ref().unwrap()).unwrap(),
            synthetic_store(2)
        );
        assert!(!journal.contains_key("FDRData"));
        assert!(
            state
                .accept(&plist::dict([]), &mut |_, _| panic!("malformed payload"))
                .is_err()
        );
        assert!(
            state
                .accept(&first, &mut |_, _| Err(Error("synthetic disk full")))
                .is_err()
        );
        assert_eq!(state.snapshots, 3);
        assert_eq!(
            plist::decode(state.latest.as_ref().unwrap()).unwrap(),
            synthetic_store(2)
        );
    }

    #[test]
    fn only_successful_final_matching_snapshot_is_selected() {
        let replay = t1_bridge::bplist::encode(&synthetic_store(2)).unwrap();
        let success = plist::dict([("Status", Value::Integer(0))]);
        let failure = plist::dict([("Status", Value::Integer(14))]);
        let mut state = FdrCommits::default();
        assert!(
            state
                .finish(&success, None, &mut |_, _| panic!("missing commit"))
                .is_err()
        );
        state
            .accept(
                &plist::dict([("FDRData", synthetic_store(1))]),
                &mut |_, _| Ok(()),
            )
            .unwrap();
        assert!(
            state
                .finish(&failure, None, &mut |_, _| panic!("failed restore"))
                .is_err()
        );
        assert!(
            state
                .finish(&success, Some(&replay), &mut |_, _| panic!(
                    "final replay mismatch"
                ))
                .is_err()
        );
        // Intermediate differences are retained; the final snapshot must match.
        let xml = plist::encode_xml(&synthetic_store(2)).unwrap();
        state
            .accept(
                &plist::dict([("FDRData", Value::Data(xml))]),
                &mut |_, _| Ok(()),
            )
            .unwrap();
        assert!(
            state
                .finish(&success, Some(&replay), &mut |_, _| Err(Error(
                    "synthetic disk full"
                )))
                .is_err()
        );
        let mut selected = false;
        state
            .finish(&success, Some(&replay), &mut |name, bytes| {
                assert_eq!(name, "FDRData");
                assert_eq!(plist::decode(bytes).unwrap(), synthetic_store(2));
                selected = true;
                Ok(())
            })
            .unwrap();
        assert!(selected);
        // An earlier match cannot excuse a mismatching final snapshot.
        state
            .accept(
                &plist::dict([("FDRData", synthetic_store(3))]),
                &mut |_, _| Ok(()),
            )
            .unwrap();
        assert!(
            state
                .finish(&success, Some(&replay), &mut |_, _| panic!(
                    "latest must match"
                ))
                .is_err()
        );
    }

    #[test]
    fn snapshot_count_and_total_size_are_bounded_before_persistence() {
        let message = plist::dict([("FDRData", synthetic_store(1))]);
        let mut state = FdrCommits::default();
        for _ in 0..FdrCommits::MAX_SNAPSHOTS {
            state.accept(&message, &mut |_, _| Ok(())).unwrap();
        }
        assert!(
            state
                .accept(&message, &mut |_, _| panic!(
                    "count limit must prevent write"
                ))
                .is_err()
        );
        let mut state = FdrCommits {
            bytes: FdrCommits::MAX_BYTES,
            ..FdrCommits::default()
        };
        assert!(
            state
                .accept(&message, &mut |_, _| panic!(
                    "byte limit must prevent write"
                ))
                .is_err()
        );
        assert!(state.latest.is_none());
    }

    #[test]
    #[ignore = "requires pinned public Apple package outside the repository; no network or USB"]
    fn pinned_firmware_replies_fit_restore_xml_transport() {
        let path = std::env::var_os("T1BRIDGE_RECOVERY_PACKAGE").expect("package path required");
        let firmware = Firmware::from_package(&std::fs::read(path).unwrap()).unwrap();
        let signed = SignedFirmware::synthetic(&firmware).unwrap();
        assert_eq!(
            fud_data(&plist::dict([]), &firmware.identity, |_| panic!(
                "pinned T1 identity has no FUD images"
            ))
            .unwrap(),
            plist::dict([("FUDImageData", plist::dict([]))])
        );
        let mut exceeded_control_limit = false;
        for kind in [
            "RootTicket",
            "KernelCache",
            "DeviceTree",
            "NORData",
            "BuildIdentityDict",
            "FUDData",
        ] {
            for version_one in [false, true] {
                let mut request = plist::dict([("DataType", plist::string(kind))]);
                if version_one {
                    let Value::Dictionary(fields) = &mut request else {
                        unreachable!()
                    };
                    fields.insert(
                        "Arguments".into(),
                        plist::dict([("FlashVersion1", Value::Boolean(true))]),
                    );
                }
                let reply = data_reply(
                    &request,
                    &firmware,
                    &signed,
                    None,
                    &mut FdrCommits::default(),
                    &mut |_, _| panic!("firmware reply must not persist FDR"),
                )
                .unwrap();
                let bytes = plist::encode_restore_xml(&reply).unwrap();
                let old_limit_rejects = plist::encode_xml(&reply).is_err();
                exceeded_control_limit |= old_limit_rejects;
                eprintln!(
                    "{kind}: XML bytes={} FlashVersion1={version_one} exceeds_control_limit={old_limit_rejects}",
                    bytes.len()
                );
            }
        }
        assert!(exceeded_control_limit);
    }

    #[test]
    fn component_response_uses_restored_file_key() {
        let value = component_reply("KernelCache", b"synthetic kernel");
        assert_eq!(
            plist::data(plist::get(&value, "KernelCacheFile").unwrap()).unwrap(),
            b"synthetic kernel"
        );
        assert!(plist::get(&value, "KernelCache").is_err());
    }

    #[test]
    fn artifacts_do_not_override_restore_failure() {
        assert!(
            check_final_status(
                &plist::dict([("Status", Value::Integer(14))]),
                Some(b"synthetic")
            )
            .is_err()
        );
        assert!(check_final_status(&plist::dict([("Status", Value::Integer(0))]), None).is_err());
        assert!(
            check_final_status(
                &plist::dict([
                    ("Status", Value::Integer(0)),
                    ("AMRError", Value::Integer(-1))
                ]),
                Some(b"synthetic")
            )
            .is_err()
        );
        assert!(
            check_final_status(
                &plist::dict([("Status", Value::Integer(0))]),
                Some(b"synthetic")
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_whole_message_fallback_empty_and_ambiguous_fdr() {
        assert!(fdr_payload(&plist::dict([("MsgType", plist::string("DataRequestMsg"))])).is_err());
        let store = plist::dict([("SyntheticRecord", Value::Data(vec![1, 2, 3]))]);
        let message = plist::dict([("Arguments", plist::dict([("FDRData", store.clone())]))]);
        assert_eq!(
            plist::decode(&fdr_payload(&message).unwrap()).unwrap(),
            store
        );
        assert!(fdr_payload(&plist::dict([("FDRData", store.clone()), ("Data", store)])).is_err());
        assert!(fdr_payload(&plist::dict([("FDRData", plist::dict([]))])).is_err());
    }

    #[test]
    fn firmware_only_options_do_not_request_host_os_or_repartitioning() {
        let options = options();
        for key in [
            "SystemImage",
            "ShouldRestoreSystemImage",
            "CreateFilesystemPartitions",
            "UpdateBaseband",
        ] {
            assert_eq!(plist::get(&options, key).unwrap(), &Value::Boolean(false));
        }
        assert_eq!(
            plist::get(
                plist::get(&options, "SupportedDataTypes").unwrap(),
                "FDRMemoryCommit"
            )
            .unwrap(),
            &Value::Boolean(true)
        );
        assert!(plist::get(&options, "BootImageTagOverride").is_err());
    }
}
