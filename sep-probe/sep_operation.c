#define _GNU_SOURCE
#define _POSIX_C_SOURCE 200809L

#include "sep_operation.h"

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include <sys/file.h>
#include <sys/random.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>

static _Thread_local sep_operation_cleanup_observer cleanup_observer;

sep_operation_cleanup_observer sep_operation_set_cleanup_observer(
	sep_operation_cleanup_observer observer)
{
	sep_operation_cleanup_observer previous = cleanup_observer;

	cleanup_observer = observer;
	return previous;
}

struct sep_operation {
	const struct sep_operation_ops *ops;
	sep_operation_cancelled cancelled;
	void *cancellation_context;
	struct sep_urb_transport transport;
	struct sep_session session;
	struct sep_acm_state acm;
	uint64_t deadline_ms;
	enum sep_operation_lock_mode lock_mode;
	int lock_descriptor;
	int sep_descriptor;
	bool session_initialized;
	bool acm_initialized;
	bool deadline_active;
	bool lease_active;
};

static int linux_monotonic_ms(void *context, uint64_t *value)
{
	struct timespec now;

	(void)context;
	if (!value || clock_gettime(CLOCK_MONOTONIC, &now) != 0)
		return -1;
	*value = (uint64_t)now.tv_sec * 1000U +
		 (uint64_t)now.tv_nsec / 1000000U;
	return 0;
}

static int linux_entropy(void *context, void *output, size_t length)
{
	uint8_t *bytes = output;
	size_t offset = 0;

	(void)context;
	while (offset < length) {
		ssize_t result = getrandom(bytes + offset, length - offset, 0);

		if (result > 0) {
			offset += (size_t)result;
			continue;
		}
		if (result < 0 && errno == EINTR)
			continue;
		return -1;
	}
	return 0;
}

static int linux_open_lock(void *context, int *file_descriptor)
{
	struct stat status;
	int descriptor;

	(void)context;
	if (!file_descriptor || geteuid() != 0)
		return -1;
	descriptor = open(SEP_OPERATION_LOCK_PATH,
			  O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC | O_NOFOLLOW,
			  0600);
	if (descriptor < 0 && errno == EEXIST)
		descriptor = open(SEP_OPERATION_LOCK_PATH,
				  O_RDWR | O_CLOEXEC | O_NOFOLLOW);
	if (descriptor < 0 || fstat(descriptor, &status) != 0 ||
	    !S_ISREG(status.st_mode) || status.st_uid != 0 ||
	    status.st_nlink != 1 || (status.st_mode & 07777U) != 0600U) {
		if (descriptor >= 0)
			(void)close(descriptor);
		return -1;
	}
	*file_descriptor = descriptor;
	return 0;
}

int sep_operation_try_lock_descriptor(int file_descriptor,
			      enum sep_operation_lock_mode mode)
{
	int operation;

	if (file_descriptor < 0 ||
	    (mode != SEP_OPERATION_LOCK_EXCLUSIVE &&
	     mode != SEP_OPERATION_LOCK_SHARED))
		return -1;
	operation = mode == SEP_OPERATION_LOCK_SHARED ? LOCK_SH : LOCK_EX;
	if (flock(file_descriptor, operation | LOCK_NB) == 0)
		return 0;
	return errno == EWOULDBLOCK || errno == EAGAIN ? 1 : -1;
}

static int linux_try_lock(void *context, int file_descriptor,
			  enum sep_operation_lock_mode mode)
{
	(void)context;
	return sep_operation_try_lock_descriptor(file_descriptor, mode);
}

static int linux_wait_ms(void *context, unsigned int timeout_ms)
{
	int result;

	(void)context;
	do {
		result = poll(NULL, 0, (int)timeout_ms);
	} while (result < 0 && errno == EINTR);
	return result < 0 ? -1 : 0;
}

static enum sep_operation_result linux_open_sep(void *context,
					       unsigned int timeout_ms,
					       int *file_descriptor)
{
	enum sep_usbfs_status status;

	(void)context;
	status = sep_usbfs_open_unique_bounded(timeout_ms, file_descriptor);
	if (status == SEP_USBFS_OK)
		return SEP_OPERATION_OK;
	if (status == SEP_USBFS_TIMED_OUT)
		return SEP_OPERATION_ERROR_TIMEOUT;
	if (status == SEP_USBFS_CLOCK_FAILED)
		return SEP_OPERATION_ERROR_CLOCK;
	return SEP_OPERATION_ERROR_USB;
}

