# Interfaces

This document fixes the v1 interfaces that cross ownership boundaries.
Implementations may change internally without changing these contracts.

## Touch Bar hardware IPC v1

### Boundary and endpoint

`t1-touchbar-hw` listens on the Unix `SOCK_SEQPACKET` socket
`/run/t1bridge/touchbar.sock`. The socket is owned by root and the dedicated
`t1bridge` group with mode `0660`.

Socket permissions are only the first check. On every connection the service:

1. reads kernel-supplied `SO_PEERCRED`; it never trusts a PID, UID, or session
   supplied in a message;
2. verifies that the non-root peer UID belongs to `t1bridge` and equals the UID
   of the active, local, non-remote graphical session on systemd's canonical
   primary seat, `seat0`;
3. permits one active rendering client for the built-in Touch Bar; and
4. binds all buffers and actions to that connection's lifetime.

The DRM card remains assigned to the noninteractive `seat-touchbar` isolation
seat so the desktop compositor does not claim it. That dummy seat is never an
authorization identity. Admission rechecks the active session before it
completes. A seat transition or loss revokes the old client and releases its
buffers and synthesized keys before another client may be admitted. Root may
run a separately marked development client, but the packaged user service does
not run as root.

### Packet format

One seqpacket contains one control message. All integers are little-endian.
Every packet begins with this 16-byte header:

| Offset | Field | Meaning |
| --- | --- | --- |
| 0 | four bytes | ASCII `T1HW` |
| 4 | `u16` | protocol major, `1` |
| 6 | `u16` | message type |
| 8 | `u32` | payload length |
| 12 | `u32` | request identifier, zero for unsolicited events |

Control packets are limited to 64 KiB and must have exactly the declared
length. Unknown major versions, malformed lengths, extra file descriptors, or
out-of-range fields close the connection. Unknown message types receive an
`Unsupported` error and do not change service state.

The client starts with `Hello { minor, feature_bits }`. The service responds
with `HelloAck { minor, width, height, pixel_format, max_buffers }`. v1 uses
`XRGB8888`, the dimensions reported by DRM, at most three registered buffers,
and the lowest mutually supported minor version. No other message is valid
before this exchange.

### Minor-zero wire values

Minor zero assigns these message types. All other values are unknown; a
message sent in the wrong direction closes the connection.

| Value | Direction | Message |
| --- | --- | --- |
| `0x0001` | client to service | `Hello` |
| `0x0002` | client to service | `RegisterBuffer` |
| `0x0003` | client to service | `SubmitFrame` |
| `0x0004` | client to service | `TapKeys` |
| `0x0005` | client to service | `SetDisplayBrightness` |
| `0x0006` | client to service | `SetKeyboardBacklight` |
| `0x0007` | client to service | `CancelTouchId` |
| `0x8001` | service to client | `HelloAck` |
| `0x8002` | service to client | `Ack` |
| `0x8003` | service to client | `Error` |
| `0x9001` | service to client | `FrameReleased` |
| `0x9002` | service to client | `InputFrame` |

Payloads are packed byte layouts with no native padding. Reserved bytes must
be zero.

| Message | Exact payload |
| --- | --- |
| `Hello` | 12 bytes: `minor:u16`, `reserved:u16`, `required_features:u64` |
| `HelloAck` | 20 bytes: `minor:u16`, `reserved:u16`, `width:u32`, `height:u32`, `pixel_format:u32`, `max_buffers:u32` |
| `RegisterBuffer` | 16 bytes plus exactly one descriptor: `buffer_id:u32`, `stride:u32`, `byte_length:u64` |
| `SubmitFrame` | `buffer_id:u32`, `frame_id:u64`, `damage_count:u32`, then exactly that many 16-byte `{x:u32,y:u32,width:u32,height:u32}` rectangles |
| `TapKeys` | 12 bytes: `count:u8`, three reserved bytes, then four `u16` Linux key-code slots; unused slots are zero and duplicates are invalid |
| `SetDisplayBrightness` | one `u8` percentage from zero through 100 |
| `SetKeyboardBacklight` | one `u8` percentage from zero through 100 |
| `CancelTouchId` | empty |
| `Ack` | empty |
| `Error` | one `u32` error code |
| `FrameReleased` | 12 bytes: `buffer_id:u32`, `frame_id:u64` |
| `InputFrame` | `monotonic_ns:u64`, `fn_pressed:u8`, `contact_count:u8`, `reserved:u16`, then exactly that many 12-byte contacts `{id:u8,tip:u8,in_range:u8,reserved:u8,x:u32,y:u32}` |

