# Shareable diagnostics

Requires T1Bridge **0.1.1 or newer**. The original 0.1.0 packages do not support
this switch or format.

Diagnostics are **off by default**. Enable them to investigate import, Touch ID,
boot-service, or Touch Bar failures without removing redaction. Logging observes
existing work; it adds no sensor commands, retries, unlock attempts, or resets.

## Enable

For one CLI command, put `--diagnostics` before the command:

```bash
sudo t1bridge --diagnostics machine-data import
```

This still performs the requested import; the flag is not a dry run. It only
enables the current process, not an already-running broker or renderer.

For services, create `/etc/t1bridge/diagnostics.conf` as root with mode `0644`
(create its parent directory with mode `0755` if absent):

```ini
T1BRIDGE_DIAGNOSTICS=1
```

All packaged T1Bridge services, including the user renderer, read this optional
file on their next start. For a fingerprint reproduction, wait until no
authentication or enrollment is running, then:

```bash
sudo systemctl restart t1-touchid-auth.service
```

For renderer/provider failures, use `systemctl --user restart t1-touchbar.service`.
For boot failures, leave the setting enabled for the next planned reboot.
Do **not** restart NCM, xART, or the keybag relay just to turn on logging: doing so
changes the live state being investigated. Keep password access available.

Enabling broker logging does not enable an already-running xART process. The
status row must treat missing or capped xART records as incomplete evidence,
not a firewall failure. Successful enrollment does not require an admission
log to exist; do not change a working firewall rule just to obtain that log.

The broker also accepts `--diagnostics` in its service command. Do not launch a
second broker manually. Direct development commands can use the environment
variable `T1BRIDGE_DIAGNOSTICS=1` instead.

## Reproduce and export

Record the start time in your normal shell, then reproduce once through the
usual UI or command:

```bash
diagnostic_start=$(date --iso-8601=seconds)
```

Do not delete a working print or reset state to make room for a diagnostic.
After the attempt, export **only diagnostic records**, without journal headers:

```bash
sudo journalctl -b --since "$diagnostic_start" --no-pager -o cat \
  --grep '^t1bridge-diagnostic ' > t1bridge-diagnostic.txt
pacman -Q t1bridge t1bridge-dkms libfprint-t1bridge fprintd-t1bridge
```

For boot failures, omit `--since "$diagnostic_start"`. For a CLI import, redirect
stderr to a local file and share only lines starting `t1bridge-diagnostic `.

Inspect the report before posting. Share it, the package versions, and what you
saw (prompt, touch progress, final result). Do not attach full journals, EFI
backups, or state directories. An empty report is not success: check that the
new binaries and setting are active. Locally overridden development binaries
can differ from the package manager's version.

## Sustained keybag relay restarts

Authentication normally stops the shared relay, uses the exclusive SEP lease,
then restores the relay. `phase=relay result=ok` records a clean relay **exit**,
not startup readiness. Those clean handoffs do not increment systemd's failure
restart counter. Inspect the current state without restarting anything:

```bash
systemctl show t1bridge-keybag.service \
  -p ActiveState -p SubState -p NRestarts -p Result -p RestartUSecNext
```

