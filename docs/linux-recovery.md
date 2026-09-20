# Native Linux recovery

Implementation for [#25](https://github.com/standardagents/t1bridge/issues/25),
September 20, 2026. **Experimental: real-device recovery is not yet accepted.
Never run recovery from installation or upgrades.**

## Attended online command

The `t1bridge` package contains its own C/Rust recovery implementation:

```sh
sudo t1bridge machine-data recover --online --efi /path/to/mounted-esp
```

The path must identify the root of a mounted, writable FAT EFI system partition,
owned by root and not writable by other users. Install the matching
`t1bridge-dkms` package and reboot before using this command; its guarded reset
interface is part of `t1_cfgsel`. Do not run the command on a working machine.

The command checks all discovered preserved EFI sources first. Any existing
`EMBEDDEDOS` directory, partial data, interrupted staging directory, or inspection
failure blocks online recovery. Try a verified same-Mac backup with the local
import command first. Recovery requires a supported T1 Mac already in USB
recovery mode and an explicit `RECOVER` confirmation on an attended terminal.
Apple receives the T1's identity and signing nonces during signing. Factory
recovery uses device TLS tunnels to public Apple HTTPS endpoints.

The native sequence verifies the pinned Apple firmware package, provisions FDR,
resets only the T1, replays the recovered store in a second restore, resets the
T1 again, and boots the image and ticket from that second signing transaction.
It requires successful final restore status, a durable FDR commit, matching
replayed data and 30 seconds of stable functional USB configuration. Only then
does it save all three EFI files in a staged directory, flush and verify them,
rename the complete directory without replacing existing data, and run the
sensor-matching local importer.

Private attempts are retained under `/var/lib/t1bridge/recovery`, with directory
mode 0700 and file mode 0600. An interrupted `active` attempt blocks another
restore. It requires inspection before retry; the tool does not automatically
replay incomplete artifacts or resume an uncertain firmware mutation. Do not
publish these files, Apple tickets, FDR contents or device identities. Report the
last printed stage and error, package versions and model instead.

The initial implementation does not repartition device storage, enter DFU, reset
a functional T1, replace existing EFI generations, or automatically recover
interrupted attempts. Unknown restore requests fail explicitly. Apple service
availability and full device behavior remain unverified. Track native acceptance
in [#43](https://github.com/standardagents/t1bridge/issues/43): generated-data
import/calibration load, enrollment, match/non-match, warm/cold boot, Touch Bar,
camera and ALS. Existing local recovery did not require erasing EFI for a test.

## Dependencies and privilege boundary

The shipped restore protocol, USB mux, FDR control flow and EFI transaction are
T1Bridge code. System libcurl/OpenSSL provide verified HTTPS and SHA-256;
libarchive and liblzma provide archive decoding. The MIT-licensed `quick-xml`,
`base64` and `memchr` crates provide parsing primitives. No `t1-revive`,
idevicerestore, libirecovery, libimobiledevice, usbmuxd or acpi_call dependency is
used, and no recovery executable is launched.

Root is required to claim the recovery USB interface, inspect EFI partitions,
write the selected ESP and invoke the T1-only reset. `/dev/t1bridge-recovery` is
0600 and requires `CAP_SYS_RAWIO`; the kernel checks the supported Mac model,
the selected USB device and absence of a functional HID personality, then
discovers a unique zero-argument `FRST` method below that USB controller's ACPI
node. The caller cannot supply arbitrary ACPI paths. No listener or permanent
network service is added. Outbound sockets are limited to the pinned firmware
download, Apple signing and device-requested public Apple HTTPS destinations.

Protocol facts were checked against
[libirecovery](https://github.com/libimobiledevice/libirecovery/tree/95dec3aa25b1e30654ca107eb971971f6a216520),
[usbmuxd](https://github.com/libimobiledevice/usbmuxd/tree/3ded00c9985a5108cfc7591a309f9a23d57a8cba),
[libimobiledevice](https://github.com/libimobiledevice/libimobiledevice/tree/fa0f79190142bc309307967c058f89c1b36eb6b8),
[libtatsu](https://github.com/libimobiledevice/libtatsu/tree/60a39f36d719344360ec2e87563ed43f61f0530f),
and [idevicerestore](https://github.com/libimobiledevice/idevicerestore/tree/540c352c4c44896f7415abef87a166e8bbaea9b0).
The implementation was authored independently; these restore libraries are
reference material, not bundled source or runtime dependencies.

## Local data comes first

Use the preserved data on the target Mac before considering regeneration:

1. Discover attached EFI partitions read-only and validate their data against
   the live sensor through the existing machine-data importer.
2. If no usable EFI source remains, try an explicitly selected, verified
   backup from the same Mac through `t1bridge machine-data import --from`.
   Do not search arbitrary home directories or silently select a backup.
3. Consider online regeneration only after those local sources have been
   exhausted. It remains a separate attended recovery procedure with its own
   validation gates, not an automatic fallback from an import error.

An unreadable partition, unavailable sensor, malformed or conflicting source,
or protected-storage failure is a diagnosis to resolve; it is not proof that
the Mac has lost its data. Never delete working EFI or calibration state to
force the recovery path.

Local import extracts sensor-matched Touch ID calibration. It does not restore
the complete EFI boot tree. Restoring that tree from a verified same-Mac backup
is a separate operation; Linux regeneration additionally needs Apple services.
See [manual setup](setup.md#import-this-machines-apple-data) for the local
commands and error categories.

## Reviewed source

Reviewed [niconistal/t1-revive at a861170](https://github.com/niconistal/t1-revive/tree/a8611702ceee4947382c64f41035087cfbb6594e):
the orchestrator, provision/personalize/boot/stage/handover steps, firmware
verification, vendored patch set and Arch recipe. This is source review, not
an independently reproduced recovery. The implementation work later downloaded
and verified the pinned generic Apple package outside the repository to test
the native parser. No Apple assets are committed or included in packages. No
hardware restore commands were run on the owner's working Mac.

## What the implementation supplies

The [restore patch](https://github.com/niconistal/t1-revive/blob/a8611702ceee4947382c64f41035087cfbb6594e/vendor/patches/idevicerestore.patch)
advertises `FDRMemoryCommit`, saves the device's committed store and
acknowledges it. A second restore replays that store in the root-ticket
conversation and captures a personalized image and ticket. This supplies a
source-backed Linux path without an original host FDRData file; it uses Apple
services and surviving device identity. It is not offline fabrication of
factory calibration, and does not establish recovery from arbitrary damage to
the device's own persistent state.

| Boundary | Source finding |
| --- | --- |
| Supported model guard | Four 2016/2017 Touch Bar models are accepted and marked tested upstream. The 14,2 reports include an intact-ESP control and a separate regeneration/cold-boot/match/non-match run. The 13,2 reports include a cold-boot/Touch Bar run without Touch ID testing and a later enrollment run without completed verification. Model acceptance is not full functional coverage. |
| Generic firmware | `lib/firmware.sh` pins the Apple CDN package, size, checksum and extracted-file manifest. Upstream identifies bundle 901 / build 14Y901. Nothing here verifies Apple's current signing availability. |
| Tools/dependencies | Patched idevicerestore, libirecovery and usbmuxd run in a private prefix with pinned upstream references. The orchestrator uses Bash and Python extractors; it cannot be copied wholesale into this repository's C/Rust runtime. |
| Provisioning | `step_provision` asks the device/Apple restore service for a store and saves it privately. `step_personalize` uses that new store and captures an image/ticket pair. |
| Reset/boot | The chain discovers and invokes the T1 ACPI `FRST` reset and sends the captured pair to recovery mode. These are disruptive recovery operations, not ordinary driver setup. |
| Persistent boot | `cmd_stage` writes `combined.memboot`, `FDRData` and `version.plist` under `EFI/APPLE/EMBEDDEDOS`. No original EFI backup is required as input, but persistent boot still depends on newly populated EFI storage. |
| Handoff | Configuration selection and service readiness follow recovery. They do not establish enrollment, match/non-match, illumination, camera or ALS acceptance. |

Sources: [firmware](https://github.com/niconistal/t1-revive/blob/a8611702ceee4947382c64f41035087cfbb6594e/lib/firmware.sh),
[orchestrator](https://github.com/niconistal/t1-revive/blob/a8611702ceee4947382c64f41035087cfbb6594e/lib/cmd-regenerate.sh),
[stage](https://github.com/niconistal/t1-revive/blob/a8611702ceee4947382c64f41035087cfbb6594e/lib/cmd-stage.sh),
[package](https://github.com/niconistal/t1-revive/blob/a8611702ceee4947382c64f41035087cfbb6594e/packaging/arch/PKGBUILD).

## Gaps before installer integration

1. **Attempt-bound artifacts and failure handling.** Provisioning can continue
   after a nonzero restore result when a nonempty store exists. Personalization
   can continue with an existing image/ticket. Require artifacts produced by
   the current successful transaction, not merely files left in a reusable
   private directory. Do not automatically retry uncertain device mutations.
2. **Replay comparison.** In
   [personalize.sh](https://github.com/niconistal/t1-revive/blob/a8611702ceee4947382c64f41035087cfbb6594e/lib/steps/personalize.sh),
   a mismatch between provisioned and replayed FDRData is printed but does not
   set `ok=0`. The final strict gate checks return status and artifact presence;
   `--strict` alone does not make that comparison fatal. An isolated invocation
   of that function returned success with mismatching synthetic stores and
   pre-existing synthetic image/ticket files. Every hardware/restore helper
   was replaced by a stub; this checks the gate, not recovery on a device.
3. **EFI transaction recovery.** Each file is copied, synced and renamed
   separately. That prevents a partial individual file from taking its final
   name, but interruption between renames can leave a mixed three-file set.
   Define a recoverable generation/backup decision and test interruption at
   each boundary. Do not call the whole folder update atomic.
4. **Protect working installations.** The full chain can reset a live device;
   an existing EFI tree without an off-disk backup causes a warning, not a hard
   stop. Our installer must detect existing provisioning and offer recovery
   only through a separate attended flow. Ordinary upgrades must never enter
   that chain, including its unattended/demo modes.
5. **Independent acceptance.** Preserve the existing T1Bridge sensor-matching
   import and calibration-load postcondition. Validate enrollment, correct and
   incorrect finger results, warm/cold persistence and other T1 functions on
   each claimed model. A boot marker or ready status does not replace these.

The [upstream reports](https://github.com/niconistal/t1-revive/blob/a8611702ceee4947382c64f41035087cfbb6594e/README.md)
and [the 14,3 enrollment follow-up](https://github.com/standardagents/t1bridge/issues/29#issuecomment-5723441614)
provide useful reported evidence of missing-backup recovery and subsequent
Touch ID. They remain distinct from independent T1Bridge acceptance. The
13,3 enrollment and dark-panel issues, and 14,3 display timeout, must not be
collapsed into a blanket recovery-success claim.

## Native implementation boundary

Implement recovery directly in T1Bridge's C/Rust code. `t1-revive` is source
reference material, not a package dependency, vendored runtime or delegated
command. Do not replace it with another external recovery executable or add
a third-party restore stack. The proposed external-tool handoff in
[PR #44](https://github.com/standardagents/t1bridge/pull/44) was withdrawn
unmerged; it was never released.

The existing local importer already owns discovery, bounded source parsing,
sensor matching and protected storage. That local path passed the owner's
backup-integrity check, live-EFI comparison and explicit backup import. It
does not need an empty-EFI test.

The native implementation is in `crates/t1-import/src/recovery`, with focused
Linux/library boundaries in `t1-platform` and the guarded kernel reset in
`t1-cfgsel`. Apple's online services remain protocol inputs. Synthetic tests
and generic firmware parsing do not establish real-device recovery acceptance.

[#25](https://github.com/standardagents/t1bridge/issues/25) owns that work and
delivery in the signed package so testers need no source build. The owner
allows experimental package delivery before hardware acceptance, with the
unverified status stated explicitly. [#43](https://github.com/standardagents/t1bridge/issues/43)
retains generated-data import/calibration-load and functional/persistence
acceptance before promotion to supported recovery.

Keep recovery attended and local-first. Installation, upgrades and importer
errors must never trigger it automatically. Preserve existing association
checks, protected calibration and password access. Do not distribute Apple
assets or introduce an unjustified recovery daemon/socket. Validate failure
handling with synthetic fixtures and disposable storage; use a machine that
already needs recovery for live acceptance. Never erase working EFI data to
manufacture that case.
