# Architecture

For a visual walkthrough, see [How Touch ID works](touch-id.md).

This document defines the v1 component boundaries, boot ordering, persistence
model, and install layout. Hardware protocol details belong in `t1-bridge`;
policy and desktop behavior do not.

## Trust boundaries

T1Bridge has four execution contexts:

| Context | Responsibility | Explicitly excluded |
| --- | --- | --- |
| Kernel | Select the T1 USB configuration and expose DRM and NCM devices | Authentication policy and user actions |
| Root services | SEP access, xART persistence, authentication brokering, DRM/hidraw/uinput access | User-configured command execution |
| User Touch Bar process | Rendering, cosmetic Touch ID state, and one optional desktop-provider client | Direct hardware, biometric-state, root-state, or desktop-specific implementation |
| PAM consumer | Request one typed authentication result and fall back to its existing password path | Hardware orchestration and persistent state |

Every root service and open socket must have a current hardware or
authentication requirement. The package does not run a general privileged
command service.

## Components

- `t1-cfgsel` selects the descriptor-validated T1 USB configuration during
  enumeration. Unexpected descriptors fall back to the kernel's normal choice.
- `apple_t1_ncm` exposes the private host-to-T1 network link.
- `t1-ncm-ready` validates that link, marks it up through rtnetlink, and waits
  boundedly for usable IPv6 link-local addressing before it exits.
- the T1 `appletbdrm` patch exposes the Touch Bar DRM device.
- `t1-xart-storage` persists one opaque anti-replay record for BridgeOS. It
  never interprets or logs record contents.
- `sep-probe` performs native AppleKeyStore/keybag operations and may hold the
  keybag notification relay open.
- the authentication broker owns biometric operations. Its direct development
  socket and root-only standard fingerprint socket share one scheduler and one
  hardware worker.
- the separately packaged libfprint driver translates standard Linux
  fingerprint operations onto that root-only socket. Shipped PAM integration
  is ordinary `pam_fprintd`; the direct PAM source remains rollback tooling.
- `t1-touchbar-hw` owns DRM, hidraw, Fn input, and uinput. It accepts only the
  typed IPC operations in `interfaces.md` and never executes configured
  commands.
- `t1-touchbar` runs as the logged-in user and owns renderer selection,
  rendering, and the bounded optional desktop-provider client. Distribution-
  specific audio, media, and notifications remain in the provider executable.
  The graphical user must belong to the package-created `t1bridge` group to
  reach the group-owned hardware socket; distributions grant that membership
  during hardware setup, and it takes effect at the user's next login.
- the root-only `t1bridge validate usb-cycle` command is an acceptance and
  recovery tool, not a daemon. It validates one complete T1 interface map,
  excludes concurrent cycle and SEP work, and coordinates one bounded
  configuration-selector unbind/rebind without resetting USB or changing power
  policy.
- the root-only `t1bridge validate usb-live-loss` command is the narrower
  active-operation acceptance tool. It requires a standard fingerprint
  handoff, leaves that stack running for removal, and does not rebind until the
  operation has released SEP.

## Boot and ordering contract

The required order is:

1. **Configuration selection.** `t1-cfgsel` chooses the validated USB
   configuration while the device enumerates. The display and NCM interfaces
   do not exist before this step succeeds.
2. **NCM link.** An early systemd link policy matches the T1 NCM driver, keeps
   its ephemeral kernel name instead of renaming it, and assigns the private
   interface to T1Bridge before a general network manager can claim it. The
   add-only udev rule then matches the imported T1 USB descriptor identity on
   that original first-activation event and requests the device-scoped service.
   A capability-limited oneshot proves its
   device-triggered instance names the independently driver- and USB-validated
   unique T1 interface, marks only that ephemeral kernel index up through
   rtnetlink, waits boundedly for usable IPv6 link-local addressing, and
   revalidates both identities before reporting readiness. No
   connection profile, identifier, fixed interface name, address, route,
   gateway, or DNS setting is packaged or retained.
3. **xART listener.** after NCM readiness, the xART service creates an
   IPv6-only listener restricted to the dynamically rediscovered T1 NCM
   interface and rejects any non-T1 peer. The short-lived NCM helper alone has
   `CAP_NET_ADMIN`; the network-facing xART process does not retain it. The
   device-scoped service reports readiness only after binding that listener,
   then starts the keybag relay; device removal propagates a relay stop.
4. **Keybag relay.** after xART storage and NCM readiness, `sep-probe` restores
   the saved keybag state, observes the promoted bag, resolves its persisted
   protected-user binding, publishes first-unlock, initializes ACM, confirms
   the biometric keybag's final device state, and holds the notification
   relay. The service is pulled in by the ready xART instance rather than
   enabled independently; an execution condition rejects any start without an
   active device-scoped xART instance, and its state-file condition keeps it
   inactive before enrollment. Missing or unusable state does not block
   password authentication.
