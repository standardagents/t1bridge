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

#[derive(Clone, Default, Eq, PartialEq)]
struct FdrCommits {
    latest: Option<Vec<u8>>,
    count: usize,
    stored_bytes: usize,
}

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
    exchange(device, &identity, signed, firmware, replay, &mut persist)
}

fn exchange(
    device: Device,
    identity: &Identity,
    signed: SignedFirmware,
    firmware: &Firmware,
    replay: Option<&[u8]>,
    mut persist: impl FnMut(&str, &[u8]) -> Result<()>,
) -> Result<(Completed, u64)> {
    if let Some(replay) = replay {
        plist::decode(replay)?;
        persist("FDRData.replay-input", replay)?;
    }
    let (mut mux, control, mut fdr_proxy) = start_restore(&device, identity)?;
    let deadline = Instant::now() + Duration::from_secs(20 * 60);
    let mut committed = FdrCommits::default();
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
                println!("recovery: handling restore data request");
                let response = data_reply(
                    &message,
                    firmware,
                    &signed,
                    replay,
                    &mut committed,
                    &mut persist,
                )
                .inspect_err(|_| eprintln!("recovery: constructing restore data reply failed"))?;
                reply(&mut mux, &mut fdr_proxy, control, &message, &response)
                    .inspect_err(|_| eprintln!("recovery: delivering restore data reply failed"))?;
            }
            "StatusMsg" => {
                println!("recovery: checking final restore status");
                check_final_status(&message, committed.latest.as_deref())?;
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
                        fdr: committed.latest.ok_or(INVALID)?,
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
    let mut mux = Mux::open(usb)?;
    until(Duration::from_secs(30), || {
        mux.poll()?;
        Ok(mux.ready())
    })?;
    println!("recovery: connecting restore control service");
    let mut control = mux.connect(62078)?;
    until(Duration::from_secs(30), || {
        mux.poll()?;
        if mux.closed(control)? {
            mux.close(control)?;
            std::thread::sleep(Duration::from_millis(250));
            control = mux.connect(62078)?;
            return Ok(false);
        }
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
    println!("recovery: checking restore hardware identity");
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
    println!("recovery: negotiating factory-data service");
    until(Duration::from_secs(30), || {
        mux.poll()?;
        fdr_proxy.poll(&mut mux)?;
        Ok(fdr_proxy.ready())
    })?;
    println!("recovery: starting factory restore transaction");
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
    println!("recovery: uploading iBEC");
    transport.upload(signed.component("iBEC")?)?;
    println!("recovery: starting iBEC");
    transport.command("go", true)?;
    drop(transport);
    println!("recovery: waiting for iBEC USB handoff");
    let device = usb::wait(
        &device.location,
        usb::RECOVERY,
        Some(device.address),
        Duration::from_secs(90),
    )?;
    println!("recovery: opening iBEC and checking its identity");
    let transport = Usb::open(&device)?;
    let current = transport.identity(&device)?;
    if current.ecid != identity.ecid {
        return Err(Error("T1 identity changed after iBEC"));
    }
    if current.ap_nonce != identity.ap_nonce || current.sep_nonce != identity.sep_nonce {
        println!("recovery: requesting a new signature after iBEC nonce change");
        signed = SignedFirmware::request(firmware, &current)?;
    }
    println!("recovery: configuring restore boot");
    transport.command("setenv auto-boot false", false)?;
    transport.command("saveenv", false)?;
    for (component, command) in [
        ("RestoreRamDisk", "ramdisk"),
        ("RestoreDeviceTree", "devicetree"),
        ("RestoreSEP", "rsepfirmware"),
    ] {
        println!("recovery: uploading {component}");
        transport.upload(signed.component(component)?)?;
        println!("recovery: activating {component}");
        transport.command(command, false)?;
        if command == "ramdisk" {
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    println!("recovery: uploading RestoreKernelCache");
    transport.upload(signed.component("RestoreKernelCache")?)?;
    transport.command(&format!("setenv boot-args {BOOT_ARGS}"), false)?;
    println!("recovery: starting restore kernel");
    transport.command("bootx", true)?;
    drop(transport);
    println!("recovery: waiting for restore USB personality");
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
    println!("recovery: setting final boot environment");
    transport.command("setenv auto-boot false", false)?;
    transport.command("saveenv", false)?;
    println!("recovery: uploading saved boot ticket");
    transport.upload(&signed.ticket)?;
    println!("recovery: activating saved boot ticket");
    transport.command("ticket", false)?;
    println!("recovery: uploading saved combined firmware");
    transport.upload(&signed.combined)?;
    println!("recovery: setting final kernel boot arguments");
    transport.command("setenv boot-args rd=md0", false)?;
    println!("recovery: issuing memboot");
    transport.command("memboot", true)?;
    drop(transport);
    println!("recovery: waiting for functional boot USB state");
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
    committed: &mut FdrCommits,
    persist: &mut impl FnMut(&str, &[u8]) -> Result<()>,
) -> Result<Value> {
    let kind = plist::text(plist::get(message, "DataType")?)?;
    match kind {
        "RootTicket" | "FDRTrustData" | "FDRMemoryCommit" | "KernelCache" | "DeviceTree"
        | "NORData" | "BuildIdentityDict" | "FUDData" => {
            println!("recovery: restore data request type: {kind}");
        }
        _ => {}
    }
    match kind {
        "RootTicket" => root_ticket(&signed.ticket, replay),
        "FDRTrustData" => Ok(plist::dict([])),
        "FUDData" => super::restore_images::fud_data(message, &firmware.identity, |name| {
            signed.component(name).map(<[u8]>::to_vec)
        }),
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
        _ => {
            // Retain only the unsupported type in protected attempt storage.
            // It must not be printed: unknown device strings can identify it.
            persist("unsupported-data-type", kind.as_bytes())?;
            Err(Error(
                "restore requested a data type outside the native T1 implementation",
            ))
        }
    }
}

fn root_ticket(ticket: &[u8], replay: Option<&[u8]>) -> Result<Value> {
    let mut values = std::collections::BTreeMap::new();
    values.insert("RootTicketData".into(), Value::Data(ticket.to_vec()));
    if let Some(replay) = replay {
        values.insert("FDRMemoryStoreData".into(), plist::decode(replay)?);
    }
    Ok(Value::Dictionary(values))
}

fn accept_commit(
    message: &Value,
    replay: Option<&[u8]>,
    committed: &mut FdrCommits,
    persist: &mut impl FnMut(&str, &[u8]) -> Result<()>,
) -> Result<Value> {
    let bytes = fdr_payload(message)?;
    if let Some(previous) = &committed.latest
        && plist::decode(&bytes)? == plist::decode(previous)?
    {
        println!("recovery: acknowledging repeated matching factory-data commit");
        return Ok(plist::dict([]));
    }
    if let Some(replay) = replay {
        let replay = plist::decode(replay)?;
        if committed.latest.is_none() && plist::decode(&bytes)? != replay {
            println!("recovery: device refreshed factory-data records during replay");
        }
    }
    let total = committed
        .stored_bytes
        .checked_add(bytes.len())
        .ok_or(Error("FDR commit storage budget exceeded"))?;
    if committed.count >= 16 || total > 32 * 1024 * 1024 {
        return Err(Error("FDR commit storage budget exceeded"));
    }
    let name = if committed.count == 0 {
        "FDRData".to_owned()
    } else {
        format!("FDRData.commit-{:04}", committed.count + 1)
    };
    // Preserve earlier snapshots; advance the authoritative store only after
    // this snapshot is durable and verified. Only then may restored get an ACK.
    persist(&name, &bytes)?;
    committed.count += 1;
    committed.stored_bytes = total;
    committed.latest = Some(bytes);
    println!("recovery: factory-data commit {} saved", committed.count);
    Ok(plist::dict([]))
}

fn component_reply(kind: &str, bytes: &[u8]) -> Value {
    plist::dict([(&format!("{kind}File"), Value::Data(bytes.to_vec()))])
}

pub(super) fn nor_data(
    message: &Value,
    firmware: &Firmware,
    signed: &SignedFirmware,
) -> Result<Value> {
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
    let status = plist::get(message, "Status")?;
    let success = match status {
        Value::Integer(status) => {
            if *status != 0 {
                eprintln!("recovery: device reported restore status {status}");
            }
            *status == 0
        }
        other => plist::integer(other)? == 0,
    };
    if !success
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
                "FUDData",
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
    fn commits_validate_input_and_advance_only_after_durable_storage() {
        let store = plist::dict([("SyntheticRecord", Value::Data(vec![1, 2, 3]))]);
        let message = plist::dict([("FDRData", store.clone())]);
        let replay = t1_bridge::bplist::encode(&store).unwrap();
        let updated = plist::dict([("SyntheticRecord", Value::Data(vec![9]))]);
        let changed = plist::dict([("FDRData", updated.clone())]);
        let different = t1_bridge::bplist::encode(&updated).unwrap();
        let mut committed = FdrCommits::default();
        assert!(
            accept_commit(
                &message,
                Some(b"invalid synthetic replay"),
                &mut committed,
                &mut |_, _| panic!("invalid replay must not persist")
            )
            .is_err()
        );
        assert!(committed.latest.is_none());
        assert!(
            accept_commit(&message, Some(&replay), &mut committed, &mut |_, _| Err(
                Error("synthetic disk full")
            ))
            .is_err()
        );
        assert!(committed.latest.is_none());
        let ack = accept_commit(
            &message,
            Some(&replay),
            &mut committed,
            &mut |name, bytes| {
                assert_eq!(name, "FDRData");
                assert_eq!(plist::decode(bytes).unwrap(), store);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(ack, plist::dict([]));
        assert_eq!(committed.count, 1);
        assert_eq!(
            accept_commit(&message, Some(&replay), &mut committed, &mut |_, _| panic!(
                "repeat must not rewrite"
            ))
            .unwrap(),
            plist::dict([])
        );
        let original = committed.clone();
        assert!(
            accept_commit(&changed, Some(&replay), &mut committed, &mut |_, _| Err(
                Error("synthetic disk full")
            ))
            .is_err()
        );
        assert!(committed == original);
        accept_commit(
            &changed,
            Some(&replay),
            &mut committed,
            &mut |name, bytes| {
                assert_eq!(name, "FDRData.commit-0002");
                assert_eq!(plist::decode(bytes).unwrap(), updated);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(committed.count, 2);
        assert_eq!(
            plist::decode(committed.latest.as_deref().unwrap()).unwrap(),
            updated
        );
        assert_eq!(committed.stored_bytes, replay.len() + different.len());
        // A later return to earlier content still gets a new snapshot.
        accept_commit(&message, Some(&replay), &mut committed, &mut |name, _| {
            assert_eq!(name, "FDRData.commit-0003");
            Ok(())
        })
        .unwrap();
        assert_eq!(committed.count, 3);
    }

    #[test]
    fn replay_is_supplied_in_root_ticket_and_refreshed_output_is_retained() {
        let input = plist::dict([("SyntheticRecord", Value::Data(vec![1]))]);
        let replay = t1_bridge::bplist::encode(&input).unwrap();
        let response = root_ticket(b"synthetic ticket", Some(&replay)).unwrap();
        assert_eq!(plist::get(&response, "FDRMemoryStoreData").unwrap(), &input);
        assert_eq!(
            plist::data(plist::get(&response, "RootTicketData").unwrap()).unwrap(),
            b"synthetic ticket"
        );
        assert!(root_ticket(b"synthetic", Some(b"invalid")).is_err());
        let updated = plist::dict([
            ("SyntheticRecord", Value::Data(vec![2])),
            ("SyntheticAdded", Value::Boolean(true)),
        ]);
        let mut committed = FdrCommits::default();
        let message = plist::dict([("FDRData", updated.clone())]);
        accept_commit(
            &message,
            Some(&replay),
            &mut committed,
            &mut |name, bytes| {
                assert_eq!(name, "FDRData");
                assert_eq!(plist::decode(bytes).unwrap(), updated);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            plist::decode(committed.latest.as_deref().unwrap()).unwrap(),
            updated
        );
        assert_eq!(plist::decode(&replay).unwrap(), input);
    }

    #[test]
    fn commit_limits_stop_before_writing_or_acknowledging_an_update() {
        let message = plist::dict([(
            "FDRData",
            plist::dict([("Synthetic", Value::Boolean(true))]),
        )]);
        for (count, stored_bytes) in [(16, 0), (1, 32 * 1024 * 1024)] {
            let mut committed = FdrCommits {
                latest: Some(t1_bridge::bplist::encode(&plist::dict([])).unwrap()),
                count,
                stored_bytes,
            };
            let original = committed.clone();
            assert!(
                accept_commit(&message, None, &mut committed, &mut |_, _| panic!(
                    "limit must precede persistence"
                ))
                .is_err()
            );
            assert!(committed == original);
        }
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
        for status in [-1, 14, i128::from(u64::MAX)] {
            assert_eq!(
                check_final_status(
                    &plist::dict([("Status", Value::Integer(status))]),
                    Some(b"synthetic")
                )
                .unwrap_err()
                .0,
                "EmbeddedOS restore reported failure"
            );
        }
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