static int linux_close_fd(void *context, int file_descriptor)
{
	(void)context;
	return close(file_descriptor);
}

static int linux_initialize_session(void *context,
				    struct sep_session *session,
				    struct sep_urb_transport *transport,
				    int file_descriptor,
				    uint64_t first_token,
				    uint32_t first_message_index)
{
	enum sep_urb_status status;
	int result;

	(void)context;
	status = sep_urb_transport_initialize(transport, file_descriptor);
	if (status != SEP_URB_OK)
		return -1;
	result = sep_session_init(session, transport, first_token,
				  first_message_index);
	if (result != SEP_SESSION_OK) {
		(void)sep_urb_transport_destroy(transport);
		return -1;
	}
	return 0;
}

static const struct sep_operation_ops linux_ops = {
	.context = NULL,
	.monotonic_ms = linux_monotonic_ms,
	.entropy = linux_entropy,
	.open_lock = linux_open_lock,
	.try_lock = linux_try_lock,
	.wait_ms = linux_wait_ms,
	.open_sep = linux_open_sep,
	.close_fd = linux_close_fd,
	.initialize_session = linux_initialize_session,
};

static bool valid_ops(const struct sep_operation_ops *ops)
{
	return ops && ops->monotonic_ms && ops->entropy && ops->open_lock &&
	       ops->try_lock && ops->wait_ms && ops->open_sep &&
	       ops->close_fd && ops->initialize_session;
}

static bool is_cancelled(const struct sep_operation *operation)
{
	return operation->cancelled &&
	       operation->cancelled(operation->cancellation_context) != 0;
}

static enum sep_operation_result remaining_ms(
	const struct sep_operation *operation, bool check_cancellation,
	unsigned int *remaining)
{
	uint64_t now;
	uint64_t difference;

	if (!operation->deadline_active)
		return SEP_OPERATION_ERROR_STATE;
	if (check_cancellation && is_cancelled(operation))
		return SEP_OPERATION_ERROR_CANCELLED;
	if (operation->ops->monotonic_ms(operation->ops->context, &now) != 0)
		return SEP_OPERATION_ERROR_CLOCK;
	if (now >= operation->deadline_ms)
		return SEP_OPERATION_ERROR_TIMEOUT;
	difference = operation->deadline_ms - now;
	if (difference > SEP_URB_MAX_TIMEOUT_MS)
		difference = SEP_URB_MAX_TIMEOUT_MS;
	*remaining = (unsigned int)difference;
	return SEP_OPERATION_OK;
}

static enum sep_operation_result acquire_lock(struct sep_operation *operation)
{
	for (;;) {
		unsigned int remaining;
		unsigned int wait;
		int lock_result;
		enum sep_operation_result result =
			remaining_ms(operation, true, &remaining);

		if (result != SEP_OPERATION_OK)
			return result;
		lock_result = operation->ops->try_lock(
			operation->ops->context, operation->lock_descriptor,
			operation->lock_mode);
		if (lock_result == 0)
			return SEP_OPERATION_OK;
		if (lock_result < 0)
			return SEP_OPERATION_ERROR_LOCK;
		wait = remaining < SEP_OPERATION_LOCK_RETRY_MS ?
			       remaining : SEP_OPERATION_LOCK_RETRY_MS;
		if (operation->ops->wait_ms(operation->ops->context, wait) != 0)
			return SEP_OPERATION_ERROR_LOCK;
	}
}

static enum sep_operation_result map_session_result(int result)
{
	if (result == SEP_SESSION_OK)
		return SEP_OPERATION_OK;
	if (result == SEP_SESSION_REMOTE_ERROR)
		return SEP_OPERATION_REMOTE_ERROR;
	if (result == SEP_SESSION_IDLE)
		return SEP_OPERATION_IDLE;
	if (result == SEP_SESSION_ERROR_TIMEOUT)
		return SEP_OPERATION_ERROR_TIMEOUT;
	if (result == SEP_SESSION_ERROR_CLOCK)
		return SEP_OPERATION_ERROR_CLOCK;
	return SEP_OPERATION_ERROR_SESSION;
}

enum sep_operation_result sep_operation_drain_notification(
	struct sep_operation *operation, unsigned int timeout_ms)
{
	if (!operation || !operation->session_initialized)
		return SEP_OPERATION_ERROR_ARGUMENT;
	return map_session_result(sep_session_drain_notification(
		&operation->session, timeout_ms));
}

