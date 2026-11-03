# Separating display discovery, replies and illumination

Investigation for [#31](https://github.com/standardagents/t1bridge/issues/31),
[#34](https://github.com/standardagents/t1bridge/issues/34) and the display part
of [#18](https://github.com/standardagents/t1bridge/issues/18).

| Observation | Established boundary | Still unknown |
| --- | --- | --- |
| #31: information request times out before DRM registration | No accepted `GINF` response arrived within the existing read timeout. | Display firmware, transport and physical panel health are not distinguished by `-110`. Darkness in Internet Recovery alone does not distinguish them either. |
| #34: DRM ready, renderer frame held, panel dark from boot | Discovery and userspace submission occurred. | Whether an accepted `UDCL` reply arrived during the observed update, and whether the panel emitted light. |
| #18: rearm succeeds after S3 but panel stays dark | The failure-only reprobe path does not run. | Which configuration-2 firmware/display state prevents relighting. |
| #18: park at sleep leaves the panel powered and the strip strobing | `SOCW(0)` before sleep and `SOCW(1)` after resume, from this driver without the park write, kept the strip dark and brought it back on one 14,3 under s2idle. | Other models, S3, and a re-enumeration across a sleep. |

The 13,3 boot failure and 14,2 suspend failure have different triggers and
reported recovery. Keep their results separate. A report-3 awake readback is
not an illumination measurement; accepted unpark writes did not relight the
reported panels. The next experiment is observation, not another reset or
speculative power write.

## Opt-in reply evidence

The following diagnostic records require **v0.1.11 or newer**. They are not
present in v0.1.10.
The running DKMS module must match the tested build; a source checkout or an
updated file on disk is not sufficient.

The existing USB response function has dynamic-debug records containing only
the expected reply kind, result, numeric error and read-attempt count. Logging
adds no requests, retries, resets, polling service or socket. Enable the call
sites for a short observation window using the kernel's dynamic-debug control:

```sh
printf '%s\n' 'module appletbdrm func appletbdrm_read_response +p' |
  sudo tee /proc/dynamic_debug/control >/dev/null
display_diagnostic_start=$(date --iso-8601=seconds)
```

First inspect the existing debug setting; retain it if these call sites were
already enabled. If the control file or call sites are absent, report that
limitation rather than unloading or reprobeing a live device to obtain logs.
Observe an ordinary UI update in an already-occurring dark state. Do not
manufacture a suspend or reset for this check. Extract the fixed records while
discarding the kernel's device-specific prefix:

```sh
sudo journalctl -b -k --since "$display_diagnostic_start" --no-pager -o cat |
  sed -n 's/^.*\(t1bridge-display v=1 .*\)$/\1/p'
```

If this observation enabled previously disabled call sites, restore them:

```sh
printf '%s\n' 'module appletbdrm func appletbdrm_read_response -p' |
  sudo tee /proc/dynamic_debug/control >/dev/null
```

Share only these records, model, kernel, package/build version, boot-versus-S3
trigger and the separately observed lit/dark state. No frame pixels, raw USB
payloads or complete journals are needed.

`kind=information result=ok` means the reply had the expected size and `GINF`
type; geometry validation follows it. `kind=update result=ok` means a
size/type-valid `UDCL` was received. **T1 replies are not timestamp-matched to
the current frame**, so even that record cannot prove that particular frame
was presented or that the panel illuminated. An empty log is inconclusive.
An error identifies the response boundary and the number of reads attempted;
it does not establish a panel-hardware diagnosis.

The protocol tests use the actual exchange functions with synthetic USB and
allocation boundaries. They cover timeouts, short transfers, stale responses,
bounded reads and cleanup. They also reproduce and fix an information-buffer
leak when sending the query fails. That leak fix does not resolve the reported
response timeout or dark-panel state.

## Illumination during system sleep

Park (HID feature report 3, byte 1 = 1) sets only the display brightness factor to
0 on the T1: the panel stays powered and keeps refreshing. On one MacBookPro14,3 the
right-hand area of the strip visibly strobed for the whole sleep, and the same state
is reproduced without any sleep by writing report 3 = 1 on an awake host, on kernels
7.2.8 and 7.2.15.

The legacy apple-ibridge driver evaluates `SOCW` with 0 in its suspend callback and
with 1 in its resume callback; on this machine `SOCW(0)` before a sleep and `SOCW(1)`
after resume left the strip dark and lit again. That driver's comments call the
arguments host sleep and host awake, and the T1 then powers its display off and back
on. On that MacBookPro14,3 the unique single-argument `SOCW` method sits below the USB
host controller's ACPI node, and the driver looks it up only for a device it has
identified as a T1. `SOCW` returns 0 when the T1 acknowledged; any other result is
logged as it arrives, and it is a blocking ACPICA evaluation, so it runs in process
context from the notifier.

This driver sends the pair from a system PM notifier, `PM_SUSPEND_PREPARE` and
`PM_POST_SUSPEND`, which is how s2idle and suspend to RAM reach the notifier, and keeps
the sleep state per module rather than per interface: an interface that is gone at wake
cannot answer for its sleep, and a device that enumerates in its place would otherwise
start with nothing remembered, the situation that leaves the display dark in the
awake-host observation. The module holds a reference to itself for the duration of the
sleep, from the sleep write to the wake write, so the wake write answers a sleep this
module began while the module stays loaded. The park write stays the fallback for a
device whose method was not discovered, for a write the T1 did not acknowledge, and for
an event that is not a plain system sleep.

Hardware-tested on that one 14,3, Linux 7.2.15, s2idle for 20 s, lid open. First from
userspace: `SOCW(0)` immediately before a real sleep and `SOCW(1)` immediately after
resume, with the park write still in place, left the strip fully dark with no strobe
during sleep and fully back after resume. Then with this driver code and no userspace
SOCW: no `SOCW` warning was logged during the sleep (the driver warns on any non-zero
result), report 3 read back awake after resume, which is consistent with the park having
been skipped, and the strip was seen dark with no strobe during the sleep and fully lit
after resume (operator observation).

Not hardware-tested: S3, other models, a device that enumerates in the place of the one
that slept, and the composition with the open unpark work in
[PR #47](https://github.com/standardagents/t1bridge/pull/47) and
[PR #49](https://github.com/standardagents/t1bridge/pull/49).

Sending `SOCW(0)` and later `SOCW(1)` on an awake host, with no real sleep between
them, left the display dark until a full power-off, so this driver evaluates `SOCW`
only from the system-sleep notifier, at `PM_SUSPEND_PREPARE` and `PM_POST_SUSPEND`,
and never at probe. Runtime autosuspend and hibernation are not system sleep, so
they do not evaluate it either.