5. **Authentication broker.** the local broker socket may be socket-activated,
   but an operation starts only after xART, NCM, and relay prerequisites are
   ready. Idle broker processes may exit while the socket remains available.

The Touch Bar hardware service starts after the DRM and input devices appear.
The user process may start later and reconnect without restarting hardware.
Missing or failed desktop providers affect only their audio, media,
notification, and display-power behavior. No provider failure blocks the hardware controls or
renderer reconnection.

## Guarded device lifecycle

The same-boot cycle command accepts no device argument. It dynamically requires
one T1 in configuration 2 with the expected camera, HID, DRM, NCM, and SEP
drivers. It snapshots and stops only the known dependent systemd units, then
holds an independent cycle lock and the ordinary exclusive SEP lock. The
configuration-selector driver is the sole unbind/rebind target; no fixed USB
topology, device node, or network-interface name crosses the boundary.

Validation has three explicit states. Before the cycle, the relay must own SEP
through `usbfs`. After services stop and while the SEP lock is held, interface 7
must be deliberately unclaimed. After rebind, the complete kernel interface map
must return while SEP remains quiesced; after releasing SEP and restoring the
prior service state, `usbfs` ownership must return. An interruption is observed
through a deferred signal flag so cleanup runs first. Any uncertain device or
service recovery is a hard, identifier-free failure.

The live-loss command shares the same device validation, cycle exclusion, and
selector-only mutation. It additionally requires fprintd and the broker active,
the relay inactive, and SEP already owned. Removal happens before this command
can acquire SEP. The command treats removal as complete only after both the
selector binding and every configuration interface are absent; successful SEP
acquisition is then the bounded proof that the in-flight operation released.
Rebind happens under SEP exclusion, followed by fixed steady-state service and
full interface-map recovery. Its failure path first stops the fixed stack and
performs the same bounded protected recovery as the ordinary cycle. Before the
steady stack restarts, it clears a failed broker unit left by a bounded shutdown
without forcing that socket-activated service to stay running.

Each standard fingerprint operation also monitors the exact dynamically
validated T1 NCM interface that existed at admission. Its disappearance or
replacement cancels one shared signal sampled by both SEP setup and the later
bridge/Mesa transport. The same cancellation owns a duplicated, redacted
BridgeXPC socket handle so it can wake an in-progress command read immediately
instead of waiting for that socket's normal deadline. The operation then
returns the existing typed `DeviceLost` result to libfprint. This is required
because selector unbind removes every configuration interface without
physically removing the root USB device, so GUsb root-device hotplug alone is
not a sufficient loss signal.

## SEP arbitration

The keybag relay holds a shared lifetime lock on
`/run/lock/t1-touchid-sep.lock`. Enrollment and authentication require
exclusive SEP access and use this sequence:

1. verify the relay is healthy;
2. arrange unconditional relay recovery;
3. stop the relay;
4. acquire the same lock exclusively with a bounded wait;
5. perform one biometric transaction; and
6. release the lock and restart the relay on success, failure, cancellation,
   timeout, or process teardown.

An operation that cannot obtain exclusive access fails without changing PAM's
password path. The lock coordinates ownership; it is not an authentication
signal.

## Biometric persistence

Catacomb persistence is one paired transaction:

- enrollment exports and durably writes the concrete biometric-user catacomb,
  then exports and confirms the master catacomb;
- only the final master confirmation commits the pending generation and causes
  the xART anti-replay record to advance; and
- restore first accepts a valid live identity for the requested user without
  loading either blob. With no valid live identity, authentication, deletion,
  enrollment, and independent recovery-candidate validation load and validate
  the selected durable master before loading and validating its paired user. A
  master loaded-state bit alone never substitutes for proving that generation.

The master and user files are therefore an inseparable generation pair. New
files are root-owned, private, written without following links, synchronized,
and atomically promoted only after both exports succeed. Existing active files
are never overwritten in place. Catacombs and xART records remain opaque and
their contents, identifiers, and hashes never enter logs or the repository.

## ACM policy registration and retry

Enrollment and restored matching keep one transient authorized ACM context
alive for the complete operation. Authentication follows the working stack's
narrow authorization profile: initialize and externalize the ACM context,
verify the keybag secret, read its UUID, and publish one final device-state
readback for the concrete biometric keybag before lending the credential to
Mesa. Enrollment and identity deletion add the `TouchIdEnrollment` credential
injection and policy verification; ordinary matching never injects or verifies
that enrollment policy. Authentication, deletion, and enrollment read the
existing per-user protected policy and re-submit those exact values before
confirming a valid live identity or restoring the complete durable pair.
Neither path changes policy as a side effect of registration. On a cold boot,
the full master load proves the durable generation before its user is loaded;
the live master bit alone is not trusted. A match request that performs this
cold restore stops before Match Start, completes SEP cleanup and relay
recovery, then reacquires one fresh lease. Only a validated live-identity fast
path may continue to sensor matching on that second lease. Warm matching stays
on one lease, and a second restore instead of the required live state fails the
request closed.

