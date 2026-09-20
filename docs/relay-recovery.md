# Sustained keybag relay failures

Investigation for [#14](https://github.com/standardagents/t1bridge/issues/14),
updated 2026-09-20. The native wedge remains unresolved. The restart backoff
reduces repeated failures; it does not restore the Secure Enclave's state.

## What the current evidence establishes

The [0.1.9 MacBookPro14,3 report](https://github.com/standardagents/t1bridge/issues/14#issuecomment-5643314506)
records a successful restore, then a rejected promotion (`0x0d`) after a
successful load (`0x03`). Subsequent retries reject load itself. Both failures
carry outer status -1 and no valid inner status. Earlier reports on the same
model and MacBookPro14,2 include load-first failures. This happened under
ordinary lock-screen authentication cycling without system suspend; #18 is a
separate lifecycle failure. Reported recovery after full power-off is evidence
of that occurrence, not permission for automated resets or a proven root cause.

## Source findings and limits

| Boundary | Established behavior | Still unknown |
| --- | --- | --- |
| Relay startup | `sep_keybag.c::keybag_relay_callback` reads saved material, prepares shared bags, loads and promotes the biometric bag, then initializes ACM and publishes readiness. | Whether repeated native load/promotion consumes firmware resources. |
| Authentication handoff | Existing-only authentication uses the promoted bag, verifies its secret, resolves its UUID and checks final state. | Whether a relay may reuse that bag with the same guarantees without replaying the saved blob. |
| Load rejection | No accepted source handle is returned; startup ends before promotion/readiness. | Whether the firmware allocated anything before rejecting the request. |
| Promotion rejection | Load has already returned a transient source handle. Startup ends before ACM initialization/readiness. | Whether promotion consumes that handle on success or failure; whether an explicit release is required. |
| Local teardown | `sep_operation.c::close_product_session` cleans up active ACM state, destroys transport/session state and closes the SEP descriptor; the operation lock is then released. Remote ACM deletion errors propagate through `sep_session_acm_exchange`. | Transport close is not proof that the firmware has released a loaded keybag. |

No code in the relay callback rewrites the saved blob. Cancellation still runs
cleanup; `-103` alone does not demonstrate a leaked ACM context. The failure
sequence is consistent with a problem in repeated restore/promotion but does
not prove handle exhaustion, damaged saved material, or any particular release
selector. In particular, selector numbers from host macOS APIs must not be
assumed to identify equivalent T1 relay requests.

## Regression coverage

`sep-probe/test_sep_operation.c` now injects native rejections at load and
promotion through the real session parser and operation owner. Twelve cases
cover outer and inner rejections with successful cleanup, transport-destroy
failure, or SEP-descriptor-close failure. They verify:

- no readiness publication, notification lease or native retry after rejection;
- the original saved secret/blob is unchanged and persistence is never called;
- session and descriptor/lock cleanup still run;
- the primary remote failure remains the operation result while the separate
  cleanup observer reports success or teardown failure.

The synthetic transport deliberately makes no assertion about firmware handle
allocation or lifetime. These tests protect failure handling; they do not
reproduce the hardware wedge or establish a native fix.

## Next evidence and decision

The cleanup observer landed in `02885e4` and is published in v0.1.10. During
ordinary use on an affected machine, retain the preceding healthy round, first
rejection, subsequent retry, and the corresponding `sep-cleanup` outcomes using
the [opt-in diagnostic procedure](diagnostics.md). Share only allowlisted
stage/status, versions, timing and restart counts. A healthy cleanup outcome
narrows the local teardown question; it does not settle firmware ownership.

Before adding an unload request or skipping restore, establish the T1-specific
load/promote ownership contract from reproducible source/protocol evidence.
Reuse would additionally need to verify that the active bag belongs to the
saved material, reject mismatch/locked/unknown state, and preserve the existing
cold-boot restore path. The existing encoder accepts secret verification with
no ACM external form, but encoder support alone is not hardware validation of
that proposed relay path.

Until those conditions are met, keep the native sequence unchanged. Do not
delete keybags, re-enroll, reset the T1, or manufacture repeated authentication
cycles to test a speculative fix. Downstream lock-screen backoff can reduce
cycling; it cannot establish that the native cause is repaired.

## Passive checkpoint, September 20

A read-only check on MacBookPro13,3, kernel `7.2.3-arch1-3`, core/DKMS
`0.1.9.r8.g995f306-1`, found the relay active with `NRestarts=1` after about
24 hours of host uptime. The boot journal contains one failed lease (`-111`)
with successful cleanup, then another relay start followed by successful
biometric load/promotion. There was no sustained failure restart loop in this
snapshot. No service was restarted and no authentication was initiated.

Some `keystore-outer=-3` records for selector `0x19` are followed by successful
create/promotion and continued startup. Count actual lease failures and
`NRestarts`, not every diagnostic `result=error` as a failed relay round.
This older development build and one machine do not validate v0.1.10 on an
affected reporter's machine. The next needed record is still the first
ordinary-use failing round on that machine, including cleanup and restart
counter changes; do not manufacture it through repeated authentication.