int sep_operation_is_cancelled(const struct sep_operation *operation)
{
	return operation && is_cancelled(operation);
}

enum sep_operation_result sep_operation_keystore_exchange(
	struct sep_operation *operation,
	const struct sep_keystore_operation *request_operation,
	const void *request, size_t request_length,
	struct sep_keystore_reply *reply)
{
	unsigned int remaining;
	enum sep_operation_result result;

	if (!operation || !operation->session_initialized)
		return SEP_OPERATION_ERROR_ARGUMENT;
	result = remaining_ms(operation, true, &remaining);
	if (result != SEP_OPERATION_OK)
		return result;
	return map_session_result(sep_session_keystore_exchange(
		&operation->session, request_operation, request, request_length,
		reply, remaining));
}

enum sep_operation_result sep_operation_entropy(
	struct sep_operation *operation, void *output, size_t length)
{
	enum sep_operation_result result;
	unsigned int remaining;

	if (!operation || !output || length == 0)
		return SEP_OPERATION_ERROR_ARGUMENT;
	result = remaining_ms(operation, true, &remaining);
	if (result != SEP_OPERATION_OK)
		return result;
	(void)remaining;
	return operation->ops->entropy(operation->ops->context, output, length) ==
		       0 ?
		       SEP_OPERATION_OK : SEP_OPERATION_ERROR_ENTROPY;
}

enum sep_operation_result sep_operation_timestamp_us(
	struct sep_operation *operation, uint64_t previous,
	uint64_t *timestamp_us)
{
	uint64_t now;
	uint64_t timestamp;

	if (!operation || !timestamp_us)
		return SEP_OPERATION_ERROR_ARGUMENT;
	if (!operation->deadline_active)
		return SEP_OPERATION_ERROR_STATE;
	if (is_cancelled(operation))
		return SEP_OPERATION_ERROR_CANCELLED;
	if (operation->ops->monotonic_ms(operation->ops->context, &now) != 0)
		return SEP_OPERATION_ERROR_CLOCK;
	if (now >= operation->deadline_ms)
		return SEP_OPERATION_ERROR_TIMEOUT;
	if (now > UINT64_MAX / 1000U)
		return SEP_OPERATION_ERROR_CLOCK;
	timestamp = now * 1000U;
	if (timestamp <= previous) {
		if (previous == UINT64_MAX)
			return SEP_OPERATION_ERROR_CLOCK;
		timestamp = previous + 1U;
	}
	*timestamp_us = timestamp;
	return SEP_OPERATION_OK;
}

enum sep_operation_result sep_operation_release_acquisition_deadline(
	struct sep_operation *operation)
{
	unsigned int remaining;
	enum sep_operation_result result;

	if (!operation)
		return SEP_OPERATION_ERROR_ARGUMENT;
	if (!operation->session_initialized || !operation->deadline_active ||
	    operation->lease_active)
		return SEP_OPERATION_ERROR_STATE;
	result = remaining_ms(operation, true, &remaining);
	if (result != SEP_OPERATION_OK)
		return result;
	(void)remaining;
	result = map_session_result(
		sep_session_release_operation_deadline(&operation->session));
	if (result != SEP_OPERATION_OK)
		return result;
	operation->deadline_ms = 0;
	operation->deadline_active = false;
	operation->lease_active = true;
	return SEP_OPERATION_OK;
}

enum sep_operation_result sep_operation_acm_exchange(
	struct sep_operation *operation, sep_session_acm_builder builder,
	void *builder_context, struct sep_acm_outcome *outcome)
{
	unsigned int remaining;
	enum sep_operation_result result;

	if (!operation || !operation->session_initialized ||
	    !operation->acm_initialized)
		return SEP_OPERATION_ERROR_ARGUMENT;
	result = remaining_ms(operation, true, &remaining);
	if (result != SEP_OPERATION_OK)
		return result;
	return map_session_result(sep_session_acm_exchange(
		&operation->session, &operation->acm, builder, builder_context,
		outcome, remaining));
}

int sep_operation_export_acm_context(
	const struct sep_operation *operation,
	struct sep_acm_external_form *external_form)
{
	if (!operation || !operation->acm_initialized)
		return SEP_ACM_ERROR_ARGUMENT;
	return sep_acm_export_active_context(&operation->acm, external_form);
}

