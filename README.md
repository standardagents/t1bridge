# T1Bridge

Open-source Linux support for Apple's T1 iBridge. The goal is to support
**every function directly handled by the T1**, including its display, camera,
Touch ID, sensors, and device lifecycle—not only fingerprint authentication.
Current coverage and remaining work are listed below.

> [!CAUTION]
> **STOP BEFORE ERASING OR PARTITIONING YOUR MAC: PRESERVE ITS APPLE EFI DATA.**
>
> Touch ID needs this Mac's original `EFI/APPLE/EMBEDDEDOS/FDRData`.
> Keep the Apple EFI partition and make a separate backup on another device
> **before installing Linux or formatting any partition**. Check that the
> backup actually contains that path; keeping only Linux boot files is not enough.
>
> **Without this data or a matching backup, T1Bridge cannot set up Touch ID.**
> Re-enrolling fingerprints, reinstalling this package, or another Mac's backup
> cannot substitute for it.
>
> **Already erased it? Recovery is possible:** restore macOS on this Mac and
> let it complete its first boot so the machine-specific EFI data is regenerated.
> Merely downloading an installer or booting into Recovery is not enough.
> Then verify and back up `EFI/APPLE/EMBEDDEDOS/FDRData` before returning to Linux.
> Back up your Linux data before restoring macOS; restoration can erase it.

Install the official signed packages from **linux.standardagents.ai**.
No source build, GitHub authentication, or download token is required.

## Supported hardware

| MacBook Pro | Model identifier | Hardware testing |
| --- | --- | --- |
| 2016, 13-inch with Touch Bar | `MacBookPro13,2` | 🟡 Targeted; awaiting tester confirmation |
| 2016, 15-inch with Touch Bar | `MacBookPro13,3` | 🟢 Success confirmed on two machines; limitations below |
| 2017, 13-inch with Touch Bar | `MacBookPro14,2` | 🟡 Targeted; awaiting tester confirmation |
| 2017, 15-inch with Touch Bar | `MacBookPro14,3` | 🔴 Enrollment failure reported; details pending |