### Minor-one additions

Minor one retains every minor-zero message and adds two relative actions:

| Value | Direction | Message | Exact payload |
| --- | --- | --- | --- |
| `0x0008` | client to service | `StepDisplayBrightness` | one direction byte: zero down, one up |
| `0x0009` | client to service | `StepKeyboardBacklight` | one direction byte: zero down, one up |

A minor-zero service never receives these messages. A renderer enables the
corresponding stock buttons only when the negotiated minor is at least one and
the existing hardware feature bit is present.

Boolean bytes are exactly zero or one. `buffer_id` and `frame_id` are nonzero.
Damage count is at most 64; contact count is at most ten; active contact IDs
are unique and at most 15. Coordinates are bounded to the negotiated display.

Feature bits are `0x01` sealed memfd frames, `0x02` input frames, `0x04` typed
key taps, `0x08` display brightness, `0x10` keyboard backlight, and `0x20`
Touch ID cancellation. The service always accepts the base `0x27` set and
accepts each brightness bit independently only when its Linux hardware class
has one usable, unambiguous device. It either accepts every required client bit
or returns `UnsupportedFeature` and closes. A client that wants optional
controls may probe them with separate `Hello` connections, then require the
accepted set on its final renderer connection. Pixel format `1` is Linux DRM
little-endian `XRGB8888`, stored as B, G, R, ignored-X bytes in memory.

Error codes are `1` unsupported message, `2` unsupported feature, `3` resource
limit, `4` invalid buffer, `5` unknown buffer, `6` buffer busy, `7` action
denied, `8` device unavailable, `9` I/O failure, and `10` internal failure.
Errors contain no text, errno, path, identifier, or echoed payload.

Every request has a nonzero identifier that remains unique until its terminal
response. `HelloAck`, `Ack`, and `Error` echo it. Unsolicited events use zero.
`SubmitFrame` first receives `Ack`, which transfers ownership; its later
`FrameReleased` event returns the buffer. Reuse after a terminal response is
allowed. A zero or duplicate outstanding request ID, invalid reserved value,
invalid boolean/count/range, wrong-state message, ancillary truncation, or
descriptor-count mismatch closes the connection. A well-formed unknown message
receives error 1 and leaves state unchanged.

### Frame buffers

The unprivileged process renders complete frames. Scene configuration and user
actions never enter the root process.

`RegisterBuffer` carries `{ buffer_id, byte_length, stride }` and exactly one
`memfd` via `SCM_RIGHTS`. The client must seal the file against shrinking,
growing, and further seal changes. The service requires:

- a unique buffer identifier;
- `stride == width * 4` and `byte_length == stride * height`;
- a regular anonymous file of exactly that size; and
- a read-only mapping on the root side.

`RegisterBuffer` is the only message that carries ancillary data. Reception
uses close-on-exec semantics and rejects control truncation, ancillary data
other than exactly one `SCM_RIGHTS` descriptor, or any descriptor-count
mismatch. Every received descriptor is closed on rejection and disconnect.
The root side snapshots submitted pixels from its read-only mapping before DRM
presentation; it never exposes externally mutable shared memory as a Rust
shared reference.

`SubmitFrame` carries `{ buffer_id, frame_id, damage[] }`. Each damage rectangle
is `{ x, y, width, height }` in display coordinates; v1 allows at most 64 and
rejects any rectangle outside the negotiated dimensions. An empty list means
the whole frame. The service owns the submitted buffer until it emits
`FrameReleased { buffer_id, frame_id }`; the client must not write it during
that interval.

