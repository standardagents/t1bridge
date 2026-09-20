# Experimental online recovery

T1Bridge offers an explicit handoff to the optional `t1-revive` package when
this Mac has no usable local provisioning data. This integration is
**experimental and awaiting a recovery tester**. The recovery tool resets the
T1, contacts Apple's services and writes `EFI/APPLE/EMBEDDEDOS`; failure can
leave the T1 needing repair. Installing either package does not run recovery.

Use this procedure only on a machine that already needs recovery. Keep any
surviving data backed up off the machine, stay at the terminal on mains power,
and preserve a working password login. Never erase working EFI data to test it.

## Use local data first

Inspect preserved EFI data and any backup from this Mac before regeneration.
Use the [existing importer](setup.md#import-this-machines-apple-data) when the
T1 is available:

```sh
sudo t1bridge machine-data import
```

For a backup, select its absolute path explicitly:

```sh
sudo t1bridge machine-data import --from /path/to/efi-backup
```

If import succeeds, stop here. A hardware, access, validation, conflict or
storage error does not establish data loss. If the T1 is stuck in recovery
mode, its sensor cannot answer an import request: inspect the preserved EFI
and same-Mac backups separately. Restore a usable saved boot tree before
considering regeneration. Calibration import alone does not restore that tree.

## Install the optional tool

The core Arch recipe declares `t1-revive>=0.1.3` as an optional dependency.
It is separately maintained and is not bundled in T1Bridge's signed package
repository. As checked on September 20, 2026, the AUR has no `t1-revive` entry.
Build the upstream Arch package from the reviewed commit, which includes the
corrected release-tarball checksum:

```sh
git clone https://github.com/niconistal/t1-revive.git
cd t1-revive
git checkout --detach a8611702ceee4947382c64f41035087cfbb6594e
cd packaging/arch
makepkg --syncdeps --install
```

Run `makepkg` as your normal user; it requests sudo for dependencies and
installation. Review the recipe before building. The package builds its
pinned libimobiledevice toolchain; it does not contain Apple firmware or
personalized data. The recovery tool downloads firmware from Apple at runtime.

Install headers matching the running kernel. The upstream recipe also lists
stock `linux-headers`; those do not satisfy a different running kernel.
Check the tool's version and preflight before attempting recovery:

```sh
t1-revive version
sudo t1-revive preflight
```

Preflight checks the model, EFI selection, network and dependencies. It can
load `acpi_call` and create private state/log directories. It does not perform
a firmware restore. Resolve its reported failures first; its default invocation
does not install packages. Consult the [upstream instructions](https://github.com/niconistal/t1-revive/tree/a8611702ceee4947382c64f41035087cfbb6594e)
for the supported T1 models and prerequisites.

## Run the attended recovery

The T1Bridge command below is new source functionality; released v0.1.11 and
earlier do not contain it. Until a package includes it, build the candidate CLI
from this T1Bridge checkout's root, leaving the installed services in place:

```sh
cargo build --locked --release -p t1-import --bin t1bridge
sudo ./target/release/t1bridge machine-data recover --online
```

After a package containing this change is installed, the equivalent command is:

```sh
sudo t1bridge machine-data recover --online
```

The command requires root, an interactive terminal and the word `recover` at
its prompt. That acknowledgement confirms the operator's source decision; the
launcher does not search for backups or declare existing data unusable.
It then runs the installed `/usr/bin/t1-revive --confirm-each regenerate` with
confirmation enabled and a clean environment. It accepts no force, unattended,
alternate-executable or arbitrary pass-through options. Follow the tool's
step-by-step prompts. Ordinary installation, upgrades, services and failed
imports never invoke it.

After the tool exits successfully, T1Bridge runs its existing sensor-matched
import. An existing different calibration record is still rejected. Recovery
failure never reaches import, and neither stage is retried automatically.

| Result | Next action |
| --- | --- |
| Exit 0 | Recovery and sensor-matched import returned success. Check status and the functions below. |
| Exit 2 or 3 | Correct command syntax or root authority; recovery has not started. |
| Exit 32 | Handoff was refused, cancelled, unavailable, interrupted or failed. Read the message and the tool's guidance; do not blindly repeat firmware recovery. A reported tool exit 7 means it requested a reboot. |
| Import exit 20–30 | Recovery returned success, but import failed. Diagnose using the [import categories](setup.md#import-this-machines-apple-data), then retry import only. |

The separately prepared transaction/retry fixes are not in upstream 0.1.3.
Interruption and retry risks found in the [source review](linux-recovery.md#gaps-before-installer-integration)
remain relevant. Do not use forced staging or resume after an uncertain write
without inspecting the tool's state and recovery instructions.

## Report a test

Record the T1Bridge and `t1-revive` versions, model, kernel, whether usable local
data was already absent, each command's exit status, and any failed step.
After successful import, run `sudo t1bridge status`, then follow the existing
[enrollment and verification procedure](setup.md#enroll-and-verify). Record
enrollment, an enrolled-finger match, and a non-enrolled-finger rejection.
Check warm reboot and full shutdown/power-on persistence, Touch Bar controls,
camera capture and ALS readings. Mark anything untested explicitly.

Share only redacted results in [the recovery validation issue](https://github.com/standardagents/t1bridge/issues/43). Do not upload EFI
contents, calibration, personalized firmware/tickets, keybags, biometric state,
raw restore logs or machine identifiers. A ready status or exit 0 alone does
not establish the full functional result. No PAM change is needed for this test.
