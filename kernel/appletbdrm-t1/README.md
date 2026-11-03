# T1 appletbdrm override

This is Linux stable v7.1.8's `appletbdrm` driver adapted for the T1 USB ID,
response flow, and suspend/resume behavior. It deliberately keeps the in-tree
module name and installs under `updates/dkms`, taking precedence over the
in-tree module.

The legacy `apple-ibridge-no-config1-revert.patch` is unnecessary because
T1Bridge does not install the config-1 `apple-ib-drv` stack it modifies.

## Upstream status

The three-patch series in [`upstream/`](upstream/) is a private review draft and
has not been submitted. It is based on kernel.org stable tag `v7.1.8` (tag
object `1af9bbf117b680f903a81485cd74501c334bf538`, commit
`25c76bea853d0db65b51fb4697a47cbfd9e35e76`). Phase 1 hardware acceptance is
complete; the submission base's suspend/resume row is still recorded as not run. The
system-sleep path of this override is hardware-tested on one machine, as recorded in the
section below. Owner DCO sign-off and validation against the eventual submission base
remain required before sending the series upstream.

## System sleep

Before a system suspend the driver evaluates the T1's ACPI `SOCW` method with argument 0
(host sleep) and after wake with argument 1 (host awake); the T1 then powers its own
display off and back on. `SOCW` returns 0 when the T1 acknowledged; any other result is
logged. It is discovered at probe, looked up only for a device the driver has identified
as a T1, from the USB device's bus-controller parent chain to the first ACPI node, and
kept only when that namespace has exactly one single-argument `SOCW` method; on that
MacBookPro14,3 the unique single-argument `SOCW` method sits below the USB host
controller's ACPI node. It is never evaluated at probe: only the system-sleep notifier
evaluates it, from the `PM_SUSPEND_PREPARE` and `PM_POST_SUSPEND` pair, which is how
s2idle and suspend to RAM reach the notifier; hibernation uses other notifier events, and
the USB callbacks never evaluate it.

The discovered handle and the state of the sleep in progress are module-wide, not per
interface. A T1 disconnected during sleep has no resume callback, and the device that
enumerates in its place would start with nothing remembered, the situation that leaves the
display dark in the awake-host observation. The handle is kept for the module and is used
only from the notifier; a namespace node is not removed by this driver. The module holds a
reference to itself for the duration of a sleep, from the sleep write to the wake write, so
the wake write answers a sleep this module began while the module stays loaded; a sleep
whose reference cannot be taken is not answered.

When the sleep write is acknowledged, the USB suspend callback does not park, because the
panel is already powered off; for any other event, including runtime autosuspend, or for a
write the T1 did not acknowledge, park is the sleep path as before. The prepare broadcast
is delivered before the device suspend callbacks, so the state it records is what this
decision sees; the post broadcast is delivered after they complete. A write that fails or
is not acknowledged is logged once with its value. `PM_POST_SUSPEND` also follows a failed
or aborted suspend, so the awake write can answer a sleep that never happened; it is still
required, because skipping it would leave the display off.

When PM is enabled, module load can now fail if the PM notifier cannot be registered;
under `!CONFIG_PM` the registration is a stub that always succeeds and no suspend callback
runs, where previously only the driver registration could. SOCW is a blocking ACPICA
evaluation, so it runs in process context from the notifier: the sleep write adds its own
latency to the prepare phase of every system sleep, cannot be bounded by the driver, and
runs before the kernel checks for pending wakeups, so a wake event during it is not
honoured until it returns. Sending 0 and then 1 on an awake host, with no real sleep
between them, leaves the display dark until a full power-off, which is why probe,
userspace and runtime autosuspend do not evaluate it.

Hardware-tested on one MacBookPro14,3, Linux 7.2.15, s2idle for 20 s, lid open, with this
driver code and no userspace `SOCW`: no `SOCW` warning was logged during the sleep (the
driver warns on any non-zero result), report 3 read back awake after resume, which is
consistent with the park having been skipped, and the strip was seen dark with no strobe
during the sleep and fully lit after resume (operator observation). Earlier observations:
the same sequence sent from a userspace sleep hook behaved the same way, and the strobing
state itself was reproduced without any sleep by writing report 3 = 1 on an awake host, on
kernels 7.2.8 and 7.2.15.
Not hardware-tested: S3, other models, and re-enumeration across a sleep.
See
[display failures](../../docs/display-failures.md#illumination-during-system-sleep).

## Failed resume recovery

The T1 resume callback checks both endpoint clear-halt operations and every
display rearm step before restoring the saved DRM mode. A rearm or mode-restore
failure returns its error and marks the display interface for USB core's
deferred unbind/reprobe at system PM completion. Returning an error alone does
not request that recovery. A successful resume retains the existing DRM device;
it need not produce a new userspace `display-open` diagnostic.

The fallback reprobes only the display interface. It does not reset the whole
T1 or change configuration, autosuspend policy, or protected state. Reprobe
recreates the DRM device so the packaged device-bound hardware service can
reopen it and renderers can reconnect. A failed probe leaves the device
unavailable rather than requesting an unbounded reset loop.

`make test sanitize` runs the production code through the headers it includes, against
synthetic USB, DRM, ACPI and PM boundaries. `test_appletbdrm_socw.c` covers the discovery
rule below a synthetic controller chain, the acknowledged, not-acknowledged and failed
sleep writes, the actions that answer nothing, the wake write and the state it clears, the
module reference held from the sleep write to the wake write, the sleep that is not
answered when the reference cannot be taken, the evaluation running with the guard not
held, the park decision for each event, a device that enumerates in the place of the one
that slept, and the module registration pair and its unwind. `test_appletbdrm_resume.c`
covers failures at every rearm step. The harness is single-threaded, so the guard and the
module reference are recorded as the calls the code makes, not as contention. This
establishes the sleep-path decisions, error propagation and the deferred-rebind request,
not physical recovery. Attended system-suspend validation remains in
[#18](https://github.com/standardagents/t1bridge/issues/18); follow the
[hardware runbook](../../docs/hardware-validation.md#suspend-and-resume).
