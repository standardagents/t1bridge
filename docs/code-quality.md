# Code quality

`make quality` is the repository's canonical deterministic quality command.
Local validation, CI, and future release validation call this same target. It
runs, in fast-feedback order:

1. formatting verification with `cargo fmt`;
2. warning-denied Clippy over every workspace target and feature;
3. the complete Rust workspace test suite;
4. dependency advisory, license, source, and ban policy with `cargo deny`;
5. all DKMS modules against the selected kernel headers.

The command requires the stable Rust toolchain with rustfmt and Clippy and
`cargo-deny`, a C compiler, kernel headers, Git, GnuPG, libarchive tools and
pacman-contrib for the publication boundary tests. Override `KDIR` when validating
against headers other than the running kernel. It is non-interactive and does
not modify tracked files.

## Design and source size

An authored source file around 2,000 physical lines prompts a cohesion review.
The number is guidance, not proof of a defect and not an automatic failure:
generated files, vendored material, and kernel-derived
sources have different ownership constraints. Review whether the file still
owns one coherent concern, whether its dependencies and side effects remain
explicit, and whether a split would create meaningful long-lived ownership.

Prefer the smallest component and workflow that protects a named behavior or
failure domain. Add abstractions after shared responsibility is clear, not in
anticipation of hypothetical consumers.

## Tests

Tests should fail when supported behavior, an interface contract, a state
transition, or a failure mode breaks. Prefer integration tests at privilege,
IPC, PAM, hardware-transport, storage, and configuration-fallback boundaries.
Avoid tests that mirror private implementation structure. Exact byte and text
assertions are appropriate for wire formats, parsers, serializers, generated
files, command output, and user-visible copy only when exactness is part of the
contract.
