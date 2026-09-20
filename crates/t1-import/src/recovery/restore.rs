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
    // Retain the exact ticket/image pair before entering the transaction. An
    // incomplete attempt never becomes eligible to boot or install these files.
    persist("combined.memboot", &signed.combined)?;
    persist("apticket", &signed.ticket)?;
    let (mut mux, control, mut fdr_proxy) = start_restore(&device, &identity)?;
    let deadline = Instant::now() + Duration::from_secs(20 * 60);
    let mut committed = None;
    let mut requests = 0;
    loop {
        if Instant::now() > deadline {
            return Err(Error("EmbeddedOS restore exceeded its deadline"));
        }
        mux.poll()?;
        fdr_proxy.poll(&mut mux)?;
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
                check_final_status(&message, committed.as_deref())?;
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
                        fdr: committed.ok_or(INVALID)?,
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
    let usb = Usb::open(device)?;
    let mut mux = Mux::new(usb)?;
    until(Duration::from_secs(30), || {
        mux.poll()?;
        Ok(mux.ready())
    })?;
    let control = mux.connect(62078)?;
    until(Duration::from_secs(30), || {
        mux.poll()?;
        mux.connected(control)
    })?;
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
    let mut fdr_proxy = Fdr::new(&mut mux)?;
    until(Duration::from_secs(30), || {
        mux.poll()?;
        fdr_proxy.poll(&mut mux)?;
        Ok(fdr_proxy.ready())
    })?;
    mux.send_plist(
        control,
        &plist::dict([
            ("Request", plist::string("StartRestore")),
            ("RestoreProtocolVersion", Value::Integer(version.into())),
            ("RestoreOptions", options()),
        ]),
        false,
    )?;
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
    let transport = Usb::open(device)?;
    let identity = transport.identity(device)?;
    if expected_ecid.is_some_and(|ecid| ecid != identity.ecid) {
        return Err(Error("T1 identity changed between restore passes"));
    }
    println!("recovery: requesting Apple firmware signature");
    let mut signed = SignedFirmware::request(firmware, &identity)?;
    transport.upload(signed.component("iBEC")?)?;
    transport.command("go", true)?;
    drop(transport);
    let device = usb::wait(
        &device.location,
        usb::RECOVERY,
        Some(device.address),
        Duration::from_secs(90),
    )?;
    let transport = Usb::open(&device)?;
    let current = transport.identity(&device)?;
    if current.ecid != identity.ecid {
        return Err(Error("T1 identity changed after iBEC"));
    }
    if current.ap_nonce != identity.ap_nonce || current.sep_nonce != identity.sep_nonce {
        signed = SignedFirmware::request(firmware, &current)?;
    }
    transport.command("setenv auto-boot false", false)?;
    transport.command("saveenv", false)?;
    for (component, command) in [
        ("RestoreRamDisk", "ramdisk"),
        ("RestoreDeviceTree", "devicetree"),
        ("RestoreSEP", "rsepfirmware"),
    ] {
        transport.upload(signed.component(component)?)?;
        transport.command(command, false)?;
        if command == "ramdisk" {
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    transport.upload(signed.component("RestoreKernelCache")?)?;
    transport.command(&format!("setenv boot-args {BOOT_ARGS}"), false)?;
    transport.command("bootx", true)?;
    drop(transport);
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
    let transport = Usb::open(device)?;
    if transport.identity(device)?.ecid != ecid {
        return Err(Error("T1 identity changed before final boot"));
    }
    transport.command("setenv auto-boot false", false)?;
    transport.command("saveenv", false)?;
    transport.upload(&signed.ticket)?;
    transport.command("ticket", false)?;
    transport.upload(&signed.combined)?;
    transport.command("setenv boot-args rd=md0", false)?;
    transport.command("memboot", true)?;
    drop(transport);
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
    committed: &mut Option<Vec<u8>>,
    persist: &mut impl FnMut(&str, &[u8]) -> Result<()>,
) -> Result<Value> {
    let kind = plist::text(plist::get(message, "DataType")?)?;
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
        "FDRMemoryCommit" => accept_commit(message, replay, committed, persist),
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
        _ => Err(Error(
            "restore requested a data type outside the native T1 implementation",
        )),
    }
}

fn accept_commit(
    message: &Value,
    replay: Option<&[u8]>,
    committed: &mut Option<Vec<u8>>,
    persist: &mut impl FnMut(&str, &[u8]) -> Result<()>,
) -> Result<Value> {
    if committed.is_some() {
        return Err(Error("duplicate FDR commit in one restore attempt"));
    }
    let bytes = fdr_payload(message)?;
    if let Some(replay) = replay
        && plist::decode(&bytes)? != plist::decode(replay)?
    {
        return Err(Error("FDR replay differs from the provisioned store"));
    }
    persist("FDRData", &bytes)?; // Durable before the device receives its ACK.
    *committed = Some(bytes);
    Ok(plist::dict([]))
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
    fn commit_requires_durable_storage_and_matching_replay_before_ack() {
        let store = plist::dict([("SyntheticRecord", Value::Data(vec![1, 2, 3]))]);
        let message = plist::dict([("FDRData", store.clone())]);
        let replay = t1_bridge::bplist::encode(&store).unwrap();
        let different =
            t1_bridge::bplist::encode(&plist::dict([("SyntheticRecord", Value::Data(vec![9]))]))
                .unwrap();
        let mut committed = None;
        assert!(
            accept_commit(
                &message,
                Some(&different),
                &mut committed,
                &mut |_, _| panic!("mismatching replay must not reach storage")
            )
            .is_err()
        );
        assert!(committed.is_none());
        assert!(
            accept_commit(&message, Some(&replay), &mut committed, &mut |_, _| Err(
                Error("synthetic disk full")
            ))
            .is_err()
        );
        assert!(committed.is_none());
        let mut durable = false;
        let ack = accept_commit(&message, Some(&replay), &mut committed, &mut |_, bytes| {
            assert_eq!(plist::decode(bytes).unwrap(), store);
            durable = true;
            Ok(())
        })
        .unwrap();
        assert!(durable && committed.is_some());
        assert_eq!(ack, plist::dict([]));
        assert!(
            accept_commit(&message, None, &mut committed, &mut |_, _| panic!(
                "duplicate must not overwrite"
            ))
            .is_err()
        );
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