The root service copies the complete first frame for each client connection,
then copies only the validated damage regions to DRM while handling physical
orientation. Pixel values cannot select files, commands, devices, or memory
offsets. Frames may be rate-limited or dropped, with an explicit release, when
newer work supersedes them.

### Input events

The root service sends `InputFrame` with a monotonic timestamp, current Fn
state, and zero or more contacts. A contact contains only `{ id, x, y, tip,
in_range }` in negotiated display coordinates. Contact count and coordinates
are bounded by the HID descriptor and panel dimensions. Vendor bytes and raw
device paths never cross the IPC boundary.

When the hardware stops sending reports after the last lift, the service emits
an empty input frame so the user process can release all contacts. Fn changes
are also emitted immediately and do not wait for a touch report.

### Typed privileged actions

The user process may request only these root operations:

| Operation | Payload and limits |
| --- | --- |
| `TapKeys` | one to four enumerated Linux input key codes; pressed then released atomically. Minor zero allows only Esc (`1`), F1 through F10 (`59` through `68`), F11 (`87`), and F12 (`88`). |
| `SetDisplayBrightness` | absolute integer from 0 through 100 |
| `SetKeyboardBacklight` | absolute integer from 0 through 100 |
| `StepDisplayBrightness` | minor one: direction down or up |
| `StepKeyboardBacklight` | minor one: direction down or up |
| `CancelTouchId` | no payload; uses the cancellation protocol below |

The service validates every value, rate-limits requests, and returns `Ack` or a
typed `Error`. It accepts no command string, executable path, environment,
shell fragment, arbitrary input event, arbitrary device node, or arbitrary
file path. On disconnect it releases any synthesized keys, unmaps all buffers,
and clears queued actions.

The service implements `TapKeys` and `CancelTouchId` unconditionally. It
dynamically discovers display and keyboard-backlight classes without packaged
device names or paths, advertises each independently, scales the bounded
percentage against the kernel maximum, and rate-limits updates. The scaled
value is sent through logind's exact admitted graphical-session brightness
method with a 250 ms per-call deadline; the service sandbox keeps sysfs
read-only. For a relative minor-one action, the service reads the live value
and moves it by one sixteenth of the device range with endpoint clamping; the
renderer receives no sysfs path and needs no cached starting level. An absent,
ambiguous, or failed class remains unsupported without affecting the other
class or the Touch Bar session.

Audio, media, theme, desktop, and user-configured command actions never enter
the root hardware IPC. The optional desktop-provider contract below remains
entirely in the unprivileged process.

## Touch ID overlay and cancellation protocols

The base v1 protocol is hardware-proven. The additive v2 enrollment record
exposes cosmetic progress already reported by the standard fingerprint path.

### Cosmetic state file

The producer atomically replaces `/run/t1bridge/touch-id-state.json` with one
newline-terminated JSON object:

```json
{"version":2,"pid":4242,"state":"enrollment","progress":60}
```

The schema has a fixed field set selected by `version`:

| Field | Type | Rule |
| --- | --- | --- |
| `version` | integer | `1` for the three-field base record; `2` for enrollment progress |
| `pid` | positive integer | producer process must still exist |
| `state` | string | one of `enrollment`, `authenticate`, `approve`, `retry`, `success` |
| `progress` | integer | v2 only; `0` through `100`, and only with `enrollment` |

v1 records retain exactly the first three fields. v2 records retain exactly
all four fields and are valid only for enrollment. Readers accept both
versions, so authentication states and existing v1 producers remain valid.

The producer writes a unique temporary file in the same directory, sets mode
`0644`, synchronizes it, renames it over the state path, and synchronizes the
directory. Before starting, it refuses to replace state owned by another live
producer PID. On cleanup it removes the file only if the PID still matches
itself.

The Touch Bar reader treats a missing file or dead producer as no overlay.
Malformed JSON, unknown fields, unknown versions, or unknown states produce a
diagnostic and no overlay; they never affect biometric success. Enrollment
progress is cosmetic: the built-in renderer animates a blue bottom-edge line
from its current displayed value to each broker-reported percentage. It never
changes enrollment authority or outcome.