static int build_context_delete(void *context, struct sep_acm_state *state,
				struct sep_relay_state *relay,
				uint8_t output[SEP_RELAY_BUFFER_SIZE])
{
	(void)context;
	return sep_acm_build_context_delete(state, relay, output);
}

static bool close_product_session(struct sep_operation *operation)
{
	bool clean = true;

	if (operation->acm_initialized) {
		if (operation->acm.phase == SEP_ACM_PHASE_ACTIVE) {
			unsigned int remaining;
			struct sep_acm_outcome outcome;
			uint64_t now;

			if (operation->lease_active) {
				if (operation->ops->monotonic_ms(
					    operation->ops->context, &now) != 0 ||
				    UINT64_MAX - now <
					    SEP_OPERATION_CLEANUP_TIMEOUT_MS) {
					clean = false;
				} else {
					operation->deadline_ms =
						now + SEP_OPERATION_CLEANUP_TIMEOUT_MS;
					if (sep_session_set_operation_deadline(
						    &operation->session,
						    operation->deadline_ms) !=
					    SEP_SESSION_OK) {
						clean = false;
					} else {
						operation->deadline_active = true;
						operation->lease_active = false;
					}
				}
			}

			if (!clean ||
			    remaining_ms(operation, false, &remaining) !=
				    SEP_OPERATION_OK ||
			    sep_session_acm_exchange(
				    &operation->session, &operation->acm,
				    build_context_delete, NULL, &outcome,
				    remaining) != SEP_SESSION_OK)
				clean = false;
		}
		sep_acm_state_wipe(&operation->acm);
		operation->acm_initialized = false;
	}
	if (operation->session_initialized) {
		if (sep_session_destroy(&operation->session) != SEP_SESSION_OK)
			clean = false;
		operation->session_initialized = false;
	}
	if (operation->sep_descriptor >= 0) {
		if (operation->ops->close_fd(operation->ops->context,
					     operation->sep_descriptor) != 0)
			clean = false;
		operation->sep_descriptor = -1;
	}
	sep_urb_transport_abandon_after_close(&operation->transport);
	memset(&operation->transport, 0, sizeof(operation->transport));
	memset(&operation->session, 0, sizeof(operation->session));
	operation->deadline_ms = 0;
	operation->deadline_active = false;
	operation->lease_active = false;
	return clean;
}

static bool close_operation_lock(struct sep_operation *operation)
{
	bool clean = true;

	if (operation->lock_descriptor >= 0) {
		if (operation->ops->close_fd(operation->ops->context,
					     operation->lock_descriptor) != 0)
			clean = false;
		operation->lock_descriptor = -1;
	}
	return clean;
}

struct sep_operation_seeds {
	uint64_t first_token;
	uint32_t first_message_index;
};

static void derive_seeds(const uint8_t entropy[12],
			 struct sep_operation_seeds *seeds)
{
	uint64_t token = 0;
	uint32_t index = 0;
	size_t offset;

	for (offset = 0; offset < sizeof(token); ++offset)
		token |= (uint64_t)entropy[offset] << (offset * 8U);
	for (offset = 0; offset < sizeof(index); ++offset)
		index |= (uint32_t)entropy[sizeof(token) + offset] <<
			 (offset * 8U);
	token &= UINT64_C(0x7fffffffffffffff);
	if (token == 0)
		token = 1;
	index &= UINT32_C(0x3fffffff);
	seeds->first_token = token;
	seeds->first_message_index = index;
}

static enum sep_operation_result run_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_preflight preflight, void *preflight_context,
	sep_operation_callback callback, void *callback_context,
	sep_operation_finalizer finalizer, void *finalizer_context,
	enum sep_operation_lock_mode lock_mode)
{
	struct sep_operation operation;
	struct sep_operation_seeds seeds = { 0 };
	uint8_t entropy[12];
	uint64_t now;
	enum sep_operation_result result = SEP_OPERATION_OK;

	if (!valid_ops(ops) || !callback || timeout_ms == 0 ||
	    (lock_mode != SEP_OPERATION_LOCK_EXCLUSIVE &&
	     lock_mode != SEP_OPERATION_LOCK_SHARED))
		return SEP_OPERATION_ERROR_ARGUMENT;
	memset(&operation, 0, sizeof(operation));
	operation.ops = ops;
	operation.cancelled = cancelled;
	operation.cancellation_context = cancellation_context;
	operation.lock_mode = lock_mode;
	operation.lock_descriptor = -1;
	operation.sep_descriptor = -1;
	if (ops->monotonic_ms(ops->context, &now) != 0 ||
	    UINT64_MAX - now < timeout_ms)
		return SEP_OPERATION_ERROR_CLOCK;
	operation.deadline_ms = now + timeout_ms;
	operation.deadline_active = true;

