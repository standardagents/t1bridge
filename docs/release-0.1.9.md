# v0.1.9

Enrollment can now recover when the keybag relay is already stopped in systemd's
`failed` state. The enrollment path treats that state as stopped, performs its
existing bootstrap, and still requires the relay to become active before
continuing. Transitional or unknown states and failed/cancelled bootstrap remain
failures. This addresses [#21](https://github.com/standardagents/t1bridge/issues/21);
it does not resolve the native relay restart loop tracked in
[#14](https://github.com/standardagents/t1bridge/issues/14).

The optional `t1bridge-omarchy` 0.2.1-1 package tightens automatic-brightness
sensor selection and restarts its consumer when the T1 sensor or sensor proxy
changes. It discards queued readings after pause/unlock and monitor backlog,
then waits for a fresh reading before applying brightness. Synthetic discovery,
consumer and staged-install checks pass; broader live coverage remains in
[#17](https://github.com/standardagents/t1bridge/issues/17).

Publication now uses a checked-in maintainer tool that preserves unrelated
packages, signs complete repository indexes and manifests, serializes publishers
and retains interrupted staging for safe resume. Immutable packages must be
anonymously downloadable with the expected bytes before index publication;
the GitHub release stays a draft until the complete public view is verified.
See [the release procedure](releasing.md) and
[#24](https://github.com/standardagents/t1bridge/issues/24).

The core cohort is `t1bridge`/`t1bridge-dkms` 0.1.9-1,
`libfprint-t1bridge` 1.94.100-17 and `fprintd-t1bridge` 1.94.5-14.
Reboot remains the supported installation handoff. Suspend/resume, broad power
saving, broad camera coverage and Linux-only machine-data recovery remain open
validation or development work.