The overlay is cosmetic. Only the broker's cryptographic result can satisfy an
authentication request.

### Broker cancellation socket

The root authentication broker listens on the Unix `SOCK_SEQPACKET` socket
`/run/t1-touchid/auth.sock`. A cancellation request is exactly these eight
bytes:

```text
T1CNCL\x01\n
```

An accepted cancellation returns exactly `OKAY`. Rejection returns `DENY`.
Requests with another length or value are rejected. The client uses a 500 ms
total connect/send/receive deadline, verifies that the path is a socket owned
by root, and verifies the connected peer credentials are root.

The broker accepts cancellation only from root and only while one operation is
active. With the privilege split, the unprivileged UI sends the typed
`CancelTouchId` hardware-IPC request; `t1-touchbar-hw` performs this fixed
broker exchange. No user-configured action can send arbitrary broker bytes.

Cancellation acknowledgement means the cancellation event was delivered. It
does not mean authentication succeeded, and a race with natural completion may
legitimately return `DENY`.

## Standard fingerprint broker IPC v1

The separately packaged libfprint driver connects to root-owned mode `0600`
Unix `SOCK_SEQPACKET` socket `/run/t1bridge/fingerprint.sock`. The broker
requires kernel `SO_PEERCRED` UID 0 before reading a packet. The filesystem and
peer checks make this an fprintd integration boundary, not a general user API.
It shares the direct broker's one scheduler, worker slot, cancellation
authority, SEP lifecycle, persistence, and Touch ID presentation.

Each packet is at most 1,024 bytes and starts with this 12-byte, big-endian
header: ASCII `T1FP`, version `u8=1`, message type `u8`, reserved `u16=0`,
payload length `u16`, reserved `u16=0`. The declared payload must consume the
packet exactly. Account names are one length byte followed by 1--255 non-NUL
UTF-8 bytes; the broker resolves the exact canonical non-root account through
NSS. Identity values are opaque nonzero 16-byte Mesa identifiers. Finger
labels are the standard left-thumb through right-little values 1--10.

| Value | Direction | Message and payload |
| --- | --- | --- |
| `0x01` | client to broker | capabilities, empty |
| `0x02` | client to broker | open, empty |
| `0x03` | client to broker | list, empty |
| `0x04` | client to broker | enroll, account then finger label |
| `0x05` | client to broker | verify, account then identity |
| `0x06` | client to broker | identify, account |
| `0x07` | client to broker | delete, account then identity |
| `0x08` | client to broker | cancel this connection's active operation, empty |
| `0x81` | broker to client | capabilities: operation bits `u16`, enrollment stages `u8`, identity capacity `u8` |
| `0x82` | broker to client | opened, empty |
| `0x83` | broker to client | identities: count, then for a nonzero count one account and repeated identity/finger pairs |
| `0x84` | broker to client | enrollment progress: completed stage `u8`, total stages `u8` |
| `0x85` | broker to client | terminal outcome byte, followed by an identity only for enrolled or matched |

Capability bits 0--5 are List, Enroll, Verify, Identify, Delete, and Cancel.
The broker advertises at most 100 enrollment stages and Mesa's five-identity
representation capacity. The
terminal outcomes are completed `1`, enrolled `2`, matched `3`, no-match `4`,
cancelled `5`, device-lost `6`, busy `7`, second-owner `8`, unsupported `9`,
error `10`, duplicate `11`, and capacity-full `12`. Only enrollment progress is
nonterminal. T1 enrollment is limited to three identities for the active user.
The wire, metadata, and catalog accept Mesa's full five-identity representation
so an existing larger set can still be listed, verified, and reduced.

One connection may have one active operation. Another request is Busy; Cancel
targets only that exact operation and waits for its authoritative completion.
Malformed framing, unknown values, invalid accounts, duplicate identities,
cross-owner enrollment, stale completion, transport loss, and device loss fail
without becoming an authentication success. No packet carries a numeric UID,
path, command, SEP payload, biometric template, or machine identifier.