**MacBookPro13,3 is the only model with confirmed success**, now reported on
two machines. One MacBookPro14,3 tester reports enrollment failure; package
versions, exact failing stage and logs are not yet available. This is not a
diagnosed cause or evidence that every function on that model fails. The other
models remain unconfirmed targets. Reports from testers are welcome
through [GitHub issues](https://github.com/standardagents/t1bridge/issues);
include your model, kernel/package versions and which functions work or fail,
but no serial numbers or machine-specific EFI data.

T2 Macs, Apple Silicon, and models without a Touch Bar are outside this
project's hardware scope. **Currently installable official packages are for
x86_64 Arch Linux and Arch-based distributions, including Omarchy, only**
(systemd 256 or newer). There are no official Ubuntu, Mint, Debian, or Fedora
packages yet. The source is desktop-neutral; supporting another distribution
requires packaging and validation, not installing these Arch packages there.
See [supported versions](docs/dependencies.md#supported-and-tested-versions).

## T1 function support

🟢 Available · 🟡 Partial / integration needed · 🔴 Not working or not implemented

These statuses describe current coverage, not a claim that every model and
kernel combination has been tested. Missing T1 functionality remains in scope.

| Function | Current status | Details |
| --- | --- | --- |
| Touch Bar display and touch input | 🟢 Available | Default renderer, Escape, hardware controls, and F1–F12 while Fn is held. This is the Touch Bar display, not the laptop's main GPU/display. |
| Screen and keyboard brightness buttons | 🟢 Available | Controls the machine's available Linux backlights; this does not mean T1Bridge owns those backlight drivers. |
| Custom Touch Bar renderers | 🟢 Available | Unprivileged programs through the [renderer interface](docs/interfaces.md#renderer-selection-v1). Try the optional [Doom demo](#try-a-custom-touch-bar-ui). |
| Volume, media controls, desktop HUDs | 🟡 Optional integration | Requires a desktop provider; none is bundled in the core package. |
| Touch ID enrollment, matching and deletion | 🟢 Available | Standard fprintd tools; up to three enrolled fingers for one Linux owner. Requires preserved Apple EFI data. |
| sudo, Polkit and lock-screen authentication | 🟡 Requires configuration | Uses `pam_fprintd`; configure each consumer and retain password fallback. |
| Saved fingerprints across reboot | 🟢 Available | Protected keybag storage and automatic restore; no routine re-enrollment. |
| FaceTime HD camera | 🟢 Available | T1 H.264 support through the packaged UVC driver. Application format support still applies. |
| Private T1 network and xART storage | 🟢 Available | Device-driven services; no manually named network profile required. |
| Apple machine-data import | 🟢 Available | Matching preserved EFI partition or explicit EFI-tree/FDR backup. Import does not recreate lost data. |
| T1 startup and reboot recovery | 🟢 Available | Packaged device/service ordering restores the T1 stack after boot. |
| T1 sleep/wake (system suspend/resume) | 🔴 Not working on the tested machine | Not supported currently. T1 recovery across system sleep/wake remains in scope; the cause of the host suspend failure is not established here. Screen blanking and waking the display are not system suspend/resume. |
| T1 runtime power saving | 🟡 Limited | Runtime autosuspend is disabled for T1 stability; power-saving suspend/recovery is not a supported feature yet. |
| Ambient-light sensor | 🔴 Planned | Linux IIO integration is not shipped. |
| General Secure Enclave key services | 🔴 Not implemented | No general-purpose signing/key-management API; Touch ID support does not imply these services exist. |

Wi-Fi, Bluetooth, speakers, the internal keyboard/trackpad, GPU switching and
host-wide power management are separate from T1Bridge. **Recovery of the T1's
own functions during host sleep/wake is in scope**; fixing unrelated GPU,
firmware, or platform suspend problems is not. This is not a complete MacBook
hardware-enablement bundle.

## Try a custom Touch Bar UI

The separate Standard Agents [touchbar-doom](https://github.com/standardagents/touchbar-doom)
package is an optional demo for Omarchy users who want to try a nonstandard
Touch Bar UI. It runs playable Doom with a panoramic game view, labeled HUD, weapon artwork,
and a mute toggle through T1Bridge's unprivileged renderer interface.

Follow its [installation instructions](https://github.com/standardagents/touchbar-doom#install-the-optional-package)
to install the package and supply the Doom shareware game data. Open **Touch Bar
Doom** from the application launcher or run `touchbar-doom launch`. Tap **QUIT**
at the far left, or press Fn, to restore the previous renderer.

It is not bundled with T1Bridge or installed by default. Installing it does not
change the active renderer; launching it is an explicit user action. Gameplay
captures the keyboard until Quit or Fn releases it. A working T1Bridge
Touch Bar is required first; the demo does not install hardware drivers.

## Install official packages

> [!CAUTION]
> **Before continuing: preserve the Apple EFI partition and verify an external
> backup contains `EFI/APPLE/EMBEDDEDOS/FDRData`. Without this Mac's data,
> Touch ID setup will not work. Do not format the partition.**
> If it is already missing, restore macOS through a complete first boot,
> then preserve and back up the regenerated EFI data before returning to Linux.

Keep a working password login and back up your disk.

### 1. Trust the signing key

Run from your normal account:

```bash
key_dir=$(mktemp -d)
curl -fSLo "$key_dir/t1bridge-signing-key.asc" \
  https://linux.standardagents.ai/arch/standardagents/x86_64/t1bridge-signing-key.asc
gpg --show-keys --with-fingerprint "$key_dir/t1bridge-signing-key.asc"
```

Check that the **primary fingerprint** is exactly:

```text
35B166F78B063B04DE1E3D913E6C4216EB03D371
```

Only after it matches:

```bash
sudo pacman-key --add "$key_dir/t1bridge-signing-key.asc"
sudo pacman-key --lsign-key 35B166F78B063B04DE1E3D913E6C4216EB03D371
```

### 2. Add the repository and install

Add this block once to `/etc/pacman.conf`, preserving your existing repositories:

```ini
[standardagents]
SigLevel = Required DatabaseRequired
Server = https://linux.standardagents.ai/arch/$repo/$arch
```

Keep `$repo` and `$arch` literal in that file. For the standard Arch `linux` kernel:

```bash
sudo pacman -Syu --needed linux-headers t1bridge t1bridge-dkms libfprint-t1bridge fprintd-t1bridge
```

Use the header package matching your kernel if it is not `linux`. This performs
a normal system upgrade. Confirm DKMS and initramfs/UKI generation succeed
before rebooting; do not bypass signature or dependency errors.

| Official package | Purpose |
| --- | --- |
| `t1bridge` | Services, importer, Touch ID broker and default Touch Bar |
| `t1bridge-dkms` | T1 configuration, display, network and camera kernel modules |
| `libfprint-t1bridge` | T1Bridge driver for the standard fingerprint API |
| `fprintd-t1bridge` | Matched fingerprint daemon, tools and PAM module |

The fingerprint packages replace distro libfprint/fprintd system-wide and must
stay a matched pair. For Touch Bar/camera-only use, omit those two packages.

### 3. Enable hardware support

Still from your normal account:

```bash
sudo systemd-sysusers /usr/lib/sysusers.d/t1bridge.conf
sudo systemd-tmpfiles --create /usr/lib/tmpfiles.d/t1bridge.conf
sudo usermod -aG t1bridge "$(id -un)"
sudo systemctl daemon-reload
sudo systemctl enable t1-touchbar-hw.service t1-touchid-auth.socket t1bridge-fingerprint.socket
```

Reboot to load the modules and refresh group membership. Then, in your local
graphical session:

```bash
systemctl --user daemon-reload
systemctl --user enable --now t1-touchbar.service
sudo t1bridge status
```

Check each status row. `keybag: not-enrolled` is normal before first enrollment.
The private network and xART services start with the device; do not enable the
keybag relay as an unconditional boot service or hot-swap competing T1 drivers.

### 4. Set up Touch ID

With this Mac's preserved Apple EFI partition attached and the reader idle:

```bash
sudo systemctl start t1bridge-import.service
sudo systemctl status t1bridge-import.service --no-pager
```

For a copied backup instead, see [backup import](docs/setup.md#import-this-machines-apple-data).
After successful import, enroll and verify from your normal account:

```bash
fprintd-list "$(id -un)"
fprintd-enroll -f right-index-finger
fprintd-verify -f right-index-finger
```

If you already have enrolled fingers, verify an existing one instead of enrolling
it again. Choose the correct finger label and repeatedly lift/touch during
enrollment. Require `enroll-completed` and then `verify-match`.

Enrollment does **not** automatically enable sudo, Polkit or lock-screen login.
Follow [safe PAM setup](docs/setup.md#enable-fingerprint-sign-in-safely) for your
distribution, preserving password access and a root recovery shell. Do not
blindly run a desktop setup wizard that replaces this matched fingerprint pair.

See [manual setup](docs/setup.md) for desktop providers, removal and recovery,
and [How Touch ID works](docs/touch-id.md) for startup and authentication diagrams.

## Package boundaries

T1Bridge owns T1 hardware support, protected machine-data import and recovery,
the fingerprint backend, and the default Touch Bar. Its core does not depend on
desktop integration or the separately packaged libfprint/fprintd integration.

Standard fprintd tools provide fingerprint management. A reusable management
TUI and a baseline desktop-controls provider are planned as separate optional
packages. Distribution integrations own installer preservation, automatic
setup, menus, themes, and HUD integration. Custom Touch Bar renderers remain
user-selected programs, not bundled presets.

## Contributing and security

See [contributing](CONTRIBUTING.md) for development and bug reports, and
[security reporting](SECURITY.md) for private vulnerability reports.
For import, Touch ID, or Touch Bar failures, see [opt-in shareable diagnostics](docs/diagnostics.md).

Maintained by Andrew Boyd.
