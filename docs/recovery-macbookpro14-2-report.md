# MacBookPro14,2 native recovery report — September 23–24, 2026

Owner-attended observations and local code investigation for [#43](https://github.com/standardagents/t1bridge/issues/43). Prepared for source review on October 8.

Firmware recovery succeeded on my **MacBookPro14,2 with local C/Rust changes**, followed by working Touch Bar and Touch ID. The signed recovery command failed first. This is useful troubleshooting evidence, but it does **not** complete #43’s unmodified signed-package acceptance.

Tested September 23–24, 2026: Omarchy, kernel `7.2.5-4-omarchy`; `t1bridge` and `t1bridge-dkms` **0.1.12-1**, `libfprint-t1bridge` **1.94.100-20**, `fprintd-t1bridge` **1.94.5-17**, `t1bridge-omarchy` **0.2.2-1**. Local recovery changes were based on `81cbdf81026a16e02f0bea74735c6b029a8ffae2`.

The T1 already needed recovery (`05ac:1281`), with no usable `EFI/APPLE/EMBEDDEDOS` or known same-Mac backup. Surviving Apple data was preserved; working EFI was not erased for testing. Recovery was attended, on mains power, with password access retained.

### Departures from the tester procedure

Compared against the [issue #43 tester procedure](https://github.com/standardagents/t1bridge/issues/43), rechecked October 8. Upstream `main` still points to the tested base commit above; the hardware results below are from September 23–24, not a new October acceptance run.

| Procedure | Actual run and limitation |
| --- | --- |
| Full system update, signed matched cohort | Installed the specified signed packages additively using existing distro databases; **no `omarchy update` or full `pacman -Syu`**. A version-scoped DKMS override corrected packaged source metadata declaring 0.1.11 instead of 0.1.12. Builds, boot images and reboot were checked. Whether a full update would change the failures is unknown. |
| Packaged native recovery command | Tried the signed command, then the same binary with process-scoped trust additions, then separately built patched executables. Installed signed binaries were retained, but the successful executable was a local build. |
| Inspect failures before retrying | Custom helpers archived inspected attempts intact, allowed explicit retries and, when needed, invoked the guarded reset on the non-HID restore device. Later owner-authorized temporary sessions supplied `RECOVER`, allowing at most three launches starting within 60 minutes per session. Retries were individually initiated after inspection. No preserved attempt was deleted or functional T1 reset. Final success used a newly added `--resume-boot`; this is beyond shipped retry behavior. |
| Post-recovery validation | Added a private-interface/peer-only xART firewall allowance. Initial enrollment failed; redacted diagnostics and an idle broker restart preceded successful enrollment. Owner-requested sudo/screen-unlock PAM integration was additional local work, not a recovery prerequisite. |

Firmware restore remained native C/Rust; no external recovery executable performed it. Temporary privileged sessions were closed; no permanent sudo grant was installed. The recovered EFI archive was read back and hash-verified, but remains on the internal disk; an external copy was deferred.

### Signed-command failures and local fixes

The first signed invocation exited **32** at the firmware-signature request, reporting `recovery input/output failed`. A separate identity-free HTTPS probe reproduced a trust failure through the native libcurl boundary: strong supporting evidence for TLS attribution, **not proof from that generic command error**. With the scoped Apple root, the HTTPS probe passed, but the second signed invocation exited **32** after iBEC handoff; its exact failing operation was not captured.

Subsequent instrumented native builds led to these local changes. Some address observed protocol blockers; the broader retry/resume and trust-policy changes need separate upstream review:

1. **Apple trust (external to the code contribution):** added process-scoped public anchors for the encountered Apple chains; hostname/peer verification stayed enabled and the global CA bundle stayed unchanged. The FDR trust-object digest matched the retained authenticated-TSS ticket’s `IM4M/MANB/ftsp/DGST`, and its certificate matched `rssl`. No separate offline ticket-signature verification was performed.
2. **USB stalls:** narrowly tolerated observed `EPIPE` on `rsepfirmware` and, later, the nonblind final-boot `ticket` command. Successful restore status and functional USB proof remained required. Blind `memboot` tolerance was also added, but that stall was not observed.
3. **Mux framing:** corrected incoming magic to `0xFACEFACE` (outgoing `0xFEEDFACE`), receive sequence at bytes 14–15, and whole-frame outgoing queue admission.
4. **Outgoing message budget:** raised outgoing XML capacity to 32 MiB minus the header for the restore response; incoming/TSS limits remained 16 MiB.
5. **HTTPS-only FDR route:** mapped the firmware’s exact `gg.apple.com:80` destination to host-side verified TLS at `gg.apple.com:443`. No plaintext fallback or certificate-verification bypass was used; device-originated TLS tunnels remained opaque.
6. **`FUDData`:** implemented manifest-selected responses; the pinned Customer Boot identity required an empty valid response.
7. **Repeated `FDRMemoryCommit`:** accepted bounded, immutable, synced/readback-verified snapshots before ACK. Successful final status selects the canonical store; final provisioning/personalization stores must match, while intermediate snapshots may differ.
8. **FDR diagnostics/retry:** added bounded connect-only retries after an unexplained connection failure. Successful restore passes required no retries; the earlier cause remains unknown.
9. **Boot-only resume:** reused completed restore phases after validating phase markers, matching final factory data, ticket/device identities and all four image/ticket pairs. An initial local `rdsk`/`osrd` validator mistake was caught before USB work and corrected with regression coverage.

Both restore phases completed successfully; the final patched boot-only invocation exited **0**. Boot reached configuration 2 / `05ac:8600` with HID, stable for 30 seconds; native EFI publication and sensor-matched import succeeded. Subsequent biometrics worked, although I did not independently trace calibration loading.

### Acceptance results and remaining failures

| Check | Observed result |
| --- | --- |
| Fingerprints | Enrollment, correct-finger match and incorrect-finger rejection passed after the initial enrollment intervention described above. |
| Desktop authentication | Sudo and screen unlock passed, including separate password-fallback tests. |
| Warm reboot / full shutdown and power-on | Retained enrollment, verification and owner-confirmed Touch Bar display/input passed. One cold-start keybag failure automatically retried successfully. |
| Ambient light | Continuous standard sensor reports fell from approximately 753 lux to 2–3 when covered, then recovered to approximately 769. This tests changing-light response, not absolute calibration. |
| Camera capture/decode | FFmpeg/V4L2 H.264 at 30 fps decoded 153 frames at 640×480 and 154 at 1280×720 over five seconds, both exit 0. The 720p run had two non-monotonic DTS warnings. No images retained. |
| Camera preview | Initial video had noticeable delay. FFplay with `-fflags nobuffer -flags low_delay -framedrop -analyzeduration 0 -probesize 32768` passed owner usability at 640×480. **720p usability failed before and after reboot with the same options.** Cause and precise latency remain unknown; browser/PipeWire/portal compatibility is untested. Indicator on/off checks passed. |
| Hibernation-associated failure | Relay errors began immediately after recorded hibernation resume, before camera tests; `t1bridge-keybag.service` reached `start-limit-hit`. Verification failed while enrollment remained listed. Temporal association does not establish causation. |
| Normal reboot after that failure | Restored retained-finger verification without re-enrollment, firmware recovery or manual service restart. A second match passed during uninterrupted 640×480 preview, working Touch Bar and 58 fresh light reports. One initial keybag failure automatically retried successfully; later zero restart counters do not erase it. This is not a durable hibernation fix. |

Measurement corrections: an early helper queried a nonexistent relay unit; those fields were discarded and the failure above was verified against the real unit/journal. An early light helper missed the cover transition; the successful light result used corrected continuous capture.

Full `make quality` and release builds passed. Additional focused recovery tests passed **37 cases**, with four public-package-dependent cases skipped in that run; separate pinned-package checks had passed earlier. Four storage tests passed on disposable **128 MiB FAT32** with unprivileged execution and a clean post-unmount filesystem check. Added cases preserve every reconstructed staging prefix across repeated publication attempts and reject each missing artifact before staging. These do not establish actual power-loss durability or complete the restore/network/signing/repeat-invocation failure matrix.

### Fresh source validation — October 8, 2026

Fresh `make quality` passed on October 8 (exit 0): formatting, warning-denied workspace/all-feature Clippy, **1,000 Rust tests passed**, dependency policy, C tests and ASan/UBSan runs, kernel-module builds against `7.2.5-4-omarchy`, and packaging verification. Five Rust tests were explicitly ignored: four requiring an external pinned Apple package and one live-hardware preflight. This run used Rust/Cargo 1.98.1 and LLD 22.1.8 from signature-verified cached distro packages extracted into an isolated workspace, plus cargo-deny 0.20.2. No toolchain or module was installed. Documentation links, whitespace and privacy-pattern checks passed.

These software checks do not repeat the September hardware observations or establish signed-package recovery acceptance.

### Connection to #46 and suggested next steps

The September 30 report in [#46](https://github.com/standardagents/t1bridge/issues/46) describes six overlapping blockers on MacBookPro13,3/Ubuntu: mux magic, outgoing plist size, the FDR port-80 request, empty `FUDData`, multiple `FDRMemoryCommit` messages, and final-boot `ticket` EPIPE. That is corroborating owner-reported evidence on a different model, not an independently reproduced result here.

There are material differences: this run routed the legacy destination over verified HTTPS and uploaded the ticket before tolerating the command stall; #46 describes booting without the command. Our enrollment/match/rejection passed after the broker intervention above, whereas #46 reports `LOAD_CALIBRATION` rejection. That does not establish why their calibration fails, nor justify transferring factory data or assuming all T1 models work.

For maintainers, I suggest:

1. Compare the overlapping protocol findings with #46 and review narrowly scoped transport/response fixes first. The accompanying draft source contribution preserves the hardware-tested native changes and regression additions. The trust-anchor setup remains external; maintainers should review the explicit boot-resume feature and HTTPS policy separately from narrow protocol fixes.
2. Keep **#43's signed-release acceptance open** until an appropriate machine already needing recovery completes the packaged flow. The successful patched run and synthetic FAT tests do not close that gate or the full failure/durability matrix. This now-functional T1 should not be erased or restored again to manufacture a cleaner baseline.
3. Route the lifecycle finding to [#18](https://github.com/standardagents/t1bridge/issues/18), explicitly identifying our sleep mode as **hibernation**, and the preview/application findings to [#20](https://github.com/standardagents/t1bridge/issues/20). Neither has an established root cause from these tests; those follow-ups can proceed separately from native recovery review.

Private EFI, tickets, personalized images, factory data, keybags, raw logs and machine identifiers are excluded. No promotion beyond experimental support is claimed.

— GPT on behalf of @seanabrahams
