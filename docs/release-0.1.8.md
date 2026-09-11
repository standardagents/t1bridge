# v0.1.8

Require the distribution `iio-sensor-proxy` so T1 ambient-light readings are
automatically discoverable through standard desktop interfaces. Configuration 2
already exposes the sensor through the stock Linux HID/IIO drivers; no new
kernel module is introduced.

The separately installed `t1bridge-omarchy` 0.2.0-2 package adds optional,
unprivileged automatic panel brightness. It adapts gradually to ambient light,
respects manual adjustments, and starts automatically at graphical login once
enabled. See [sensor support](ambient-light.md) for installation and controls.

On MacBookPro13,3, attended tests confirmed cover/uncover dimming and recovery,
a manual 100% reference beyond the 30-second hold, healthy operation after
lock/unlock, and automatic startup after reboot. Tests did not observe the
locked interval. Other-model and concurrent camera/authentication acceptance
remain open in #17; system suspend and runtime power saving remain unsupported.

Core packaging checks and the existing quality suite passed. The optional
package passed policy/discovery/session tests and isolated install, reinstall,
upgrade and removal checks. Kernel and fingerprint protocol code are unchanged.

The core cohort is `t1bridge`/`t1bridge-dkms` 0.1.8-1,
`libfprint-t1bridge` 1.94.100-16 and `fprintd-t1bridge` 1.94.5-13.
Update with `omarchy update` or `sudo pacman -Syu`; no re-enrollment is needed.
The #14 relay diagnostics and restart backoff remain evidence collection and
mitigation, not a confirmed root-cause fix.