For [issue #14](https://github.com/standardagents/t1bridge/issues/14), report
whether `NRestarts` is increasing, the first failing stage, the installed
package versions, and whether authentication or lid handling was active.
Use existing evidence; do not trigger a lid/suspend test or reset protected
state. The relay can fail without lid activity. A rapid lock-screen retry
loop is a separate consumer behavior, not proof that each attempt reached SEP.

Interpret `component=sep phase=sep-lease` codes as T1Bridge operation results:
`-103` means cancellation, `-111` means keybag validation/protocol failure,
and `1` means a remote operation error. They are not Linux errno values or
proof of a particular underlying SEP fault. Structured records require the
[diagnostic setting](#enable); their absence with logging disabled is expected.

The source service now backs off automatic failure restarts from 2 seconds to
60 seconds over five exponential steps (systemd 254 or newer). This preserves
the initial boot retry and reduces a persistent storm; it does not repair the
underlying keybag failure or rate-limit downstream lock-screen requests. An
installed package may still use the earlier fixed two-second policy. Recovery
on affected hardware remains unverified.

T1Bridge v0.1.7 also records each relay keystore reply when diagnostics are
enabled. `phase=keystore-reply` carries the parser result (0 success, 1 remote
rejection, negative parser error). A remote rejection adds `keystore-outer`
and, only if the outer status is zero, `keystore-inner` records with the actual
remote status. `selector` is an allowlisted operation: `0x03` loads a keybag,
`0x0d` promotes it, `0x04` queries lock state, `0x23` gets configuration,
`0x18` unlocks, and `0x19` queries device state. It is not a handle or identity.

These records distinguish which operation within a broad stage failed. Keep
the first rejected reply and its immediately preceding successful replies,
plus the final `sep-lease` result. Expected not-found replies can occur during
initial setup; a rejected reply alone does not establish a failed lease.
No new requests, retries or resets are added. The observer runs only during the
relay's synchronous native call and shares the process-wide 4096-record limit.
This additional evidence is not present in v0.1.6 or earlier packages.

Development source after v0.1.9 adds `component=sep phase=sep-cleanup` for the
relay and broker's native operation owners. Code `0` means cleanup of resources
owned by that operation reported no error; it can also mean no native resource
had been acquired. Code `-109` means ACM deletion, session destruction, descriptor
closure or lock closure reported a teardown failure. The record appears after
cleanup, independently of the final operation result. A primary remote error
such as `1` remains the return value even if cleanup also fails; successful or
cancelled operations still return `-109` when teardown fails.

Keep this cleanup record beside the failing keystore reply and final lease
result. It closes a diagnostic gap where an earlier failure could hide teardown
failure. It does not expose handles, identify a resource leak, prove that firmware
released transient keybag state, or fix the sustained restart loop. The observer
adds no exchanges or retries, is disabled by default, and shares the existing
record limit. These records are not present in the v0.1.9 package.

## Coverage and format

| Component | Recorded boundaries |
| --- | --- |
| Importer/EFI | Live hardware association, EFI discovery/open/read/parse, record selection, durable commit, final error category |
| SEP/broker | Relay/service lifecycle, SEP lease return codes, standard list/enroll/verify/identify/delete, existing Mesa commands and native rejection codes |
| NCM/xART | Link preparation, listener bind/accept failures, peer admission, storage-session success/failure |
| Touch Bar hardware | DRM/input/keyboard/session startup, renderer admission and session revocation |
| Renderer/provider | Renderer connection lifetime, provider availability changes and action success/failure |

T1Bridge v0.1.4 also traces OLED cancellation in the renderer:
`overlay` begin/ok means a visible state was entered/cleared; `overlay-press`
ok/error means a new contact was eligible/ineligible for cancellation;
`touchid-cancel` begin means a send was attempted, ok means the hardware service
acknowledged it, and error means sending failed or the service rejected it.
These records do not prove the final fprintd result. No `overlay-press` record
does not identify which earlier input boundary failed: the service reads hidraw,
not evdev, and a held contact does not generate another press. Do not use
absence of `evtest` events as proof of missing Touch Bar input.

**Cancellation can print `verify-no-match`.** fprintd 1.94.5 maps
`G_IO_ERROR_CANCELLED` to that same CLI result. It does not establish a failed
scan or broker outcome 4. Check the cancellation acknowledgment, overlay
dismissal and operation completion together; the broker gives accepted
cancellation precedence over a late match result.

Example:

```text
t1bridge-diagnostic v=1 component=broker phase=transaction result=error code=1 command=0x03
```

Labels come from fixed enums. `code` is a native command status or adapter error
category; interpret it with its component and phase. `command` is an allowlisted
Mesa command code, never a request header identifier. Optional `selector`
identifies a supported keystore operation, not a Mesa command. A stage's `ok` means that
call returned successfully, not that a fingerprint matched or enrollment
completed. Keep the final fprintd result.

No usernames, finger labels, device identifiers, paths, coordinates, keypresses,
credentials, calibration, keybags, or biometric payloads are included. Responses
are not dumped and errors are not formatted with unrestricted `Debug`. Each
process emits at most 4096 records plus a limit marker; each fingerprint
transport trace is capped at 256 commands. Work continues after either limit.

This covers userspace lifecycle/error boundaries and native adapter statuses,
not every internal branch. It does not enable kernel dynamic debug, firmware
tracing, USB capture, or logging in third-party fprintd/PAM/desktop applications.
Those outputs are not part of this public-shareable format.

## Disable

Set `T1BRIDGE_DIAGNOSTICS=0` in the same file. Restart only affected services while
idle, or let the change take effect at the next planned reboot. Remove a broker
`--diagnostics` override separately if you added one; preserve other local
overrides. No diagnostic state or fingerprint data needs deleting.
