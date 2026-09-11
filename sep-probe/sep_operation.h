#ifndef T1BRIDGE_SEP_OPERATION_H
#define T1BRIDGE_SEP_OPERATION_H

#include "sep_session.h"

#include <stddef.h>
#include <stdint.h>

#define SEP_OPERATION_LOCK_PATH "/run/lock/t1-touchid-sep.lock"
#define SEP_OPERATION_LOCK_RETRY_MS 25U
#define SEP_OPERATION_CLEANUP_TIMEOUT_MS 30000U

enum sep_operation_result {
	SEP_OPERATION_OK = 0,
	SEP_OPERATION_REMOTE_ERROR = 1,
	SEP_OPERATION_IDLE = 2,
	SEP_OPERATION_ERROR_ARGUMENT = -100,
	SEP_OPERATION_ERROR_CLOCK = -101,
	SEP_OPERATION_ERROR_TIMEOUT = -102,
	SEP_OPERATION_ERROR_CANCELLED = -103,
	SEP_OPERATION_ERROR_LOCK = -104,
	SEP_OPERATION_ERROR_ENTROPY = -105,
	SEP_OPERATION_ERROR_USB = -106,
	SEP_OPERATION_ERROR_SESSION = -107,
	SEP_OPERATION_ERROR_ACM = -108,
	SEP_OPERATION_ERROR_TEARDOWN = -109,
	SEP_OPERATION_ERROR_CALLBACK = -110,
	SEP_OPERATION_ERROR_KEYBAG = -111,
	SEP_OPERATION_ERROR_PERSISTENCE = -112,
	SEP_OPERATION_ERROR_STATE = -113,
};

enum sep_operation_lock_mode {
	SEP_OPERATION_LOCK_EXCLUSIVE = 0,
	SEP_OPERATION_LOCK_SHARED = 1,
};

struct sep_operation;

typedef int (*sep_operation_cancelled)(void *context);
typedef enum sep_operation_result (*sep_operation_preflight)(void *context);
typedef void (*sep_operation_finalizer)(void *context);
typedef enum sep_operation_result (*sep_operation_callback)(
	void *context, struct sep_operation *operation);
typedef int (*sep_operation_credential_callback)(
	void *context,
	const uint8_t credential[SEP_ACM_EXTERNAL_FORM_SIZE],
	size_t credential_length);

/*
 * Optional synchronous, thread-local cleanup observation. Receives only OK or
 * ERROR_TEARDOWN after owned resources and the lock have been closed. It must
 * not reenter native operations or unwind. The setter returns the prior hook.
 * The operation's primary result and teardown precedence remain unchanged.
 */
typedef void (*sep_operation_cleanup_observer)(int result);
sep_operation_cleanup_observer sep_operation_set_cleanup_observer(
	sep_operation_cleanup_observer observer);

/* Focused syscall/session seam for deterministic native tests. */
struct sep_operation_ops {
	void *context;
	int (*monotonic_ms)(void *context, uint64_t *value);
	int (*entropy)(void *context, void *output, size_t length);
	int (*open_lock)(void *context, int *file_descriptor);
	/* Zero acquired, one busy, negative failure. */
	int (*try_lock)(void *context, int file_descriptor,
			enum sep_operation_lock_mode mode);
	int (*wait_ms)(void *context, unsigned int timeout_ms);
	enum sep_operation_result (*open_sep)(void *context,
					      unsigned int timeout_ms,
					      int *file_descriptor);
	/* The descriptor is consumed even when the close operation reports error. */
	int (*close_fd)(void *context, int file_descriptor);
	int (*initialize_session)(void *context, struct sep_session *session,
				  struct sep_urb_transport *transport,
				  int file_descriptor, uint64_t first_token,
				  uint32_t first_message_index);
};

/*
 * Runs one non-reusable operation under the fixed exclusive SEP lock. The
 * callback borrows the operation only for the call and must not retain it.
 */
enum sep_operation_result sep_operation_run(
	unsigned int timeout_ms, sep_operation_cancelled cancelled,
	void *cancellation_context, sep_operation_callback callback,
	void *callback_context);

enum sep_operation_result sep_operation_run_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_callback callback, void *callback_context);