## Renderer selection v1

For an installable example, see the separate Standard Agents
[touchbar-doom demo](https://github.com/standardagents/touchbar-doom).
It is an optional Omarchy package that exercises frame submission, touch input,
and explicit renderer selection. Its leftmost Quit button restores the prior
renderer. T1Bridge does not install or select the demo automatically.

The package always includes an unprivileged built-in renderer. A user may
replace it by creating an executable file or symlink at
`${XDG_CONFIG_HOME}/t1bridge/renderer`; the target may live anywhere the user
can execute it. The launcher invokes the selected path without arguments and
does not interpret, rewrite, register, or migrate the renderer's own
configuration or assets.

If the selected path cannot be executed or the selected process exits, the
launcher emits one best-effort diagnostic and starts the built-in renderer.
It does not retry or rewrite the selection. If supervision fails without
proving that the selected process exited, the launcher starts no second
renderer.

Every renderer runs as the logged-in user and uses the hardware IPC contract
above to negotiate dimensions, submit complete pixel frames, receive bounded
input, and request only advertised typed actions. A renderer owns its layout,
themes, assets, desktop integration, media controls, and other user-visible
policy. Touch ID presentation is cosmetic: a renderer may theme the documented
states, but only the authentication broker can produce an authentication
result. No renderer receives raw device access, biometric material, root
state, arbitrary privileged commands, or an in-process plugin ABI.

## Desktop provider v1

Desktop integration is an optional unprivileged executable contract. The user
service reads `T1BRIDGE_DESKTOP_PROVIDER`; an empty, relative, non-file, or
non-executable value disables the provider. The root service never reads this
setting. The user process invokes the absolute path directly, with no shell and
no caller-supplied argument or environment interpolation.

Every invocation begins with the arguments `v1` and one fixed operation:

| Operation | Remaining arguments | Purpose |
| --- | --- | --- |
| `status` | none | Return current capabilities and state. |
| `set-volume` | one decimal integer from `0` through `100` | Set output volume. |
| `toggle-mute` | none | Toggle output mute. |
| `media-previous` | none | Select the previous playable item. |
| `media-play-pause` | none | Toggle playback. |
| `media-next` | none | Select the next playable item. |
| `show-display-brightness` | none | Show desktop feedback for the current display brightness. |
| `show-keyboard-backlight` | none | Show desktop feedback for the current keyboard-backlight level. |
| `notify-renderer-fallback` | `selection-unavailable` or `selection-exited` | Report the one automatic fallback event. |

`status` writes one ASCII record with five whitespace-separated fields:

```text
T1BRIDGE-DESKTOP 1 CAPABILITIES VOLUME MUTED
```

`CAPABILITIES` is an unsigned decimal bit set: `1` audio, `2` media player
available, `4` desktop notification, and `8` level feedback. Unknown bits reject the record. With
audio advertised, `VOLUME` is `0` through `100` and `MUTED` is `0` or `1`.
Without audio, both fields must be `-`. Media and notification have no extra
state in minor zero.

Provider stdout is limited to 128 bytes and each process receives a 500 ms
monotonic deadline. A status record is refreshed at most once per second.
Actions return exit status zero for accepted delivery; their output is ignored.
Malformed, oversized, timed-out, signaled, nonzero, missing, or disappearing
providers become unavailable without stopping or restarting the renderer.
Only provider-owned controls are hidden. Action dispatch is asynchronous and
bounded so a slow provider cannot stall display, input, brightness, Fn/F-keys,
or Touch ID presentation. Consecutive pending absolute-volume positions are
coalesced to the newest value so a drag cannot build a stale action backlog
without reordering mute or media actions.

The contract contains no arbitrary command action, provider discovery,
manifest, registry, socket, or persistent-provider requirement. T1Bridge ships
no desktop implementation. A distribution may install an executable and set
the service environment; a replacement renderer may use or ignore it.
