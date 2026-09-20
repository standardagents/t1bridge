# Runtime power investigation

Next step for [#19](https://github.com/standardagents/t1bridge/issues/19).
Keep the existing stability policy until a coordinated replacement is tested.

## Current boundary

The T1 camera entry in [uvc_driver.c](../kernel/uvcvideo-t1/uvc_driver.c)
includes `UVC_QUIRK_DISABLE_AUTOSUSPEND`; its probe therefore does not enable
USB autosuspend. The NCM driver delegates suspend/resume to usbnet. Display
resume has its own rearm/reprobe path, while the userspace SEP owner manages
exclusive operations and a long-running keybag relay. A driver's
`supports_autosuspend` flag does not prove the composite device can recover
all these consumers together.

A read-only September 20 observation on MacBookPro13,3, kernel
`7.2.3-arch1-3`, core/DKMS `0.1.9.r8.g995f306-1`, found one T1 in configuration
2, runtime control `on`, runtime status `active`, and zero accumulated runtime
suspended milliseconds after approximately 24 hours of host uptime. No power
setting was changed. These are residency observations, **not watts or an idle
power baseline**, and not acceptance of the newly published v0.1.10 packages.

## Measurement before policy changes

Record the following in one attended, stable session before attempting any
power transition. Discover the T1 by USB vendor/product and traverse its
actual sysfs parent; do not use a saved bus path or interface name.

| Measurement | Required control or interpretation |
| --- | --- |
| Model, kernel, exact packages and loaded modules | Verify installed and loaded versions match. Keep firmware identity and bus paths private. |
| Display brightness, AC/battery state, radio state and background workload | Hold conditions fixed across comparison windows. Do not attribute a whole-host power change to T1 alone. |
| Ten-minute idle window, repeated three times | Record average platform power and variation using an existing trusted meter or battery-energy sampling. Record measurement resolution; unsupported metering stays unknown. |
| Runtime active/suspended counter deltas and control policy | Read only. Residency counters are not energy measurements. |
| Active owners | Record presence of renderer, camera stream, xART/network traffic and SEP operation/relay without process arguments or private payloads. Idle desktop appearance is not proof that every consumer is idle. |

Then define the lifecycle contract before changing the guard: every active
consumer must hold device use, a single owner must coordinate idle entry, and
wake must restore display/input, camera, network/xART, Touch ID and ALS within
recorded bounds. A failed transition must return to the stable policy without
resetting protected state. Define cancellation and disconnect behavior as well
as the normal path; do not add a polling daemon or privileged socket merely to
observe these existing states.

Only an attended candidate with that contract can supply the second half of
the power comparison. Whole-system sleep remains a separate dependency in
[#18](https://github.com/standardagents/t1bridge/issues/18). No runtime power
saving or measured battery benefit is claimed by this investigation.
