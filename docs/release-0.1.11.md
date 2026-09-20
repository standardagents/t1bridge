# v0.1.11

Failed Touch Bar information sends now free their allocated reply buffer.
Opt-in kernel diagnostics report information/update reply results and bounded
read-attempt counts without pixels, payloads or identifiers. The actual
exchange functions have synthetic allocation, timeout, short-transfer and
stale-response coverage. This fixes the leak; it does not fix the reported
display timeout or dark panels in #31/#34. Accepted T1 update replies are not
timestamp-matched and do not prove frame presentation or illumination.

The optional `t1bridge-omarchy` 0.2.2-1 package supplies XDG autostart after
graphical-session readiness. Its drop-in skips a renderer start with an
absent/empty display environment, and its helper starts or restarts only
enabled units. Disabled/masked units and custom renderer selection are
preserved. No per-user startup hook is required. If an older manual hook
exists, remove it only after verifying that it points to the packaged helper.
See [session startup](setup.md#omarchy-session-startup).

Local `make quality`, kernel builds and sanitizers passed. The optional
package passed helper, real systemd-condition/XDG-generator and isolated
install/reinstall/upgrade/removal checks. Cold-login acceptance of the new
startup path remains pending in #35; publication does not establish it.

The documentation now separates camera format metadata from application
acceptance, runtime residency from measured power, and public SEP API/test
doubles from the missing T1 wire contract. The external Linux recovery review
identifies artifact-generation, replay-comparison and interrupted EFI update
gates. No device recovery or new hardware acceptance was performed.

The signed cohort is `t1bridge`/`t1bridge-dkms` 0.1.11-1,
`libfprint-t1bridge` 1.94.100-19, `fprintd-t1bridge` 1.94.5-16 and optional
`t1bridge-omarchy` 0.2.2-1. The fingerprint pair is rebuilt for this cohort;
its protocol is unchanged.

On Omarchy, upgrade with `omarchy update`; other supported Arch installations
use a full `sudo pacman -Syu`. Confirm DKMS and initramfs/UKI generation succeed,
then reboot to load the new driver. Keep machine data and enrolled prints.

This remains a prerelease. Relay failures, system suspend, runtime power
saving, broader camera/ALS coverage, general SEP key services, Linux recovery,
the original enrollment failure and the display reports remain open.
