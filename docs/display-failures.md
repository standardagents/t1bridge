# Separating display discovery, replies and illumination

Investigation for [#31](https://github.com/standardagents/t1bridge/issues/31),
[#34](https://github.com/standardagents/t1bridge/issues/34) and the display part
of [#18](https://github.com/standardagents/t1bridge/issues/18).

| Observation | Established boundary | Still unknown |
| --- | --- | --- |
| #31: information request times out before DRM registration | No accepted `GINF` response arrived within the existing read timeout. | Display firmware, transport and physical panel health are not distinguished by `-110`. Darkness in Internet Recovery alone does not distinguish them either. |
| #34: DRM ready, renderer frame held, panel dark from boot | Discovery and userspace submission occurred. | Whether an accepted `UDCL` reply arrived during the observed update, and whether the panel emitted light. |
| #18: rearm succeeds after S3 but panel stays dark | The failure-only reprobe path does not run. | Which configuration-2 firmware/display state prevents relighting. |

The 13,3 boot failure and 14,2 suspend failure have different triggers and
reported recovery. Keep their results separate. A report-3 awake readback is
not an illumination measurement; accepted unpark writes did not relight the
reported panels. The next experiment is observation, not another reset or
speculative power write.

## Opt-in reply evidence

The following diagnostic records were added **after v0.1.10** and require a
driver containing the new source change. They are not present in v0.1.10.
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
