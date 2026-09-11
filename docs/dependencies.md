# Current dependency surface

This inventory distinguishes shipped runtime links from build and packaging
tools. See the [README](../README.md) for official packages and function support.

## Runtime and linked dependencies

| Surface | Dependency | Scope |
| --- | --- | --- |
| Ambient-light desktop discovery | `iio-sensor-proxy`, stock `hid_sensor_hub` and `hid_sensor_als` | The core package recipe requires the distribution sensor proxy, which exposes the kernel IIO readings over the standard system D-Bus API. Its own udev rules activate the service. T1Bridge adds no sensor daemon or legacy iBridge driver; automatic brightness policy belongs to the desktop. Required starting with v0.1.8. |
| Rust userspace | Rust standard library and T1Bridge workspace crates | `Cargo.lock` contains only the six local crates; there are no registry or Git crates. The normal compiler/platform runtime (`libc`, ELF loader, and `libgcc_s` on the audited Linux build) is still required. |
| Machine-data importer | System `libudev` | The packaged preserved-ESP command enumerates block devices through the already-required systemd/libudev stack. Its current direct-FDR production path does not retain a `liblzma` dynamic link. Nothing launches `xz`, accepts device paths, or adds another device manager. |
| Local sockets | Linux Unix `SOCK_SEQPACKET`, `SO_PEERCRED`, and libc | `t1-daemons` enables only `t1-platform/seqpacket`. The focused in-tree C boundary supplies the Linux socket operations; there is no socket crate. |
| Private NCM readiness | Linux rtnetlink, udev, and systemd 256 or newer | An early link policy keeps the driver-assigned ephemeral name and assigns the interface to T1Bridge before network managers observe it. One short-lived capability-limited helper then validates the `apple_t1_ncm` interface, marks only that ephemeral kernel index up, and waits boundedly for IPv6 link-local readiness. The package creates no network-manager profile and invokes no `ip`, `nmcli`, network daemon, or shell. |
| Guarded USB cycle | Linux sysfs, libc, and the packaged systemd CLI | The root-only acceptance command invokes fixed `systemctl` operations directly without a shell, dynamically validates the complete T1 interface map, and writes only the validated configuration-selector driver's bind controls. It adds no library, daemon, socket, USB reset, or power-policy dependency. |
| SEP transport | Linux usbfs UAPI and libc | The in-tree C transport uses `/dev/bus/usb` and `USBDEVFS_*`; it does not use libusb or a cryptography library. |
| Kernel modules | Mainline Linux USB, DRM, networking, media, V4L2, videobuf2, and CDC-NCM APIs | `t1_cfgsel` and `appletbdrm` use kernel USB/DRM APIs. `apple_t1_ncm` reports only the mainline `usbnet`, `cdc_ncm`, and `cdc_ether` modules as load dependencies. The temporary `uvcvideo` override is mainline Linux 7.1.9 plus the bounded H.264 descriptor parser required by the T1 camera. |

`liblzma` is currently build/test-only input. The importer manifest enables
`xz` for its bounded archive-walker library path, but the packaged CLI has no
production caller of that path and reaches preserved data through direct FDR.
The linker's unused-section and as-needed handling therefore leaves no
`liblzma` `NEEDED` entry in a clean importer-only release build or the stripped
package binary. Workspace-wide all-feature tests do exercise and link the XZ
boundary.

## Build, quality, and packaging only

- Rust builds use Cargo and rustc; the manifests declare Rust 1.88 as their
  minimum language-toolchain version. `rustfmt`, Clippy, and `cargo-deny` are
  quality-gate tools, not shipped runtime dependencies.
- Native userspace builds require a C17 compiler, an archiver, GNU Make, libc
  development headers, and Linux UAPI headers. Importer builds additionally
  require the system `liblzma` headers and linker library.
- The direct C PAM adapter and its Linux-PAM headers are build/test-only
  rollback tooling. The package ships no T1-specific PAM module; supported
  integration uses the separately packaged libfprint driver with stock
  `pam_fprintd`.
- The sanitizer quality target specifically uses GCC AddressSanitizer and
  UndefinedBehaviorSanitizer. These runtimes are test-only.
- Kernel-module builds require Kbuild and headers matching the target kernel.
  The Arch package depends on DKMS to compile and install the modules;
  DKMS and package-building tools are not T1Bridge userspace runtime links.

No current code depends on crates.io packages, libusb, OpenSSL, an external
`xz` command, `ip`, `nmcli`, a network-management daemon, `libfprint`,
`fprintd`, or the `macbook-t1-linux` tree. The built-in renderer's generated
Cupertino, Myna UI, and Inter masks add no runtime dependency; their required
notices are centralized in [`THIRD_PARTY_NOTICES.md`](../THIRD_PARTY_NOTICES.md).

The `libfprint-t1bridge` and `fprintd-t1bridge` compatibility packages replace the distribution packages as a matched pair; the fprintd package depends on the exact T1Bridge libfprint package version and release rather than accepting the distro library or a different compatibility build. The latter changes only enrollment dispatch and its advertised stage count for drivers that report native duplicate detection; drivers without that capability retain upstream behavior. Both packages are rebuilt when their corresponding Arch package changes and are removed once their changes land upstream.

The Touch Bar's optional desktop-provider executable is a downstream
integration point, not a core runtime dependency. T1Bridge contains no
Omarchy, `wpctl`, MPRIS/`busctl`, desktop-notification utility, provider daemon,
or provider socket implementation. Its Rust process fixture is built only by
the all-targets test feature and is not included by the package recipe.

## Supported and tested versions

The v1 packaging contract is intentionally narrow:

- Distribution and architecture: x86_64 Arch Linux and Arch-based distributions, using the official signed packages. Other distributions may package the source but are not part of the v1 tested support claim.
- Kernel: recorded hardware validation uses Arch kernel `7.1.9-arch1-2`; CI also compiled all four DKMS modules against Arch kernel headers `7.2.2-arch1-1`. These are tested versions, not a guarantee for every kernel; install headers matching the target kernel and check DKMS results.
- Init and device stack: systemd 256 or newer is required by the core package. The current hardware evidence uses systemd 261.2.
- Rust: 1.88 is the declared minimum. Recorded checks pass `cargo +1.88.0 test --locked --workspace --all-targets --all-features`; CI also passes with Arch Rust 1.98.
- Native toolchain: C17 and matching kernel Kbuild headers are required. The active development build uses GCC 16.2.1. CI pins its container image, while the installed Arch compiler, Rust toolchain, and kernel headers are point-in-time versions recorded by each run because both workflows update from their configured package mirrors.
- XZ: the build and all-feature test gate uses the system `liblzma` headers and library from Arch `xz`; version 5.8.3 is tested. The current packaged runtime has no `liblzma` dynamic dependency.
- Standard fingerprint stack: the matched compatibility pair is `libfprint-t1bridge 1.94.100-8` and `fprintd-t1bridge 1.94.5-5`. The fprintd package requires that exact T1Bridge libfprint package version and the libfprint 2 ABI.

## Audit evidence

The current audit used `cargo tree --workspace --all-features`, `readelf` on
the built Rust and C test executables, `modinfo` on all four built modules,
and `pkg-config` for the installed `liblzma`. It found only local Cargo
packages, confirmed `liblzma` linkage in all-feature tests, confirmed its
absence from a clean importer-only release build and the stripped package, and
confirmed the kernel-module relationships listed above.

The minimum-Rust run, CI, dynamic-link inspection, DKMS build logs, package
manifests, and hardware observations establish the matrix above. An untested
version or operation is not covered by those observations.