	if (is_cancelled(&operation))
		return SEP_OPERATION_ERROR_CANCELLED;
	if (ops->open_lock(ops->context, &operation.lock_descriptor) != 0 ||
	    operation.lock_descriptor < 0) {
		result = SEP_OPERATION_ERROR_LOCK;
		goto cleanup;
	}
	result = acquire_lock(&operation);
	if (result != SEP_OPERATION_OK)
		goto cleanup;
	result = remaining_ms(&operation, true, &(unsigned int){ 0 });
	if (result != SEP_OPERATION_OK)
		goto cleanup;
	if (preflight) {
		result = preflight(preflight_context);
		if (result != SEP_OPERATION_OK)
			goto cleanup;
		result = remaining_ms(
			&operation, true, &(unsigned int){ 0 });
		if (result != SEP_OPERATION_OK)
			goto cleanup;
	}
	if (ops->entropy(ops->context, entropy, sizeof(entropy)) != 0) {
		result = SEP_OPERATION_ERROR_ENTROPY;
		goto cleanup;
	}
	derive_seeds(entropy, &seeds);
	sep_keystore_clear(entropy, sizeof(entropy));
	{
		unsigned int remaining;

		result = remaining_ms(&operation, true, &remaining);
		if (result == SEP_OPERATION_OK)
			result = ops->open_sep(ops->context, remaining,
					       &operation.sep_descriptor);
	}
	if (result != SEP_OPERATION_OK || operation.sep_descriptor < 0) {
		if (result == SEP_OPERATION_OK)
			result = SEP_OPERATION_ERROR_USB;
		goto cleanup;
	}
	result = remaining_ms(&operation, true, &(unsigned int){ 0 });
	if (result != SEP_OPERATION_OK)
		goto cleanup;
	if (ops->initialize_session(ops->context, &operation.session,
				    &operation.transport,
				    operation.sep_descriptor, seeds.first_token,
				    seeds.first_message_index) != 0) {
		result = SEP_OPERATION_ERROR_SESSION;
		goto cleanup;
	}
	sep_keystore_clear(&seeds, sizeof(seeds));
	operation.session_initialized = true;
	result = map_session_result(sep_session_set_operation_deadline(
		&operation.session, operation.deadline_ms));
	if (result != SEP_OPERATION_OK)
		goto cleanup;
	{
		int session_result = sep_session_negotiate(
			&operation.session, SEP_URB_MAX_TIMEOUT_MS);

		if (session_result != SEP_SESSION_OK)
			fprintf(stderr, "t1bridge SEP negotiation: %s\n",
				sep_session_result_name(session_result));
		result = map_session_result(session_result);
	}
	if (result != SEP_OPERATION_OK)
		goto cleanup;
	if (sep_acm_state_init(&operation.acm) != SEP_ACM_OK) {
		result = SEP_OPERATION_ERROR_ACM;
		goto cleanup;
	}
	operation.acm_initialized = true;
	if (is_cancelled(&operation))
		result = SEP_OPERATION_ERROR_CANCELLED;
	else
		result = callback(callback_context, &operation);
	if (result == SEP_OPERATION_OK && is_cancelled(&operation))
		result = SEP_OPERATION_ERROR_CANCELLED;

cleanup:
	sep_keystore_clear(entropy, sizeof(entropy));
	sep_keystore_clear(&seeds, sizeof(seeds));
	{
		bool clean = close_product_session(&operation);

		if (finalizer)
			finalizer(finalizer_context);
		if (!close_operation_lock(&operation))
			clean = false;
		if (cleanup_observer)
			cleanup_observer(clean ? SEP_OPERATION_OK :
					 SEP_OPERATION_ERROR_TEARDOWN);
		if (!clean &&
		    (result == SEP_OPERATION_OK ||
		     result == SEP_OPERATION_ERROR_CANCELLED))
			result = SEP_OPERATION_ERROR_TEARDOWN;
	}
	return result;
}