Mesa may return `EBUSY` while registering the runtime user. Only that status is
retried, using bounded backoff under the same ACM lease. If the bounded schedule
ends in `EBUSY`, the broker performs one exact readback and continues only when
both the requested and effective fields equal the submitted policy. This is an
idempotent already-live result, not cached authorization; every operation runs
the setter and verifies live state again. Any divergence, other error, failed
readback, or cancelled ACM lease fails the biometric attempt and restores the
relay.

## Failure behavior

- Missing machine data, catacombs, keybag state, hardware, NCM readiness, or a
  broker produces a biometric failure, never a password lockout.
- Cancellation closes the active transaction, clears cosmetic Touch ID state,
  releases SEP ownership, and restores the relay.
- xART writes are size-bounded, root-private, and atomic. A failed write leaves
  the last complete record intact.
- The root Touch Bar service validates peer credentials, frame bounds, buffer
  sizes, and typed actions. Disconnect releases synthesized keys and discards
  client buffers. Display and keyboard brightness are discovered independently
  from their standard Linux classes; ambiguous or unusable classes are not
  advertised to renderers.
- The user Touch Bar launcher runs the executable selected at
  `${XDG_CONFIG_HOME}/t1bridge/renderer`. If that selection is unavailable or
  exits, it uses the provider's notification action when available, otherwise
  reports one journal diagnostic, and falls back to the built-in renderer. An
  uncertain supervision failure never starts a potentially competing renderer.
- The built-in renderer reads one absolute `T1BRIDGE_DESKTOP_PROVIDER` path and
  invokes only the fixed contract in `interfaces.md`, directly and as the
  graphical user. Calls run on a bounded worker with no socket or daemon. Bad
  paths, output, deadlines, exits, and disappearance withdraw only provider
  state. Root never reads the setting or invokes the provider.

## Runtime install layout

| Path | Contents |
| --- | --- |
| `/usr/bin/t1bridge` | Administrative import, enrollment, matching, status, and validation CLI |
| `/usr/lib/t1bridge/` | Internal daemons, `sep-probe`, Touch Bar hardware service, and built-in renderer launcher |
| `/usr/lib/libfprint-2.so.2` | Separately packaged `libfprint-t1bridge` compatibility library; replaces the distro library until the driver lands upstream |
| `/usr/lib/fprintd` | Separately packaged `fprintd-t1bridge` compatibility daemon; replaces the distro daemon until fprintd honors driver-native duplicate detection |
| `/usr/lib/systemd/system/` | Service, socket, readiness, and preset units |
| `/usr/lib/udev/rules.d/` | Dynamic T1 device discovery and permissions |
| `/usr/lib/tmpfiles.d/` | Runtime and persistent directory creation |
| `/etc/t1bridge/` | Optional administrator configuration; never overwritten as package defaults |
| `${XDG_CONFIG_HOME}/t1bridge/renderer` | Optional user-selected renderer executable or symlink |
| `T1BRIDGE_DESKTOP_PROVIDER` | Optional absolute provider path set by downstream user-service configuration; unset by the T1Bridge package |
| `/var/lib/t1bridge/` | Root-only imported machine data, catacomb pairs, keybag state, and xART storage |
| `/run/t1bridge/` | Runtime sockets and transient state |
| `/run/lock/` | SEP and Touch Bar hardware arbitration locks |
| `/usr/src/` | Versioned DKMS module sources owned by `t1bridge-dkms` |

The administrative CLI keeps user identity and hardware privilege separate.
Direct enrollment and matching run as the non-root enrollment owner and send
one typed request to the root broker; enrollment refuses a root caller rather
than inferring an identity from `sudo` state. Machine-data import and status
require root. Status is strictly observational: it queries dynamic USB, DRM,
NCM, protected keybag-presence, and fixed systemd unit state without activating
or changing them, prints only the fixed identifier-free rows in
`hardware-validation.md`, and returns nonzero only when inspection itself
cannot be completed safely.

The v1 Touch ID overlay and broker cancellation endpoints retain their
hardware-proven compatibility paths documented in `interfaces.md`. Packages
write only under `/usr`; runtime setup creates `/var` and `/run` state, and
neither package upgrades nor uninstall overwrite per-user configuration.
