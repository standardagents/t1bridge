# General Secure Enclave key services

Assessment for [#23](https://github.com/standardagents/t1bridge/issues/23),
2026-09-14, against T1Bridge `0229cd6`.

**Continue protocol research; do not ship a general Linux key service yet.**
T1 has relevant macOS precedent. The sources reviewed below do not establish
the T1 requests, ownership rules or access controls needed for a Linux signer.
General key services remain unimplemented. No live key or biometric operations
were performed for this assessment.

## Capability inventory

Apple explicitly includes 2016–2017 T1 MacBook Pros in its
[Secure Enclave overview](https://support.apple.com/guide/security/the-secure-enclave-sec59b0b31ff/web).
Its [Security framework guide](https://developer.apple.com/documentation/security/protecting-keys-with-the-secure-enclave)
covers Touch Bar/Touch ID Macs and describes enclave-generated NIST P-256 keys,
signing and ECDH. It excludes importing an existing plaintext private key.
These are macOS API guarantees; the guide does not specify the T1 USB protocol.

| Capability | Established evidence | T1 Linux gap |
| --- | --- | --- |
| Key generation | The macOS API supports creating enclave keys. T1Bridge's `sep_keystore_build_create` instead accepts a 32-byte **keybag secret** and returns a bag handle. | No typed asymmetric key-generation request or verified public-key reply. Bag creation is not key-pair generation. |
| Algorithms and operations | P-256 signing/ECDH are the documented macOS baseline. SeKey implements P-256 SSH signing through that framework. | No verified signing, ECDH or key-export selector. No basis for promising RSA, Ed25519, other curves, post-quantum keys or attestation on T1. |
| Storage and persistence | T1Bridge serializes/restores its biometric bag. macOS consumers store key references or opaque representations. | No general-key namespace, wrapping format, crash recovery, deletion contract or reboot-persistence result. An opaque blob is not proof of device binding. |
| Access policy | macOS provides access-control flags and authentication contexts. T1Bridge has Linux caller admission and exclusive SEP ownership for biometric operations. | No mapping from a Linux caller or fresh Touch ID result to firmware-enforced key-use permission. UID checks alone do not reproduce macOS entitlements. |
| Firmware and startup | Existing Touch ID uses the configured T1 USB stack, private network/xART services and preserved same-machine Apple data. | No general-key firmware compatibility range or evidence that biometric startup provides the required keystore environment. Record model and firmware version for any future result. |

Local evidence: [keystore selectors and interface](../sep-probe/sep_keystore.h),
[builders and reply parser](../sep-probe/sep_keystore.c),
[keybag workflow](../sep-probe/sep_keybag.c),
[platform FFI](../crates/t1-platform/src/ffi.rs), and
[Touch ID startup](touch-id.md).
The capabilities reply remains opaque in `parse_typed_output`; it is not an
algorithm inventory. No general crypto operation is exposed by the FFI.

## Prior art and what transfers

The review covered macOS key consumers, SEP exploitation research, Asahi
transport code and current T1/Apple Silicon Linux projects. Repository links
pin the inspected revisions. This is a bounded source review, not a claim that
no other implementation exists.

| Work | Evidence and reusable contribution | Limit for T1Bridge |
| --- | --- | --- |
| [SeKey](https://github.com/sekey/sekey/blob/869cb896da62ad43352c99a2b7d0e1baf4b00af6/src/keychain.rs), Nicolas Trippar and contributors | Rust code generates keys and signs through `SecKeyGeneratePair`/`SecKeyCreateSignature`, with the Secure Enclave token and access-control flags. Useful consumer and public-key/signature format precedent. | Uses macOS frameworks. It supplies no T1 USB implementation. |
| [Secretive](https://github.com/maxgoedjen/secretive/blob/2382cd01536c37a3064ff1187adb656caa40aadb/Sources/Packages/Sources/SecureEnclaveSecretKit/SecureEnclaveStore.swift), Max Goedjen and contributors; [age-plugin-se](https://github.com/remko/age-plugin-se/blob/57ccea12fc1a3d3bb81b8866745be4d2951c4eaf/README.md#requirements), Remko Tronçon and contributors | Secretive uses CryptoKit enclave keys and authentication contexts. age-plugin-se separates public-key encryption from operations requiring the local enclave. Useful ownership, opaque-storage and consumer-boundary examples. | macOS owns the hardware backend. age-plugin-se's Linux build encrypts with public keys; it cannot generate enclave identities or decrypt there. Modern CryptoKit algorithms do not establish T1 support. |
| [Blackbird disclosure](https://gist.github.com/littlelailo/d2f32eac06bd4375ba0059a20c22c3c4), littlelailo; [PongoOS SEP driver](https://github.com/checkra1n/PongoOS/blob/4c9b7541629234147fcc778f0ce4162482aaccef/src/drivers/sep/sep.c), checkra1n contributors | Real SEPROM vulnerability research and low-level monitor code. Useful for understanding boot and trust boundaries. PongoOS's automatic exploit path explicitly selects A10/A10X/T2. | The inspected path does not demonstrate T1 exploitation or a T1/Linux signer. Boot-ROM control is distinct from a stable key-service protocol. Modified SEP firmware would require a different security assessment. |
| [Inside Apple Secure Enclave Processor in 2025](https://2025.hexacon.fr/slides/inside_secure_enclave_processor_in_2025.pdf), Quentin Salingue, Synacktiv | Documents SEP architecture, pointer authentication, ROM patching and Trusted Boot Monitor. Useful generation-specific security research. | Its newer-chip mechanisms and boot analysis do not provide a T1 key-generation/signing ABI. |
| [Asahi Linux SEP driver](https://github.com/AsahiLinux/linux/blob/77cb8f24c2381a8abb7272d7bbdec548d6426a8a/drivers/soc/apple/sep.rs) and [m1n1 SEP client](https://github.com/AsahiLinux/m1n1/blob/b4654b32941d51afdb77579d63e7cb1aa6c03ecc/proxyclient/m1n1/hw/sep.py) | Firmware boot, shared memory, mailbox messages and endpoint discovery provide transport/lifecycle examples. The inspected kernel driver identifies itself as a stub and implements no key service. | Apple Silicon uses on-chip resources and mailboxes. T1Bridge reaches a separate T1 over USB; endpoint names and opcode values cannot be transferred without evidence. |

The newer Linux projects narrow the search further:

- [DriftDeV/apple_biometrics_linuix](https://github.com/DriftDeV/apple_biometrics_linuix/blob/ff6bf4461ecaa3294a69ea464a7d85b043129583/dkms_module/apple_t1_touchid.c)
  targets T1. Its inspected kernel driver registers/probes USB and leaves
  interface selection and cleanup as TODOs. It provides no signing operation.
- [ELI3GANT/open-touchid-linux](https://github.com/ELI3GANT/open-touchid-linux/blob/6c7169da061c92f52d549d0ae92a6ceff9f4957f/RESEARCH_NOTE.md)
  reports live SEP endpoint advertisements on M1 and explicitly says Touch ID
  authentication remains unavailable. Endpoint discovery does not identify a
  callable signing contract.
- [apstrand/m2-air-touchid](https://github.com/apstrand/m2-air-touchid/blob/a6d67b872a25455cf0ff75a42316dd315ef1f4df/README.md)
  reports driver binding on M2 but no SEP reply in its current Linux baseline.
  Its result does not demonstrate general keys or T1 compatibility.

These projects retain credit for their own work. No external implementation,
firmware, captures, extracted assets or key material were incorporated here.

Apple's open [SecCTKKey implementation](https://github.com/apple-oss-distributions/Security/blob/db15acbe6a7f257a859ad9a3bb86097bfe0679d9/OSX/sec/Security/SecCTKKey.m)
also stops at CryptoTokenKit token/session operations. It helps locate the
macOS boundary; it does not supply the missing Linux T1 transport mapping.

## Proposed Linux contract

### Source mapping follow-up, September 20

The public Security tree at `db15acbe6a7f257a859ad9a3bb86097bfe0679d9`
was checked for the next layer below SecKey/CryptoTokenKit. Its
[SecAKSWrappers header](https://github.com/apple-oss-distributions/Security/blob/db15acbe6a7f257a859ad9a3bb86097bfe0679d9/OSX/utilities/SecAKSWrappers.h)
conditionally imports `AppleKeyStore/libaks.h`; that backend implementation
and `libaks_ref_key.h` are not supplied in this tree. The similarly named
functions in [mockaks.m](https://github.com/apple-oss-distributions/Security/blob/db15acbe6a7f257a859ad9a3bb86097bfe0679d9/tests/secdmockaks/mockaks.m)
are test doubles, not a T1 protocol implementation:

| Operation sought | What this source actually provides | Contract still missing |
| --- | --- | --- |
| Create | `aks_ref_key_create` retains a mock object with fixed synthetic key data. | T1 request/reply, key type, allocation and owner. |
| Export public key | `aks_ref_key_get_public_key` returns a zero-filled dummy buffer. | T1 reply encoding and validated point format. |
| Sign | `aks_ref_key_sign` returns an error as an unimplemented mock. | T1 signing selector, digest rules, status and signature encoding. |
| Delete/free | Mock deletion returns success; free releases a host-side reference. | Firmware release acknowledgement, uncertain-outcome behavior and isolation from biometric bags. |

The [AKS regression consumer](https://github.com/apple-oss-distributions/Security/blob/db15acbe6a7f257a859ad9a3bb86097bfe0679d9/OSX/shared_regressions/si-44-seckey-aks.m)
exercises SecKey signing/verification and opaque references through macOS
frameworks. It does not connect those calls to a T1 USB request. None of the
four operations has reached the source-to-wire evidence gate, so adding typed
codecs now would encode guesses. No mock bytes or host API selector numbers
were copied into T1Bridge.

The next source requirement is the T1 backend mapping between those host
operations and the EmbeddedOS SEP/keystore service, including allocation and
release semantics. A useful contribution must identify firmware scope and
request/reply layouts for all four operations. Until it exists, retain the
existing synthetic biometric codec tests and do not allocate disposable keys
on hardware or expose a new Linux signing socket.

### Future API boundaries

This is a design constraint for a future implementation, not an available API.
Start with disposable P-256 signing only. Defer ECDH, persistence, SSH-agent
integration and TPM interfaces until the underlying service is established.

| Boundary | Required behavior |
| --- | --- |
| Caller authentication | A privileged SEP owner authenticates peers using kernel credentials. An explicit policy grants access to the requesting UID. Never accept a caller-supplied UID or expose raw SEP selectors over a public socket. |
| Key ownership | The owner issues an unpredictable, caller-bound key ID. Operations are create, export public key, sign a 32-byte SHA-256 digest, cancel, and delete **that caller's disposable key**. Define public-key/signature encodings and reject unsupported algorithms. Raw firmware handles stay private. |
| Storage | Initially permit only a verified disposable lifetime. Do not borrow the biometric bag, promote a test bag to the system bag, or persist arbitrary returned blobs. Define crash cleanup before allocation; process exit alone cannot guarantee firmware cleanup. |
| Authorization and non-exportability | Separate Linux permission from firmware-enforced key policy. A successful signature proves an operation, not where the private key lived or whether it can be exported. Any hardware claim needs T1-specific creation, storage and firmware evidence. Root/kernel compromise remains inside the Linux policy boundary. |
| Scheduling and cancellation | Reuse the existing exclusive SEP operation owner. Give Touch ID priority; return busy rather than race or restart its relay. Bind cancellation to the requesting operation, enforce deadlines, discard late results and prevent automatic replay after an uncertain outcome. Logs contain stages/status only. |

macOS keychain access groups and entitlements have additional policy semantics;
see [TN3137](https://developer.apple.com/documentation/technotes/tn3137-on-mac-keychains).
Fresh Touch ID authorization for a key operation remains a separate protocol
question. Reusing an earlier authentication result is not an established
firmware policy.

The existing [operation owner](../sep-probe/sep_operation.c),
[session interface](../sep-probe/sep_session.h) and
[security review](security-review/v1.md) provide local admission and cleanup
patterns. They do not settle firmware object lifetime.
[The unresolved relay failure in #14](relay-recovery.md) already demonstrates
why closing a transport cannot be treated as proof that a bag was released.

## Next work and experiment gate

**The next feasible slice is source-only protocol mapping.** Trace a T1-specific
key-generation/signing path across the host API, transport request and firmware
service. For create, public-key export, sign and release, require a cited
request/reply layout, object owner, error behavior and firmware scope. Record
unknowns explicitly; do not fill them with selectors from another platform.
Only then add C/Rust typed builders and parsers with synthetic fixtures for
length, selector, transaction, status and ownership failures. That slice passes
when each encoded operation has a traceable contract and rejected input cannot
reach the transport.

Source inspection and synthetic tests can establish encoding, caller policy,
local cleanup and cancellation behavior. They cannot establish hardware
non-exportability, durable deletion, reboot persistence or coexistence.

A live disposable-key test becomes justified only after isolation from existing
bags and the key's allocation/release contract are established. The attended
test would:

1. Record model and firmware/software versions; establish a healthy Touch ID
   baseline and retain password access. Use no production keys.
2. Create exactly one disposable key in the verified isolated namespace.
   Export its public key and sign a synthetic challenge. Verify the signature
   independently; a changed challenge and a different public key must fail.
3. Check bounded cancellation without retrying uncertain allocations or signs.
   Confirm the owner rejects another caller before any SEP request.
4. Release only the disposable object using the verified contract. Confirm it
   cannot be reused and that ordinary Touch ID still works. Stop on rejection
   or uncertain cleanup; never reset T1 or alter existing biometric state.

Keep returned blobs and live diagnostics private. Publish only source-derived
formats, synthetic fixtures and aggregate results. A later persistence test
would need its own owned-object storage/recovery contract; one successful
signature would not satisfy it.

Revisit the shipping decision when T1-specific create/sign/release contracts,
caller policy and an attended disposable-key result are available.