/*
 * Runs preflight exactly once after acquiring the exclusive lock but before
 * entropy, SEP-device acquisition, or protocol negotiation. Preflight and the
 * product callback share the operation's one monotonic deadline.
 */
enum sep_operation_result sep_operation_run_preflight(
	unsigned int timeout_ms, sep_operation_cancelled cancelled,
	void *cancellation_context, sep_operation_preflight preflight,
	void *preflight_context, sep_operation_callback callback,
	void *callback_context);

enum sep_operation_result sep_operation_run_preflight_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_preflight preflight, void *preflight_context,
	sep_operation_callback callback, void *callback_context);

/* Finalizer runs after ACM/SEP cleanup and before the fixed lock is closed. */
enum sep_operation_result sep_operation_run_preflight_finalized(
	unsigned int timeout_ms, sep_operation_cancelled cancelled,
	void *cancellation_context, sep_operation_preflight preflight,
	void *preflight_context, sep_operation_callback callback,
	void *callback_context, sep_operation_finalizer finalizer,
	void *finalizer_context);

enum sep_operation_result sep_operation_run_preflight_finalized_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_preflight preflight, void *preflight_context,
	sep_operation_callback callback, void *callback_context,
	sep_operation_finalizer finalizer, void *finalizer_context);

enum sep_operation_result sep_operation_run_shared(
	unsigned int timeout_ms, sep_operation_cancelled cancelled,
	void *cancellation_context, sep_operation_callback callback,
	void *callback_context);

enum sep_operation_result sep_operation_run_shared_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_callback callback, void *callback_context);

/* Focused lock primitive used by production and native concurrency tests. */
int sep_operation_try_lock_descriptor(int file_descriptor,
			      enum sep_operation_lock_mode mode);

/*
 * Runs one exclusive product session, creates and externalizes an authorized
 * ACM context for audit_uid, and lends the fixed credential only for callback.
 * Native cleanup destroys the ACM context and wipes the borrowed credential
 * before return on success, callback failure, cancellation, or teardown.
 */
enum sep_operation_result sep_operation_run_authorized(
	unsigned int timeout_ms, uint32_t audit_uid,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_credential_callback callback, void *callback_context);

enum sep_operation_result sep_operation_run_authorized_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	uint32_t audit_uid, sep_operation_cancelled cancelled,
	void *cancellation_context,
	sep_operation_credential_callback callback, void *callback_context);

/* Request/reply exchanges are pinned to the acquisition deadline. */
enum sep_operation_result sep_operation_keystore_exchange(
	struct sep_operation *operation,
	const struct sep_keystore_operation *request_operation,
	const void *request, size_t request_length,
	struct sep_keystore_reply *reply);

enum sep_operation_result sep_operation_entropy(
	struct sep_operation *operation, void *output, size_t length);

enum sep_operation_result sep_operation_timestamp_us(
	struct sep_operation *operation, uint64_t previous,
	uint64_t *timestamp_us);

/*
 * Ends the bounded acquisition phase after its final successful SEP exchange.
 * The borrowed lease may then remain live under caller cancellation without an
 * acquisition deadline. No further product exchange is accepted until native
 * cleanup installs its fresh bounded deadline.
 */
enum sep_operation_result sep_operation_release_acquisition_deadline(
	struct sep_operation *operation);

enum sep_operation_result sep_operation_with_authorized_credential(
	struct sep_operation *operation, uint32_t audit_uid,
	sep_operation_credential_callback callback, void *callback_context);

enum sep_operation_result sep_operation_acm_exchange(
	struct sep_operation *operation, sep_session_acm_builder builder,
	void *builder_context, struct sep_acm_outcome *outcome);

/* Receive and validate one notification, or return SEP_OPERATION_IDLE. */
enum sep_operation_result sep_operation_drain_notification(
	struct sep_operation *operation, unsigned int timeout_ms);

int sep_operation_is_cancelled(const struct sep_operation *operation);

int sep_operation_export_acm_context(
	const struct sep_operation *operation,
	struct sep_acm_external_form *external_form);

const char *sep_operation_result_name(enum sep_operation_result result);

#endif