enum sep_operation_result sep_operation_run_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_callback callback, void *callback_context)
{
	return run_with_ops(ops, timeout_ms, cancelled, cancellation_context,
			    NULL, NULL, callback, callback_context, NULL, NULL,
			    SEP_OPERATION_LOCK_EXCLUSIVE);
}

enum sep_operation_result sep_operation_run_preflight_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_preflight preflight, void *preflight_context,
	sep_operation_callback callback, void *callback_context)
{
	if (!preflight)
		return SEP_OPERATION_ERROR_ARGUMENT;
	return run_with_ops(
		ops, timeout_ms, cancelled, cancellation_context, preflight,
		preflight_context, callback, callback_context, NULL, NULL,
		SEP_OPERATION_LOCK_EXCLUSIVE);
}

enum sep_operation_result sep_operation_run_preflight_finalized_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_preflight preflight, void *preflight_context,
	sep_operation_callback callback, void *callback_context,
	sep_operation_finalizer finalizer, void *finalizer_context)
{
	if (!preflight || !finalizer)
		return SEP_OPERATION_ERROR_ARGUMENT;
	return run_with_ops(
		ops, timeout_ms, cancelled, cancellation_context, preflight,
		preflight_context, callback, callback_context, finalizer,
		finalizer_context, SEP_OPERATION_LOCK_EXCLUSIVE);
}

enum sep_operation_result sep_operation_run_shared_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_callback callback, void *callback_context)
{
	return run_with_ops(ops, timeout_ms, cancelled, cancellation_context,
			    NULL, NULL, callback, callback_context, NULL, NULL,
			    SEP_OPERATION_LOCK_SHARED);
}

enum sep_operation_result sep_operation_run(
	unsigned int timeout_ms, sep_operation_cancelled cancelled,
	void *cancellation_context, sep_operation_callback callback,
	void *callback_context)
{
	return sep_operation_run_with_ops(
		&linux_ops, timeout_ms, cancelled, cancellation_context,
		callback, callback_context);
}

enum sep_operation_result sep_operation_run_preflight(
	unsigned int timeout_ms, sep_operation_cancelled cancelled,
	void *cancellation_context, sep_operation_preflight preflight,
	void *preflight_context, sep_operation_callback callback,
	void *callback_context)
{
	return sep_operation_run_preflight_with_ops(
		&linux_ops, timeout_ms, cancelled, cancellation_context,
		preflight, preflight_context, callback, callback_context);
}

enum sep_operation_result sep_operation_run_preflight_finalized(
	unsigned int timeout_ms, sep_operation_cancelled cancelled,
	void *cancellation_context, sep_operation_preflight preflight,
	void *preflight_context, sep_operation_callback callback,
	void *callback_context, sep_operation_finalizer finalizer,
	void *finalizer_context)
{
	return sep_operation_run_preflight_finalized_with_ops(
		&linux_ops, timeout_ms, cancelled, cancellation_context,
		preflight, preflight_context, callback, callback_context,
		finalizer, finalizer_context);
}

enum sep_operation_result sep_operation_run_shared(
	unsigned int timeout_ms, sep_operation_cancelled cancelled,
	void *cancellation_context, sep_operation_callback callback,
	void *callback_context)
{
	return sep_operation_run_shared_with_ops(
		&linux_ops, timeout_ms, cancelled, cancellation_context,
		callback, callback_context);
}

struct authorized_operation_context {
	uint32_t audit_uid;
	sep_operation_credential_callback callback;
	void *callback_context;
};

static int build_initialized_acm(void *context,
				 struct sep_acm_state *state,
				 struct sep_relay_state *relay,
				 uint8_t output[SEP_RELAY_BUFFER_SIZE])
{
	(void)context;
	return sep_acm_build_initialize(state, relay, output);
}

static int build_authorized_context(void *context,
				    struct sep_acm_state *state,
				    struct sep_relay_state *relay,
				    uint8_t output[SEP_RELAY_BUFFER_SIZE])
{
	const struct authorized_operation_context *authorized = context;

	return sep_acm_build_context_create(state, relay,
					    authorized->audit_uid, output);
}

static int build_externalized_context(void *context,
				      struct sep_acm_state *state,
				      struct sep_relay_state *relay,
				      uint8_t output[SEP_RELAY_BUFFER_SIZE])
{
	(void)context;
	return sep_acm_build_context_externalize(state, relay, output);
}

