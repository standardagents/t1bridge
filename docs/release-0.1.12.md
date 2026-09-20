# v0.1.12

The signed `t1bridge` package adds an experimental native online recovery command:

```sh
sudo t1bridge machine-data recover --online --efi /path/to/mounted-esp
```

T1Bridge owns the restore protocol, direct USB transport, Apple signing/FDR
exchanges and EFI publication. It uses established system libraries for HTTPS,
hashing and archives. No external recovery executable or third-party restore
stack is required.

Recovery stays local first. Existing preserved EFI data, partial generations,
inspection failures and functional devices block the online command. A verified
same-Mac backup should be used before online recovery. Installation and upgrades
never run recovery. The command requires an attended terminal, a T1 already in
recovery mode, and matching userspace/DKMS packages.

The implementation requires successful restore status, durable FDR capture and
matching replay. It keeps each personalized image with its ticket, checks a
stable functional USB boot, and saves one complete EFI generation without
replacing existing data. Private interrupted attempts are retained and require
inspection before another restore. A root-only kernel interface discovers a
unique T1 `FRST` method and refuses functional devices and arbitrary ACPI paths.

Validation includes `make quality`, synthetic protocol and storage failure
tests, C sanitizers, kernel builds, and native extraction/personalization checks
using Apple's checksum-pinned generic package outside the repository. The
personalization check used a synthetic ticket and did not contact Apple signing
or a USB device. No Apple binaries or machine data are distributed.

This release enables tester feedback; it does not establish real-device
recovery. [#43](https://github.com/standardagents/t1bridge/issues/43) tracks
generated-data import/calibration load, enrollment, correct/incorrect fingers,
warm/cold persistence, Touch Bar, camera and ALS. Test only a machine that
already needs recovery. Do not erase working EFI to create a test case.
See [recovery instructions and limits](linux-recovery.md).

Packages: `t1bridge` and `t1bridge-dkms` 0.1.12-1,
`libfprint-t1bridge` 1.94.100-20 and `fprintd-t1bridge` 1.94.5-17.
The fingerprint pair is rebuilt for this cohort; its protocol is unchanged.
The existing optional `t1bridge-omarchy` 0.2.2-1 package is retained.

Use `omarchy update` on Omarchy or a full `sudo pacman -Syu` on other supported
Arch installations. Confirm DKMS and initramfs/UKI generation succeed, then
reboot to load the matching driver. Existing machine data and enrollments
remain in place.
