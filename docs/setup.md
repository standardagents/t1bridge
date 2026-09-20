# Manual setup

> [!CAUTION]
> **STOP BEFORE PARTITIONING: KEEP AND BACK UP THIS MAC'S APPLE EFI DATA.**
> Preserve the Apple EFI partition and verify a backup on another device
> contains `EFI/APPLE/EMBEDDEDOS/FDRData`. Do this before installing Linux or
> formatting any partition. Without the original data or a matching backup,
> T1Bridge cannot set up Touch ID. Re-enrollment, reinstalling packages, and
> another Mac's backup are not substitutes.
>
> **Missing the data? Restore macOS on this Mac through a complete first boot
> to regenerate it.** Just booting Recovery or downloading the installer is
> not enough. Verify and back up `EFI/APPLE/EMBEDDEDOS/FDRData` before returning
> to Linux. Back up your Linux data first; restoring macOS can erase it.

Start with the [official package installation instructions](../README.md#install-official-packages)
for the public repository and signing key. This guide covers service activation,
machine-data import, fingerprint management, desktop integration and recovery.

The supplied packages currently support **x86_64 Arch Linux and Arch-based
distributions (including Omarchy) only**, with systemd 256 or newer. No official
Ubuntu, Mint, Debian, or Fedora packages are available yet; those distributions
need their own packaging and validation.
See [supported versions](dependencies.md#supported-and-tested-versions).

## Install and start the hardware stack

Keep a working password login and back up the disk before changing boot
configuration. Preserve the Apple EFI partition: it contains machine-specific
data needed by Touch ID. Do not format it as part of installing Linux.

Install these packages together from the official repository:

| Package | Purpose |
| --- | --- |
| `t1bridge` | Hardware services, importer, fingerprint broker, default Touch Bar |
| `t1bridge-dkms` | T1 configuration, display, network, and camera modules |
| `libfprint-t1bridge` | Standard fingerprint driver integration |
| `fprintd-t1bridge` | Matching fprintd integration and command-line tools |

The last two replace the distribution's libfprint/fprintd system-wide and must
stay a matched pair. They are optional for Touch Bar-only use. Use the
[published signing key](../README.md#1-trust-the-signing-key); do not disable
signature checks. Install headers matching the kernel you will
boot, and confirm DKMS and the normal initramfs generation complete successfully.

From your normal user account, after package installation:

```sh
sudo systemd-sysusers /usr/lib/sysusers.d/t1bridge.conf
sudo systemd-tmpfiles --create /usr/lib/tmpfiles.d/t1bridge.conf
sudo usermod -aG t1bridge "$(id -un)"
sudo systemctl daemon-reload
sudo systemctl enable t1-touchbar-hw.service t1-touchid-auth.socket t1bridge-fingerprint.socket
```

Reboot when convenient to load the packaged early configuration selector and
start a fresh login with the new group membership. Do not hot-swap competing
T1 stacks or reset the USB device to substitute for that boot. In the local
graphical session, start the default Touch Bar:

```sh
systemctl --user daemon-reload
systemctl --user enable --now t1-touchbar.service
sudo t1bridge status
```

Hardware access requires the active local graphical user as well as group
membership; an SSH session alone is not a substitute.

Read the individual status rows; a successful status command does not mean
every component is ready. The private network and xART units are device-driven.
The keybag relay starts when protected state exists; do not enable it as an
unconditional boot service. A new installation can legitimately show no
keybag before its first enrollment. See the [startup diagram](touch-id.md).

### Installing while the T1 is already running

An already-booted T1 may remain in USB configuration 1 after package
installation. Reboot remains the supported handoff to the packaged early
configuration selector. The new login also picks up `t1bridge` group membership;
re-enumerating USB alone cannot refresh an existing graphical session's groups.

Two successful no-reboot handoffs were [reported on MacBookPro14,3](https://github.com/standardagents/t1bridge/issues/22)
after explicitly deauthorizing and reauthorizing an idle T1. That observation
does not yet establish a supported installation command. The existing
`t1bridge validate usb-cycle` is a development validation tool: it requires
configuration 2, `t1bridge-cfgselector` ownership, and the expected interface
drivers before cycling. It therefore cannot perform the configuration-1
installation handoff, and its checks must not be weakened to make it do so.

A future no-reboot handoff needs separate validation of the starting driver
state, exclusive ownership, quiescing of all affected T1 clients, recovery on
interruption, and configuration-2 readiness. Until that path is implemented and
tested, complete installation with a reboot rather than raw USB toggles.

## Import this machine's Apple data

Use local data first: preserved EFI, then a verified backup from this same Mac.
Import reads those sources and extracts Touch ID calibration; it does not
restore the complete EFI boot tree, download firmware, or run regeneration.

With the matching preserved Apple EFI partition attached:

```sh
sudo systemctl start t1bridge-import.service
sudo systemctl status t1bridge-import.service
```

The sandboxed importer discovers EFI partitions, reads them without modifying
them, matches data to the live sensor, and stores the selected record under
root-only `/var/lib/t1bridge/machine-data/`. Missing or conflicting data is an
error, not permission to choose an arbitrary partition.

Already-mounted EFI partitions are inspected through a private read-only view;
the importer does not change their existing mount flags.

On 0.1.9, automatic import can fail with exit 30 and `private import temporary
file could not be created`, even when the destination is writable outside the
service ([#36](https://github.com/standardagents/t1bridge/issues/36)). EFI
discovery changes mount namespaces; the old storage handle can miss the
service's writable mount. Version 0.1.10 reacquires that handle before commit.
On older packages, use the explicit source command below with this Mac's
preserved EFI tree or backup. Changing `WorkingDirectory` or
loosening the service sandbox is not needed. That exit-30 message identifies
the commit stage; an EFI discovery failure is reported as a source failure.

To use a backup instead, supply its absolute path:

```sh
sudo t1bridge machine-data import --from /path/to/efi-backup
```

The directory must contain `EFI/APPLE/EMBEDDEDOS/FDRData`. You can also pass the
absolute path to that `FDRData` file directly. This works for a copied backup
or a manually mounted preserved EFI filesystem. The source must originate
from the target Mac; a different Mac's data is rejected by live-sensor matching.
No symlink component or special file is accepted. The backup is read-only,
diagnostics do not print its path or identifiers, and a different existing
calibration record is never overwritten.

Run import while no fingerprint operation is in progress. A sensor-unavailable
error does not change calibration; retry when the reader is idle rather than
resetting hardware or deleting state.

Use the import exit category to choose the next step. An import failure never
automatically triggers online recovery:

| Exit | Meaning and next step |
| --- | --- |
| 20 | Sensor unavailable. Resolve hardware/service readiness and retry while idle. |
| 21 | Discovery found no preserved local source. Check the available EFI partitions, then select a verified same-Mac backup explicitly. |
| 22 | Source unreadable. Resolve access, mount or read errors; this does not establish data loss. |
| 23–25 | Invalid, nonmatching or conflicting data. Resolve the source problem; never use donor data or choose an arbitrary record. |
| 26–30 | Import reservation or protected-storage failure. Resolve that failure; do not delete working calibration or regenerate firmware. |

Compressed backups and macOS installer/disk-image containers are not accepted
by this command yet. Extract a backup you control first; do not copy guessed
records into protected storage. An installer archive is not this Mac's
calibration backup. If the data was lost, restore macOS on this Mac and complete
its first boot, then preserve and back up the regenerated EFI data. This is a
macOS recovery procedure, not something the T1Bridge importer performs.

A separate Linux recovery project is under [source and integration
review](linux-recovery.md). Consider that attended online route only after
usable local EFI and same-Mac backups have been exhausted. Its reported results
do not yet establish a supported T1Bridge installer recovery path.

## Enroll and verify

Run these commands as your normal user in a terminal inside your graphical
desktop session, with a working Polkit authentication agent:

```sh
fprintd-list "$(id -un)"
fprintd-enroll -f right-index-finger
fprintd-verify
```

Choose the intended finger label before enrollment. Authorization may require
your password; the distro's Polkit policy determines that prompt. Keep lifting
and touching the same finger until enrollment completes, then verify it.
T1Bridge currently permits three enrolled identities for one Linux owner.
Use `fprintd-list` again to confirm the recorded label. Do not use the direct
`t1bridge enroll` development path for standard fingerprint management.

### Enrollment timeouts

`EnrollStart failed: Timeout was reached` does not by itself identify an xART
firewall failure. Enrollment requires Polkit authorization for
`net.reactivated.fprint.device.enroll`. An SSH session, script, or agent-driven
shell without an available authorization agent can time out because the
password prompt cannot be shown.

Before changing firewall rules, inspect the fprintd journal locally:

```sh
journalctl -b -u fprintd --since "5 minutes ago" --no-pager
```

If it reports `Authorization denied` for `EnrollStart` and that Polkit action,
retry from your graphical desktop terminal and complete the authorization
prompt. Do not use `sudo fprintd-enroll` to bypass the policy. If authorization
succeeds but enrollment still fails, continue with the
[private xART firewall checks](#firewall-recovery-and-removal). Keep the journal
private; report only the denied action and error, not the full log.

### Remove a print

To deliberately remove one print, substitute its exact listed label:

```sh
fprintd-delete "$(id -un)" -f right-middle-finger
fprintd-list "$(id -un)"
```

Do not omit `-f`: that requests deletion of all the user's prints. The utility
visits every detected reader, so check the device list first if more than one
is connected. Retain a working password regardless of how many prints remain.

## Enable fingerprint sign-in safely

Enrollment does not configure sudo, Polkit, or your lock screen. Use your
distribution's supported **pam_fprintd** configuration for each consumer, and
consult `man pam_fprintd`. T1Bridge does not replace PAM files or ship a custom
PAM module.

Before editing PAM, verify password authentication and keep a persistent root
recovery shell open. Preserve password fallback and existing account/access
checks; do not paste a replacement PAM stack from another distribution.
Use bounded attempts/timeouts. The default fingerprint timeout is 30 seconds,
and a serial PAM conversation can delay the password prompt until fingerprint
authentication finishes.

Before closing the recovery shell, test both a successful fingerprint and
password fallback with the fingerprint service unavailable, separately for
sudo, Polkit, and the lock screen. Never assume that success in one consumer
proves the others. T1Bridge's Touch Bar prompt is cosmetic, not proof that the
requesting application accepted authentication.

### Omarchy lock-screen unlock

After enrollment and `fprintd-verify` succeed, Omarchy's Quickshell lock screen
also needs `/etc/pam.d/omarchy-lock-fingerprint`. It uses a separate
`omarchy-lock-password` service for password unlock. These instructions apply
to that lock screen, not older Hyprlock configurations or other desktops.

First verify password unlock and keep the persistent root recovery shell
described above open. If the fingerprint file already exists, back it up,
inspect it, and preserve local account/access policy. To create a missing
file, run:

```sh
sudoedit /etc/pam.d/omarchy-lock-fingerprint
```

Use the following configuration, matching
[Omarchy's fingerprint service](https://github.com/basecamp/omarchy/blob/quattro/bin/omarchy-setup-security-fingerprint)
with bounded attempts and a ten-second timeout:

```text
#%PAM-1.0
auth       required    pam_fprintd.so max-tries=3 timeout=10
account    include     system-local-login
```

Keep `/etc/pam.d/omarchy-lock-password` unchanged: password fallback is handled
by the lock screen's independent password conversation, including when the
fingerprint module, broker, socket, or device fails. This separate fingerprint
service is not a replacement for a sudo, Polkit, or login PAM stack.

Lock the screen and check fingerprint unlock, then verify password unlock
after a failed or cancelled fingerprint attempt and with the fingerprint
service unavailable. Keep the recovery shell open through these checks. To
undo this setup, remove only the fingerprint file you created; restore a
prior file from your backup if you edited one. Enrollment is unaffected.

Do not run the generic Omarchy fingerprint setup wizard just to create this
file: it can replace the matched T1Bridge libfprint/fprintd packages. This
configuration enables lock-screen unlock only; sudo and Polkit remain
separate consumers.

## Desktop controls and customization

The default renderer is included. Hardware controls use T1Bridge's advertised
capabilities; desktop audio/media actions and notifications need an optional
provider. No desktop provider is bundled.

To use a compatible provider, set `T1BRIDGE_DESKTOP_PROVIDER` to its absolute
executable path in a user-service drop-in (`systemctl --user edit
t1-touchbar.service`), then restart that user service. To replace the renderer,
create an executable or symlink at `${XDG_CONFIG_HOME:-$HOME/.config}/t1bridge/renderer`
and restart it. Neither program should run as root. Follow the
[provider and renderer contracts](interfaces.md#renderer-selection-v1), not a
private hardware API. The reusable management TUI and baseline desktop provider
are separate planned packages, not prerequisites for the commands above.

### Omarchy session startup

For Omarchy controls and HUDs, install the optional
[t1bridge-omarchy integration](https://github.com/standardagents/t1bridge-omarchy#install-and-enable)
version 0.2.2 or newer. It supplies package-owned XDG autostart; no per-user
hook is required. The package's README owns migration from the older manual
hook and removal instructions. Remove an old hook only after verifying that
it points to this package's helper, to avoid a second restart at login.

The core renderer starts with the user manager and can precede the graphical
session. The optional integration skips a start with an absent or empty
display environment, then invokes the packaged helper through XDG autostart
after graphical-session readiness. It imports the required desktop variables
and starts or restarts only an enabled renderer. Disabled/masked units and
custom renderer selection are preserved. Cold-login hardware acceptance is
still pending in [#35](https://github.com/standardagents/t1bridge/issues/35).
After installing, log out and back in, or run the helper from a terminal
inside the active Omarchy session:

```sh
/usr/lib/t1bridge-omarchy/session-start --autostart
```

Check controls and HUDs again after the next login. A renderer that now resolves
its provider can still have a dark panel; that separate report is
[#34](https://github.com/standardagents/t1bridge/issues/34).

## Firewall, recovery, and removal

xART needs inbound IPv6 TCP port 61500 on the dynamically discovered private
T1 network interface. Keep IPv6 enabled there. If your firewall blocks it,
restrict any exception to that interface and the validated T1 peer; never open
the port on Wi-Fi, Ethernet, or all interfaces. Do not save a machine-specific
interface name as a portable rule. The listener also enforces device and peer
admission. Cross-machine peer validation remains an open release gate.

Check this **before enrollment**: testers with a default-deny firewall saw a
first enrollment timeout followed by immediate failures until the private
link was permitted. `xart: ready` means the service is active, not that inbound
traffic reaches it. With opt-in diagnostics enabled, `xart-admission` reports
recorded evidence from this boot; unavailable records are not a failed health
check, and old admission evidence does not prove current reachability.

On failure, inspect `sudo t1bridge status` and the relevant systemd journal.
Do not delete `/var/lib/t1bridge`, reset enrollment, or run USB lifecycle
validation commands as generic repair steps. Protect diagnostic logs before
sharing them. Keep existing protected state through upgrades and reinstalls.

Before uninstalling, restore and test password-only authentication using your
distro's configuration tools, with the root recovery shell still open. Stop
the user renderer and the T1Bridge services, sockets, and device-driven xART
instances. Remove the packages through the package manager, restore the
distribution's matched libfprint/fprintd pair if needed, and rebuild the
initramfs through its normal mechanism before rebooting. Package removal is
not authorization to erase protected state or the preserved Apple EFI partition.
