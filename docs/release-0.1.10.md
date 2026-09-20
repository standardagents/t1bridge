# v0.1.10

Automatic machine-data import now reopens its storage root after EFI discovery
changes mount namespaces. This preserves the service's writable storage mount
and fixes the reproduced `private import temporary file could not be created`
failure in [#36](https://github.com/standardagents/t1bridge/issues/36), without
weakening the sandbox. The synthetic namespace regression fails before the
fix and passes afterward, including idempotent repeat import. Confirmation on
the reporting machine remains pending.

The default Touch Bar renderer now supports display-off state from a desktop
provider and cancels held input when the display turns off. Providers without
that capability keep their existing behavior. Display blanking is separate
from system suspend and does not establish recovery from the dark-panel
reports in [#34](https://github.com/standardagents/t1bridge/issues/34).

Failed Touch Bar system-resume initialization now requests a display-interface
reprobe. This is bounded failure recovery, not a claim that system suspend or
panel relighting works. Successful initialization followed by a dark panel
remains unresolved in [#18](https://github.com/standardagents/t1bridge/issues/18).

SEP diagnostics retain cleanup failures alongside the original operation
failure. Native rejection tests cover the relay boundary; the sustained relay
restart loop in [#14](https://github.com/standardagents/t1bridge/issues/14)
remains unresolved.

The setup guide now documents Omarchy Quickshell Touch ID unlock with password
fallback and explains the optional integration's required per-user session
hook. No package edits user PAM files or installs that hook automatically.

The core cohort is `t1bridge`/`t1bridge-dkms` 0.1.10-1,
`libfprint-t1bridge` 1.94.100-18 and `fprintd-t1bridge` 1.94.5-15.
The fingerprint pair is rebuilt for this cohort; its protocol is unchanged.
The existing optional `t1bridge-omarchy` 0.2.1-1 remains available.

On Omarchy, upgrade with `omarchy update`; other supported Arch installations
use a full `sudo pacman -Syu`. Reboot after successful DKMS and initramfs/UKI
generation to activate the updated drivers. Existing machine data and enrolled
fingerprints are retained; do not re-enroll or reset state to upgrade.

This remains a prerelease. Suspend/resume, runtime power saving, broader camera
and ambient-light coverage, general SEP key services, and Linux-only recovery
remain open development or validation work.