static enum sep_operation_result authorized_operation_callback(
	void *context, struct sep_operation *operation)
{
	struct authorized_operation_context *authorized = context;

	return sep_operation_with_authorized_credential(
		operation, authorized->audit_uid, authorized->callback,
		authorized->callback_context);
}

enum sep_operation_result sep_operation_with_authorized_credential(
	struct sep_operation *operation, uint32_t audit_uid,
	sep_operation_credential_callback callback, void *callback_context)
{
	struct authorized_operation_context authorized = {
		.audit_uid = audit_uid,
		.callback = callback,
		.callback_context = callback_context,
	};
	struct sep_acm_external_form credential;
	struct sep_acm_outcome outcome;
	enum sep_operation_result result;

	if (!operation || !callback)
		return SEP_OPERATION_ERROR_ARGUMENT;

	result = sep_operation_acm_exchange(
		operation, build_initialized_acm, NULL, &outcome);
	if (result != SEP_OPERATION_OK)
		return result;
	result = sep_operation_acm_exchange(
		operation, build_authorized_context, &authorized, &outcome);
	if (result != SEP_OPERATION_OK)
		return result;
	result = sep_operation_acm_exchange(
		operation, build_externalized_context, NULL, &outcome);
	if (result != SEP_OPERATION_OK)
		return result;
	if (sep_operation_export_acm_context(operation, &credential) !=
	    SEP_ACM_OK)
		return SEP_OPERATION_ERROR_ACM;
	result = callback(
			 callback_context, credential.bytes,
			 sizeof(credential.bytes)) == 0 ?
			 SEP_OPERATION_OK : SEP_OPERATION_ERROR_CALLBACK;
	sep_acm_external_form_wipe(&credential);
	return result;
}

enum sep_operation_result sep_operation_run_authorized_with_ops(
	const struct sep_operation_ops *ops, unsigned int timeout_ms,
	uint32_t audit_uid, sep_operation_cancelled cancelled,
	void *cancellation_context,
	sep_operation_credential_callback callback, void *callback_context)
{
	struct authorized_operation_context authorized = {
		.audit_uid = audit_uid,
		.callback = callback,
		.callback_context = callback_context,
	};

	if (!callback)
		return SEP_OPERATION_ERROR_ARGUMENT;
	return sep_operation_run_with_ops(
		ops, timeout_ms, cancelled, cancellation_context,
		authorized_operation_callback, &authorized);
}

enum sep_operation_result sep_operation_run_authorized(
	unsigned int timeout_ms, uint32_t audit_uid,
	sep_operation_cancelled cancelled, void *cancellation_context,
	sep_operation_credential_callback callback, void *callback_context)
{
	return sep_operation_run_authorized_with_ops(
		&linux_ops, timeout_ms, audit_uid, cancelled,
		cancellation_context, callback, callback_context);
}

const char *sep_operation_result_name(enum sep_operation_result result)
{
	switch (result) {
	case SEP_OPERATION_OK:
		return "success";
	case SEP_OPERATION_REMOTE_ERROR:
		return "remote operation rejected";
	case SEP_OPERATION_IDLE:
		return "notification relay idle";
	case SEP_OPERATION_ERROR_ARGUMENT:
		return "invalid operation input";
	case SEP_OPERATION_ERROR_CLOCK:
		return "operation clock failed";
	case SEP_OPERATION_ERROR_TIMEOUT:
		return "operation timed out";
	case SEP_OPERATION_ERROR_CANCELLED:
		return "operation cancelled";
	case SEP_OPERATION_ERROR_LOCK:
		return "SEP lock failed";
	case SEP_OPERATION_ERROR_ENTROPY:
		return "kernel entropy failed";
	case SEP_OPERATION_ERROR_USB:
		return "SEP device acquisition failed";
	case SEP_OPERATION_ERROR_SESSION:
		return "SEP product session failed";
	case SEP_OPERATION_ERROR_ACM:
		return "ACM ownership failed";
	case SEP_OPERATION_ERROR_TEARDOWN:
		return "SEP operation teardown failed";
	case SEP_OPERATION_ERROR_CALLBACK:
		return "authorized operation callback failed";
	case SEP_OPERATION_ERROR_KEYBAG:
		return "keybag operation failed";
	case SEP_OPERATION_ERROR_PERSISTENCE:
		return "keybag persistence failed";
	case SEP_OPERATION_ERROR_STATE:
		return "keybag state was unavailable or unsafe";
	default:
		return "unknown SEP operation failure";
	}
}
