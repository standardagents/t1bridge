# Linux-only recovery source review

Review for [#25](https://github.com/standardagents/t1bridge/issues/25),
September 20, 2026. **Keep recovery an explicit, separately owned operation;
do not run it from T1Bridge installation or upgrades.**

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
an independently reproduced recovery. No firmware, device data or private
artifacts were downloaded, and no hardware restore commands were run.

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
| Supported model guard | Four 2016/2017 Touch Bar models are accepted and marked tested upstream. Its reports distinguish intact-ESP checks on 14,2 from regeneration on 13,2; the 13,2 report has cold-boot/Touch Bar evidence but no Touch ID test. Model acceptance is not full functional coverage. |
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

## Integration decision and next deliverable

Retain `t1-revive` as an external recovery tool under its own ownership. Do not
add it as a core dependency, vendor its scripts, distribute personalized Apple
assets, or create a privileged recovery daemon/socket. A future installer
handoff should detect missing data, explain the distinct attended recovery
step, then re-enter T1Bridge through the existing importer after recovery.

Before recommending that handoff as supported, resolve the failure/EFI gates
above and obtain a reproducible result on a machine that already needs
recovery, with backups and an agreed recovery procedure. Never erase a
working machine's EFI data to satisfy the test. This review advances the
source/integration decision; it does not replace current supported recovery
instructions or close #25.
