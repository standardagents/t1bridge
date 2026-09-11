#define _GNU_SOURCE

#include "sep_operation.h"
#include "sep_keybag.h"

#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#define HEADER_OFFSET SEP_RELAY_MESSAGE_OFFSET
#define REPLY_TOKEN_OFFSET 0x8cU
#define REQUEST_TOKEN_OFFSET 0x98U
#define MESSAGE_LENGTH_OFFSET 0xa4U
#define DATA_LENGTH_OFFSET 0xa8U
#define IPC_FIRST_ARGUMENT_OFFSET 0x60U
#define IPC_SECOND_ARGUMENT_OFFSET 0x64U
#define IPC_LENGTH_OFFSET 0x68U
#define IPC_BYTES_OFFSET 0x6cU
#define ACM_INITIALIZE UINT8_C(0x0a)
#define ACM_CONTEXT_CREATE UINT8_C(0x01)
#define ACM_CONTEXT_DELETE UINT8_C(0x02)
#define ACM_CONTEXT_VERIFY_POLICY UINT8_C(0x03)
#define ACM_CONTEXT_CONTAINS_CREDENTIAL UINT8_C(0x04)
#define ACM_CONTEXT_REPLACE_PASSPHRASE UINT8_C(0x0f)
#define ACM_CONTEXT_GET_EXTERNAL_FORM UINT8_C(0x13)
#define VERIFY_SECRET_BASE_SIZE 0x68U

enum authorization_event {
	AUTH_ACM_INITIALIZE = 1,
	AUTH_ACM_CREATE,
	AUTH_ACM_EXTERNALIZE,
	AUTH_VERIFY_SECRET,
	AUTH_COPY_UUID,
	AUTH_CONTAINS_TYPE_1_BEFORE,
	AUTH_CONTAINS_TYPE_2_BEFORE,
	AUTH_REPLACE_PASSPHRASE,
	AUTH_CONTAINS_TYPE_1_AFTER,
	AUTH_CONTAINS_TYPE_2_AFTER,
	AUTH_VERIFY_UNBOUND,
	AUTH_VERIFY_BOUND,
};

struct fake_operation {
	uint64_t now;
	uint64_t expire_at;
	unsigned int expire_after_transfer;
	unsigned int transfer_advance_ms;
	unsigned int clock_calls;
	unsigned int clock_fail_call;
	unsigned int entropy_calls;
	unsigned int entropy_generation;
	int entropy_fails;
	unsigned int open_lock_calls;
	int open_lock_fails;
	unsigned int lock_attempts;
	enum sep_operation_lock_mode lock_modes[4];
	unsigned int lock_busy_attempts;
	int lock_fails;
	unsigned int wait_calls;
	unsigned int waited_ms;
	int wait_fails;
	unsigned int open_sep_calls;
	unsigned int open_sep_timeout_ms;
	enum sep_operation_result open_sep_result;
	unsigned int initialize_calls;
	int initialize_fails;
	uint64_t tokens[4];
	uint32_t message_indexes[4];
	unsigned int close_calls;
	int closed[4];
	int close_fails_for;
	unsigned int destroy_calls;
	int destroy_fails;
	unsigned int exchange_calls;
	unsigned int delete_exchange_calls;
	unsigned int delete_timeout_ms;
	int delete_exchange_fails;
	unsigned int send_calls;
	unsigned int receive_calls;
	unsigned int notification_count;
	unsigned int cancel_after_receive;
	unsigned int interrupt_receive_call;
	int malformed_notification;
	unsigned int transfer_calls;
	unsigned int last_timeout_ms;
	int timeout_order_valid;
	unsigned int cancellation_checks;
	unsigned int cancel_on_check;
	int cancelled;
	uint8_t keystore_selectors[32];
	unsigned int keystore_calls;
	unsigned int malformed_lock_state_call;
	uint8_t created_keybag_secret[SEP_KEYSTORE_SECRET_SIZE];
	int device_keybag_present;
	int default_system_keybag_present;
	int device_keybag_bad_state;
	int default_system_keybag_bad_state;
	int biometric_keybag_missing;
	int biometric_keybag_bad_state;
	uint8_t expected_persistent_secret[SEP_KEYSTORE_SECRET_SIZE];
	uint8_t authorization_events[16];
	unsigned int authorization_event_count;
	int passphrase_replaced;
	int keybag_authorization;
};

struct callback_state {
	enum sep_operation_result result;
	unsigned int calls;
	int set_cancelled;
	int expire_budget;
	int initialize_acm;
	int acm_succeeded;
	int export_succeeded;
	int release_acquisition;
	int release_succeeded;
	int attempt_exchange_during_lease;
	enum sep_operation_result lease_exchange_result;
	uint64_t lease_advance_ms;
	struct fake_operation *fake;
};

struct preflight_state {
	struct fake_operation *fake;
	enum sep_operation_result result;
	unsigned int calls;
	unsigned int advance_ms;
};

struct credential_callback_state {
	unsigned int calls;
	size_t length;
	uint8_t credential[SEP_ACM_EXTERNAL_FORM_SIZE];
	int fail;
};

struct fake_keybag_store {
	int load_result;
	int persist_fails;
	unsigned int load_calls;
	unsigned int persist_calls;
	struct sep_keybag_material loaded;
	struct sep_keybag_material persisted;
};

struct prepared_keybag_state;

struct keybag_callback_state {
	unsigned int calls;
	enum sep_keybag_disposition disposition;
	enum sep_keybag_authorization authorization;
	uint8_t credential[SEP_ACM_EXTERNAL_FORM_SIZE];
	int fail;
	struct fake_keybag_store *store;
	struct fake_operation *fake;
	struct prepared_keybag_state *prepared;
	uint64_t lease_advance_ms;
};

struct prepared_keybag_state {
	struct fake_operation *fake;
	struct fake_keybag_store *store;
	unsigned int prepare_calls;
	unsigned int cleanup_calls;
	int prepare_fails;
	int expect_session_cleanup;
};

struct relay_callback_state {
	unsigned int calls;
	int fail;
	struct fake_operation *fake;
	struct fake_keybag_store *store;
	const uint8_t *expected_selectors;
	size_t expected_selector_count;
};

static unsigned int failures;

#define EXPECT(condition) test_expect((condition), #condition, __LINE__)

static void test_expect(int condition, const char *expression, int line)
{
	if (!condition) {
		fprintf(stderr, "line %d: failed: %s\n", line, expression);
		++failures;
	}
}

static uint32_t load_u32_le(const uint8_t *input)
{
	return (uint32_t)input[0] | (uint32_t)input[1] << 8 |
	       (uint32_t)input[2] << 16 | (uint32_t)input[3] << 24;
}

static uint64_t load_u64_le(const uint8_t *input)
{
	return (uint64_t)load_u32_le(input) |
	       (uint64_t)load_u32_le(input + sizeof(uint32_t)) << 32;
}

static void store_u32_le(uint8_t *output, uint32_t value)
{
	output[0] = (uint8_t)value;
	output[1] = (uint8_t)(value >> 8);
	output[2] = (uint8_t)(value >> 16);
	output[3] = (uint8_t)(value >> 24);
}

static void store_u16_le(uint8_t *output, uint16_t value)
{
	output[0] = (uint8_t)value;
	output[1] = (uint8_t)(value >> 8);
}

static void store_u64_le(uint8_t *output, uint64_t value)
{
	store_u32_le(output, (uint32_t)value);
	store_u32_le(output + sizeof(uint32_t), (uint32_t)(value >> 32));
}

static int bytes_are_zero(const uint8_t *bytes, size_t length)
{
	for (size_t index = 0; index < length; ++index) {
		if (bytes[index] != 0)
			return 0;
	}
	return 1;
}

static void record_authorization(struct fake_operation *fake,
				 enum authorization_event event)
{
	if (fake->authorization_event_count <
	    sizeof(fake->authorization_events))
		fake->authorization_events[fake->authorization_event_count] =
			(uint8_t)event;
	++fake->authorization_event_count;
}

static const uint8_t acm_external_form[SEP_ACM_EXTERNAL_FORM_SIZE] = {
	1, 2, 3, 4, 5, 6, 7, 8,
	9, 10, 11, 12, 13, 14, 15, 16,
};

static const uint8_t biometric_keybag_uuid[SEP_KEYSTORE_UUID_SIZE] = {
	0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
	0xa8, 0xa9, 0xaa, 0xab, 0xac, 0xad, 0xae, 0xaf,
};

static size_t build_device_state(uint8_t output[32], int bad_state)
{
	const uint8_t state[] = {
		0x0c, 0x02, 's', 's', 0x02, 0x01,
		bad_state ? 0x00 : 0x04,
		0x0c, 0x03, 's', 'l', 's', 0x02, 0x01, 0x00,
	};

	store_u32_le(output, sizeof(state));
	memcpy(output + sizeof(uint32_t), state, sizeof(state));
	return sizeof(uint32_t) + sizeof(state);
}

static void build_response(uint8_t output[SEP_RELAY_BUFFER_SIZE],
			   uint32_t endpoint, const void *message,
			   size_t message_length, const void *data,
			   size_t data_length, uint64_t reply_token)
{
	uint8_t *header = output + HEADER_OFFSET;

	memset(output, 0, SEP_RELAY_BUFFER_SIZE);
	if (data_length != 0)
		memcpy(output, data, data_length);
	header[0] = SEP_RELAY_WIRE_VERSION;
	store_u32_le(header + 4, endpoint);
	if (message_length != 0)
		memcpy(header + 8, message, message_length);
	store_u64_le(header + REPLY_TOKEN_OFFSET, reply_token);
	store_u32_le(header + MESSAGE_LENGTH_OFFSET, (uint32_t)message_length);
	store_u32_le(header + DATA_LENGTH_OFFSET, (uint32_t)data_length);
}

static void finish_transfer(struct fake_operation *fake,
			    unsigned int timeout_ms)
{
	if (timeout_ms == 0 ||
	    (fake->last_timeout_ms != 0 && timeout_ms > fake->last_timeout_ms))
		fake->timeout_order_valid = 0;
	fake->last_timeout_ms = timeout_ms;
	++fake->transfer_calls;
	if (fake->expire_after_transfer != 0 &&
	    fake->transfer_calls == fake->expire_after_transfer)
		fake->now = fake->expire_at;
	else
		fake->now += fake->transfer_advance_ms;
}

static int exact_protected_configuration(
	const struct fake_operation *fake, const uint8_t *configuration,
	size_t length)
{
	uint8_t expected[68] = { 0 };
	const uint8_t user_uuid[SEP_KEYSTORE_UUID_SIZE] = {
		0xff, 0xff, 0xee, 0xee, 0xdd, 0xdd, 0xcc, 0xcc,
		0xbb, 0xbb, 0xaa, 0xaa, 0x00, 0x00, 0x01, 0xf5,
	};

	expected[0] = 0x31;
	expected[1] = 0x42;
	expected[2] = 0x30;
	expected[3] = 0x25;
	expected[4] = 0x0c;
	expected[5] = 0x01;
	expected[6] = 'p';
	expected[7] = 0x04;
	expected[8] = SEP_KEYSTORE_SECRET_SIZE;
	memcpy(expected + 9, fake->created_keybag_secret,
	       sizeof(fake->created_keybag_secret));
	expected[41] = 0x30;
	expected[42] = 0x19;
	expected[43] = 0x0c;
	expected[44] = 0x05;
	memcpy(expected + 45, "uuuid", 5);
	expected[50] = 0x04;
	expected[51] = SEP_KEYSTORE_UUID_SIZE;
	memcpy(expected + 52, user_uuid, sizeof(user_uuid));
	return length == sizeof(expected) &&
	       memcmp(configuration, expected, sizeof(expected)) == 0;
}

static enum sep_urb_status fake_exchange(void *context,
					 const uint8_t *output,
					 size_t output_length, uint8_t *input,
					 size_t input_capacity,
					 unsigned int timeout_ms)
{
	struct fake_operation *fake = context;
	const uint8_t *header = output + HEADER_OFFSET;
	uint32_t endpoint = load_u32_le(header + 4);
	uint64_t token = load_u64_le(header + REQUEST_TOKEN_OFFSET);

	EXPECT(output_length == SEP_RELAY_BUFFER_SIZE &&
	       input_capacity == SEP_RELAY_BUFFER_SIZE);
	++fake->exchange_calls;
	if (endpoint == SEP_RELAY_CONTROL_ENDPOINT) {
		if (header[8] == 1) {
			const uint8_t message[] = { 1 };

			build_response(input, endpoint, message, sizeof(message), NULL, 0,
				       token);
		} else {
			uint8_t message[2 + SEP_RELAY_ENDPOINT_COUNT] = {
				6, SEP_RELAY_ENDPOINT_COUNT,
			};

			message[2 + SEP_RELAY_ACM_ENDPOINT] = 1;
			message[2 + SEP_RELAY_KEYSTORE_ENDPOINT] = 1;
			build_response(input, endpoint, message, sizeof(message), NULL, 0,
				       token);
		}
	} else if (endpoint == SEP_RELAY_ACM_ENDPOINT) {
		uint8_t message[10] = { 0 };
		uint8_t create_reply[SEP_ACM_CONTEXT_CREATE_REPLY_SIZE] = { 0 };
		uint8_t boolean_reply[sizeof(uint32_t)] = { 0 };
		uint8_t selector = output[4];
		const void *data = NULL;
		size_t data_length = 0;

		EXPECT(endpoint == SEP_RELAY_ACM_ENDPOINT);
		if (selector == ACM_CONTEXT_DELETE) {
			++fake->delete_exchange_calls;
			fake->delete_timeout_ms = timeout_ms;
			if (fake->delete_exchange_fails) {
				finish_transfer(fake, timeout_ms);
				return SEP_URB_TRANSFER_FAILED;
			}
			EXPECT(memcmp(output + SEP_ACM_COMMAND_HEADER_SIZE,
				      acm_external_form,
				      sizeof(acm_external_form)) == 0);
		} else if (selector == ACM_INITIALIZE) {
			if (fake->keybag_authorization)
				record_authorization(fake, AUTH_ACM_INITIALIZE);
			EXPECT(output[5] == 0x28);
		} else if (selector == ACM_CONTEXT_CREATE) {
			if (fake->keybag_authorization) {
				record_authorization(fake, AUTH_ACM_CREATE);
				EXPECT(load_u32_le(
					       output + SEP_ACM_COMMAND_HEADER_SIZE) == 501);
			}
			memcpy(create_reply, acm_external_form,
			       sizeof(acm_external_form));
			create_reply[SEP_ACM_EXTERNAL_FORM_SIZE] = 0x28;
			data = create_reply;
			data_length = sizeof(create_reply);
		} else if (selector == ACM_CONTEXT_GET_EXTERNAL_FORM) {
			if (fake->keybag_authorization)
				record_authorization(fake, AUTH_ACM_EXTERNALIZE);
			EXPECT(memcmp(output + SEP_ACM_COMMAND_HEADER_SIZE,
				      acm_external_form,
				      sizeof(acm_external_form)) == 0);
		} else if (selector == ACM_CONTEXT_CONTAINS_CREDENTIAL) {
			uint32_t credential_type = load_u32_le(
				output + SEP_ACM_COMMAND_HEADER_SIZE +
				SEP_ACM_EXTERNAL_FORM_SIZE);

			EXPECT(memcmp(output + SEP_ACM_COMMAND_HEADER_SIZE,
				      acm_external_form,
				      sizeof(acm_external_form)) == 0);
			EXPECT(load_u32_le(output + SEP_ACM_COMMAND_HEADER_SIZE +
					   SEP_ACM_EXTERNAL_FORM_SIZE +
					   sizeof(uint32_t)) == 0);
			EXPECT(credential_type == 1 || credential_type == 2);
			if (credential_type == 1)
				record_authorization(
					fake, fake->passphrase_replaced ?
						      AUTH_CONTAINS_TYPE_1_AFTER :
						      AUTH_CONTAINS_TYPE_1_BEFORE);
			else
				record_authorization(
					fake, fake->passphrase_replaced ?
						      AUTH_CONTAINS_TYPE_2_AFTER :
						      AUTH_CONTAINS_TYPE_2_BEFORE);
			store_u32_le(boolean_reply,
				     credential_type == 2 &&
					     fake->passphrase_replaced);
			data = boolean_reply;
			data_length = sizeof(boolean_reply);
		} else if (selector == ACM_CONTEXT_REPLACE_PASSPHRASE) {
			const uint8_t *credential =
				output + SEP_ACM_COMMAND_HEADER_SIZE +
				SEP_ACM_EXTERNAL_FORM_SIZE;

			record_authorization(fake, AUTH_REPLACE_PASSPHRASE);
			EXPECT(memcmp(output + SEP_ACM_COMMAND_HEADER_SIZE,
				      acm_external_form,
				      sizeof(acm_external_form)) == 0);
			EXPECT(load_u32_le(credential) == 2 &&
			       load_u32_le(credential + 4U) == 1 &&
			       load_u32_le(credential + 0x24U) ==
				       SEP_KEYSTORE_SECRET_SIZE);
			EXPECT(memcmp(credential + 0x28U,
				      fake->expected_persistent_secret,
				      SEP_KEYSTORE_SECRET_SIZE) == 0);
			fake->passphrase_replaced = 1;
		} else if (selector == ACM_CONTEXT_VERIFY_POLICY) {
			const uint8_t *payload =
				output + SEP_ACM_COMMAND_HEADER_SIZE;
			const size_t preflight_offset =
				SEP_ACM_EXTERNAL_FORM_SIZE + 18U;
			const size_t count_offset = preflight_offset + 5U;
			uint8_t preflight = payload[preflight_offset];

			EXPECT(memcmp(payload, acm_external_form,
				      sizeof(acm_external_form)) == 0);
			EXPECT(memcmp(payload + SEP_ACM_EXTERNAL_FORM_SIZE,
				      "TouchIdEnrollment", 18) == 0);
			if (preflight == 1) {
				record_authorization(fake, AUTH_VERIFY_UNBOUND);
				EXPECT(load_u32_le(payload + count_offset) == 0);
			} else {
				const size_t parameter_offset = 0x2bU;

				record_authorization(fake, AUTH_VERIFY_BOUND);
				EXPECT(preflight == 0 &&
				       load_u32_le(payload + count_offset) == 1 &&
				       load_u32_le(payload + parameter_offset) == 2 &&
				       load_u32_le(payload + parameter_offset + 4U) ==
					       SEP_KEYSTORE_UUID_SIZE);
				EXPECT(memcmp(payload + parameter_offset + 8U,
					      biometric_keybag_uuid,
					      sizeof(biometric_keybag_uuid)) == 0);
			}
			store_u32_le(boolean_reply, 1);
			data = boolean_reply;
			data_length = sizeof(boolean_reply);
		} else {
			EXPECT(0);
		}
		message[1] = SEP_ACM_RELAY_REQUEST;
		store_u32_le(message + 2, (uint32_t)data_length);
		build_response(input, endpoint, message, sizeof(message), data,
			       data_length, 0);
	} else {
		uint8_t message[8] = { 0 };
		uint8_t ipc[128] = { 0 };
		uint8_t typed[32] = { 0 };
		uint8_t selector = header[9];
		int32_t inner_result = 0;
		size_t typed_length = 0;
		size_t ipc_length;

		EXPECT(endpoint == SEP_RELAY_KEYSTORE_ENDPOINT);
		if (fake->keystore_calls < sizeof(fake->keystore_selectors))
			fake->keystore_selectors[fake->keystore_calls] = selector;
		++fake->keystore_calls;
		if (selector == SEP_KEYSTORE_SELECTOR_SET_ENVIRONMENT) {
			const size_t environment_offset = 0x64U;

			EXPECT(load_u32_le(header + DATA_LENGTH_OFFSET) ==
			       environment_offset + SEP_KEYSTORE_ENVIRONMENT_SIZE);
			EXPECT(load_u64_le(output + 0x58U) == 1 &&
			       load_u32_le(output + IPC_FIRST_ARGUMENT_OFFSET) ==
				       SEP_KEYSTORE_ENVIRONMENT_SIZE);
			EXPECT(load_u32_le(output + environment_offset) == 1 &&
			       load_u32_le(output + environment_offset + 4U) == 0 &&
			       load_u32_le(output + environment_offset + 8U) == 0 &&
			       load_u64_le(output + environment_offset + 0xcU) == 0);
			EXPECT(bytes_are_zero(
				       output + environment_offset + 0x14U,
				       SEP_KEYSTORE_ENVIRONMENT_SIZE - 0x14U));
		} else if (selector == SEP_KEYSTORE_SELECTOR_DEVICE_STATE) {
			int32_t handle =
				(int32_t)load_u32_le(output +
						     IPC_FIRST_ARGUMENT_OFFSET);
			int present;
			int bad_state;

			EXPECT(load_u32_le(output + IPC_SECOND_ARGUMENT_OFFSET) == 0);
			if (handle == 0) {
				present = fake->device_keybag_present;
				bad_state = fake->device_keybag_bad_state;
			} else if (handle == -3) {
				EXPECT(handle == -3);
				present = fake->default_system_keybag_present;
				bad_state = fake->default_system_keybag_bad_state;
			} else {
				EXPECT(handle == SEP_KEYBAG_BIOMETRIC_HANDLE);
				present = !fake->biometric_keybag_missing;
				bad_state = fake->biometric_keybag_bad_state;
			}
			if (present)
				typed_length = build_device_state(typed, bad_state);
			else
				inner_result = -3;
		} else if (selector == SEP_KEYSTORE_SELECTOR_CREATE) {
			EXPECT(load_u32_le(output + IPC_LENGTH_OFFSET) ==
			       SEP_KEYSTORE_SECRET_SIZE);
			memcpy(fake->created_keybag_secret,
			       output + IPC_BYTES_OFFSET,
			       sizeof(fake->created_keybag_secret));
			store_u32_le(typed, 23);
			typed_length = sizeof(uint32_t);
		} else if (selector == SEP_KEYSTORE_SELECTOR_LOAD) {
			store_u32_le(typed, 23);
			typed_length = sizeof(uint32_t);
		} else if (selector == SEP_KEYSTORE_SELECTOR_MAKE_SYSTEM) {
			int32_t target =
				(int32_t)load_u32_le(output +
						     IPC_SECOND_ARGUMENT_OFFSET);

			EXPECT(load_u32_le(output + IPC_FIRST_ARGUMENT_OFFSET) == 23);
			if (target == 0) {
				EXPECT(load_u32_le(output + IPC_LENGTH_OFFSET) ==
				       SEP_KEYSTORE_SECRET_SIZE);
				EXPECT(memcmp(
					       output + IPC_BYTES_OFFSET,
					       fake->created_keybag_secret,
					       sizeof(fake->created_keybag_secret)) == 0);
				fake->device_keybag_present = 1;
			} else if (target == -3) {
				EXPECT(load_u32_le(output + IPC_LENGTH_OFFSET) == 0);
				fake->default_system_keybag_present = 1;
			} else {
				EXPECT(target == SEP_KEYBAG_BIOMETRIC_HANDLE);
				if (fake->keystore_selectors[fake->keystore_calls - 2U] ==
				    SEP_KEYSTORE_SELECTOR_CREATE) {
					EXPECT(load_u32_le(output + IPC_LENGTH_OFFSET) ==
					       SEP_KEYSTORE_SECRET_SIZE);
					EXPECT(memcmp(
						       output + IPC_BYTES_OFFSET,
						       fake->created_keybag_secret,
						       sizeof(fake->created_keybag_secret)) == 0);
					memcpy(fake->expected_persistent_secret,
					       fake->created_keybag_secret,
					       sizeof(fake->expected_persistent_secret));
				} else {
					EXPECT(load_u32_le(output + IPC_LENGTH_OFFSET) == 0);
				}
			}
		} else if (selector == SEP_KEYSTORE_SELECTOR_GET_CONFIGURATION) {
			EXPECT((int32_t)load_u32_le(
				       output + IPC_FIRST_ARGUMENT_OFFSET) ==
			       SEP_KEYBAG_BIOMETRIC_HANDLE);
			store_u32_le(typed, 0);
			typed_length = sizeof(uint32_t);
		} else if (selector == SEP_KEYSTORE_SELECTOR_UNLOCK) {
			int32_t handle =
				(int32_t)load_u32_le(output +
						     IPC_FIRST_ARGUMENT_OFFSET);

			EXPECT(handle == -3 ||
			       handle == SEP_KEYBAG_BIOMETRIC_HANDLE);
			if (handle == -3)
				EXPECT(memcmp(
					       output + 0x74U,
					       fake->created_keybag_secret,
					       sizeof(fake->created_keybag_secret)) == 0);
			store_u64_le(typed, 11);
			store_u64_le(typed + sizeof(uint64_t), 12);
			typed_length = 2U * sizeof(uint64_t);
		} else if (selector == SEP_KEYSTORE_SELECTOR_LOCK_STATE) {
			store_u32_le(typed, 4);
			if (fake->malformed_lock_state_call !=
			    fake->keystore_calls) {
				store_u64_le(typed + sizeof(uint32_t), 0);
				typed_length =
					sizeof(uint32_t) + sizeof(uint64_t);
			} else {
				typed_length = sizeof(uint32_t);
			}
		} else if (selector == SEP_KEYSTORE_SELECTOR_SET_CONFIGURATION) {
			EXPECT(load_u32_le(output + IPC_FIRST_ARGUMENT_OFFSET) ==
			       (uint32_t)SEP_KEYBAG_BIOMETRIC_HANDLE);
			EXPECT(load_u32_le(output + IPC_SECOND_ARGUMENT_OFFSET) == 2);
			EXPECT(exact_protected_configuration(
				       fake, output + IPC_BYTES_OFFSET,
				       load_u32_le(output + IPC_LENGTH_OFFSET)));
			store_u64_le(typed, 13);
			store_u64_le(typed + sizeof(uint64_t), 14);
			typed_length = 2U * sizeof(uint64_t);
		} else if (selector == SEP_KEYSTORE_SELECTOR_SERIALIZE) {
			const uint8_t blob[] = { 9, 8, 7 };

			store_u32_le(typed, sizeof(blob));
			memcpy(typed + sizeof(uint32_t), blob, sizeof(blob));
			typed_length = sizeof(uint32_t) + sizeof(blob);
		} else if (selector == SEP_KEYSTORE_SELECTOR_VERIFY_SECRET) {
			record_authorization(fake, AUTH_VERIFY_SECRET);
			EXPECT((int32_t)load_u32_le(
				       output + IPC_FIRST_ARGUMENT_OFFSET) ==
			       SEP_KEYBAG_BIOMETRIC_HANDLE);
			EXPECT(load_u32_le(output + IPC_SECOND_ARGUMENT_OFFSET) ==
			       SEP_KEYSTORE_SECRET_SIZE);
			EXPECT(memcmp(output + VERIFY_SECRET_BASE_SIZE,
				      fake->expected_persistent_secret,
				      SEP_KEYSTORE_SECRET_SIZE) == 0);
			EXPECT(load_u32_le(output + VERIFY_SECRET_BASE_SIZE +
					   SEP_KEYSTORE_SECRET_SIZE) ==
			       SEP_ACM_EXTERNAL_FORM_SIZE);
			EXPECT(memcmp(output + VERIFY_SECRET_BASE_SIZE +
					      SEP_KEYSTORE_SECRET_SIZE +
					      sizeof(uint32_t),
				      acm_external_form,
				      sizeof(acm_external_form)) == 0);
		} else if (selector == SEP_KEYSTORE_SELECTOR_COPY_UUID) {
			record_authorization(fake, AUTH_COPY_UUID);
			EXPECT((int32_t)load_u32_le(
				       output + IPC_FIRST_ARGUMENT_OFFSET) ==
			       SEP_KEYBAG_BIOMETRIC_HANDLE);
			store_u32_le(typed, SEP_KEYSTORE_UUID_SIZE);
			memcpy(typed + sizeof(uint32_t), biometric_keybag_uuid,
			       sizeof(biometric_keybag_uuid));
			typed_length = sizeof(uint32_t) +
				       sizeof(biometric_keybag_uuid);
		} else
			EXPECT(0);
		ipc_length = SEP_KEYSTORE_IPC_WIRE_HEADER_SIZE +
			     sizeof(uint32_t) + typed_length;
		store_u32_le(ipc + SEP_KEYSTORE_IPC_WIRE_HEADER_SIZE,
			     (uint32_t)inner_result);
		if (typed_length != 0)
			memcpy(ipc + SEP_KEYSTORE_IPC_WIRE_HEADER_SIZE +
				       sizeof(uint32_t), typed, typed_length);
		EXPECT(sep_keystore_seal(ipc, ipc_length, 9001) ==
		       SEP_KEYSTORE_OK);
		message[1] = (uint8_t)(selector | UINT8_C(0x80));
		message[2] = header[10];
		store_u16_le(message + 6, (uint16_t)ipc_length);
		build_response(input, endpoint, message, sizeof(message), ipc,
			       ipc_length, 0);
	}
	finish_transfer(fake, timeout_ms);
	return SEP_URB_OK;
}

static enum sep_urb_status fake_send_only(void *context,
					  const uint8_t *output,
					  size_t output_length,
					  unsigned int timeout_ms)
{
	struct fake_operation *fake = context;

	EXPECT(output != NULL && output_length == SEP_RELAY_BUFFER_SIZE);
	++fake->send_calls;
	finish_transfer(fake, timeout_ms);
	return SEP_URB_OK;
}

static enum sep_urb_status fake_receive_only(void *context, uint8_t *input,
					     size_t input_capacity,
					     unsigned int timeout_ms)
{
	struct fake_operation *fake = context;
	uint8_t notification[8] = { 3, 0, 0, 0, 4, 0, 0, 0 };

	EXPECT(input != NULL && input_capacity == SEP_RELAY_BUFFER_SIZE);
	++fake->receive_calls;
	if (fake->interrupt_receive_call == fake->receive_calls) {
		if (fake->cancel_after_receive == fake->receive_calls)
			fake->cancelled = 1;
		return SEP_URB_INTERRUPTED;
	}
	if (fake->receive_calls <= fake->notification_count) {
		build_response(input, SEP_RELAY_KEYSTORE_ENDPOINT, notification,
			       fake->malformed_notification ? 7 :
						      sizeof(notification),
			       NULL, 0, 0);
		if (fake->cancel_after_receive == fake->receive_calls)
			fake->cancelled = 1;
		fake->now += timeout_ms;
		return SEP_URB_OK;
	}
	if (fake->cancel_after_receive == fake->receive_calls)
		fake->cancelled = 1;
	fake->now += timeout_ms;
	return SEP_URB_TIMED_OUT;
}

static enum sep_urb_status fake_destroy(void *context)
{
	struct fake_operation *fake = context;

	++fake->destroy_calls;
	return fake->destroy_fails ? SEP_URB_RELEASE_FAILED : SEP_URB_OK;
}

static int fake_monotonic_ms(void *context, uint64_t *value)
{
	struct fake_operation *fake = context;

	++fake->clock_calls;
	if (fake->clock_fail_call == fake->clock_calls)
		return -1;
	*value = fake->now;
	return 0;
}

static int fake_entropy(void *context, void *output, size_t length)
{
	struct fake_operation *fake = context;
	uint8_t *bytes = output;
	size_t index;

	++fake->entropy_calls;
	if (fake->entropy_fails)
		return -1;
	++fake->entropy_generation;
	for (index = 0; index < length; ++index)
		bytes[index] = (uint8_t)(fake->entropy_generation + index);
	return 0;
}

static int fake_open_lock(void *context, int *file_descriptor)
{
	struct fake_operation *fake = context;

	++fake->open_lock_calls;
	if (fake->open_lock_fails)
		return -1;
	*file_descriptor = 70;
	return 0;
}

static int fake_try_lock(void *context, int file_descriptor,
			 enum sep_operation_lock_mode mode)
{
	struct fake_operation *fake = context;

	EXPECT(file_descriptor == 70);
	if (fake->lock_attempts <
	    sizeof(fake->lock_modes) / sizeof(fake->lock_modes[0]))
		fake->lock_modes[fake->lock_attempts] = mode;
	++fake->lock_attempts;
	if (fake->lock_fails)
		return -1;
	if (fake->lock_busy_attempts != 0) {
		--fake->lock_busy_attempts;
		return 1;
	}
	return 0;
}

static int fake_wait_ms(void *context, unsigned int timeout_ms)
{
	struct fake_operation *fake = context;

	++fake->wait_calls;
	fake->waited_ms += timeout_ms;
	fake->now += timeout_ms;
	return fake->wait_fails ? -1 : 0;
}

static enum sep_operation_result fake_open_sep(void *context,
					      unsigned int timeout_ms,
					      int *file_descriptor)
{
	struct fake_operation *fake = context;

	++fake->open_sep_calls;
	fake->open_sep_timeout_ms = timeout_ms;
	if (fake->open_sep_result != SEP_OPERATION_OK)
		return fake->open_sep_result;
	*file_descriptor = 80;
	return SEP_OPERATION_OK;
}

static int fake_close_fd(void *context, int file_descriptor)
{
	struct fake_operation *fake = context;

	if (fake->close_calls < sizeof(fake->closed) / sizeof(fake->closed[0]))
		fake->closed[fake->close_calls] = file_descriptor;
	++fake->close_calls;
	return file_descriptor == fake->close_fails_for ? -1 : 0;
}

static int fake_initialize_session(void *context, struct sep_session *session,
				   struct sep_urb_transport *transport,
				   int file_descriptor, uint64_t first_token,
				   uint32_t first_message_index)
{
	struct fake_operation *fake = context;
	const struct sep_session_transport_ops transport_ops = {
		.context = fake,
		.exchange = fake_exchange,
		.send_only = fake_send_only,
		.receive_only = fake_receive_only,
		.destroy = fake_destroy,
		.monotonic_ms = fake_monotonic_ms,
	};
	unsigned int index = fake->initialize_calls;

	(void)transport;
	EXPECT(file_descriptor == 80);
	if (index < sizeof(fake->tokens) / sizeof(fake->tokens[0])) {
		fake->tokens[index] = first_token;
		fake->message_indexes[index] = first_message_index;
	}
	++fake->initialize_calls;
	if (fake->initialize_fails)
		return -1;
	return sep_session_init_with_ops(session, &transport_ops, first_token,
					 first_message_index) == SEP_SESSION_OK ?
		       0 :
		       -1;
}

static struct sep_operation_ops fake_ops(struct fake_operation *fake)
{
	const struct sep_operation_ops ops = {
		.context = fake,
		.monotonic_ms = fake_monotonic_ms,
		.entropy = fake_entropy,
		.open_lock = fake_open_lock,
		.try_lock = fake_try_lock,
		.wait_ms = fake_wait_ms,
		.open_sep = fake_open_sep,
		.close_fd = fake_close_fd,
		.initialize_session = fake_initialize_session,
	};

	return ops;
}

static struct fake_operation make_fake(void)
{
	struct fake_operation fake = { 0 };

	fake.now = 100;
	fake.timeout_order_valid = 1;
	fake.close_fails_for = -1;
	fake.device_keybag_present = 1;
	fake.default_system_keybag_present = 1;
	return fake;
}

static int fake_cancelled(void *context)
{
	struct fake_operation *fake = context;

	++fake->cancellation_checks;
	if (fake->cancel_on_check == fake->cancellation_checks)
		fake->cancelled = 1;
	return fake->cancelled;
}

static enum sep_operation_result operation_preflight(void *context)
{
	struct preflight_state *preflight = context;

	++preflight->calls;
	EXPECT(preflight->fake->lock_attempts == 1);
	EXPECT(preflight->fake->entropy_calls == 0);
	EXPECT(preflight->fake->open_sep_calls == 0);
	EXPECT(preflight->fake->initialize_calls == 0);
	preflight->fake->now += preflight->advance_ms;
	return preflight->result;
}

static int build_acm_initialize(void *context, struct sep_acm_state *state,
				struct sep_relay_state *relay,
				uint8_t output[SEP_RELAY_BUFFER_SIZE])
{
	(void)context;
	return sep_acm_build_initialize(state, relay, output);
}

static int build_acm_create(void *context, struct sep_acm_state *state,
			    struct sep_relay_state *relay,
			    uint8_t output[SEP_RELAY_BUFFER_SIZE])
{
	(void)context;
	return sep_acm_build_context_create(state, relay, 1000, output);
}

static int build_acm_externalize(void *context, struct sep_acm_state *state,
				 struct sep_relay_state *relay,
				 uint8_t output[SEP_RELAY_BUFFER_SIZE])
{
	(void)context;
	return sep_acm_build_context_externalize(state, relay, output);
}

static enum sep_operation_result operation_callback(
	void *context, struct sep_operation *operation)
{
	struct callback_state *callback = context;
	enum sep_operation_result release_result = SEP_OPERATION_OK;

	++callback->calls;
	if (callback->initialize_acm) {
		struct sep_acm_outcome outcome;
		struct sep_acm_external_form external;

		callback->acm_succeeded =
			sep_operation_acm_exchange(operation, build_acm_initialize,
					   NULL, &outcome) == SEP_OPERATION_OK &&
			sep_operation_acm_exchange(operation, build_acm_create, NULL,
					   &outcome) == SEP_OPERATION_OK &&
			sep_operation_acm_exchange(operation, build_acm_externalize,
					   NULL, &outcome) == SEP_OPERATION_OK;
		callback->export_succeeded = sep_operation_export_acm_context(
			operation, &external) == SEP_ACM_OK;
		sep_keystore_clear(&external, sizeof(external));
	}
	if (callback->expire_budget)
		callback->fake->now = callback->fake->expire_at;
	if (callback->release_acquisition) {
		release_result = sep_operation_release_acquisition_deadline(operation);
		callback->release_succeeded = release_result == SEP_OPERATION_OK;
		if (callback->release_succeeded) {
			callback->fake->now += callback->lease_advance_ms;
			if (callback->attempt_exchange_during_lease) {
				struct sep_acm_outcome outcome;

				callback->lease_exchange_result =
					sep_operation_acm_exchange(
						operation, build_acm_create, NULL,
						&outcome);
			}
		}
	}
	if (callback->set_cancelled)
		callback->fake->cancelled = 1;
	return release_result == SEP_OPERATION_OK ? callback->result :
					       release_result;
}

static int prepare_keybag_caller_resources(void *context)
{
	struct prepared_keybag_state *prepared = context;

	++prepared->prepare_calls;
	EXPECT(prepared->store->load_calls == 1);
	EXPECT(prepared->fake->open_lock_calls == 1);
	EXPECT(prepared->fake->lock_attempts == 1);
	EXPECT(prepared->fake->entropy_calls == 0);
	EXPECT(prepared->fake->open_sep_calls == 0);
	EXPECT(prepared->fake->initialize_calls == 0);
	return prepared->prepare_fails;
}

static void cleanup_keybag_caller_resources(void *context)
{
	struct prepared_keybag_state *prepared = context;

	++prepared->cleanup_calls;
	if (prepared->expect_session_cleanup) {
		EXPECT(prepared->fake->destroy_calls == 1);
		EXPECT(prepared->fake->close_calls == 1);
		EXPECT(prepared->fake->closed[0] == 80);
	} else {
		EXPECT(prepared->fake->destroy_calls == 0);
		EXPECT(prepared->fake->close_calls == 0);
	}
}

static enum sep_operation_result run_fake(struct fake_operation *fake,
					  unsigned int timeout_ms,
					  struct callback_state *callback)
{
	struct sep_operation_ops ops = fake_ops(fake);

	callback->fake = fake;
	return sep_operation_run_with_ops(
		&ops, timeout_ms, fake_cancelled, fake, operation_callback,
		callback);
}

static int credential_callback(void *context, const uint8_t *credential,
			       size_t credential_length)
{
	struct credential_callback_state *callback = context;

	++callback->calls;
	callback->length = credential_length;
	if (credential_length == sizeof(callback->credential))
		memcpy(callback->credential, credential,
		       sizeof(callback->credential));
	return callback->fail;
}

static enum sep_operation_result run_authorized_fake(
	struct fake_operation *fake, unsigned int timeout_ms, uint32_t audit_uid,
	struct credential_callback_state *callback)
{
	struct sep_operation_ops ops = fake_ops(fake);

	return sep_operation_run_authorized_with_ops(
		&ops, timeout_ms, audit_uid, fake_cancelled, fake,
		credential_callback, callback);
}

static void test_authorized_operation_lends_credential_and_owns_teardown(void)
{
	struct fake_operation fake = make_fake();
	struct credential_callback_state callback = { 0 };
	const uint8_t expected[SEP_ACM_EXTERNAL_FORM_SIZE] = {
		1, 2, 3, 4, 5, 6, 7, 8,
		9, 10, 11, 12, 13, 14, 15, 16,
	};

	EXPECT(run_authorized_fake(&fake, 1000, 1000, &callback) ==
	       SEP_OPERATION_OK);
	EXPECT(callback.calls == 1 && callback.length == sizeof(expected));
	EXPECT(memcmp(callback.credential, expected, sizeof(expected)) == 0);
	EXPECT(fake.exchange_calls == 6 && fake.delete_exchange_calls == 1 &&
	       fake.destroy_calls == 1);
	EXPECT(fake.close_calls == 2 && fake.closed[0] == 80 &&
	       fake.closed[1] == 70);
}

static void test_authorized_callback_failure_still_deletes_context(void)
{
	struct fake_operation fake = make_fake();
	struct credential_callback_state callback = { .fail = 1 };
	struct sep_operation_ops ops = fake_ops(&fake);

	EXPECT(run_authorized_fake(&fake, 1000, 1000, &callback) ==
	       SEP_OPERATION_ERROR_CALLBACK);
	EXPECT(callback.calls == 1 && fake.delete_exchange_calls == 1 &&
	       fake.destroy_calls == 1 && fake.close_calls == 2);
	EXPECT(sep_operation_run_authorized_with_ops(
		       &ops, 1000, 1000, fake_cancelled, &fake, NULL,
		       &callback) == SEP_OPERATION_ERROR_ARGUMENT);
}

static int fake_keybag_load(void *context,
			    struct sep_keybag_material *material)
{
	struct fake_keybag_store *store = context;

	++store->load_calls;
	if (store->load_result == SEP_KEYBAG_STORE_EXISTING)
		*material = store->loaded;
	return store->load_result;
}

static int fake_keybag_persist(
	void *context, const struct sep_keybag_material *material)
{
	struct fake_keybag_store *store = context;

	++store->persist_calls;
	store->persisted = *material;
	return store->persist_fails;
}

static int keybag_callback(
	void *context, enum sep_keybag_disposition disposition,
	const uint8_t *credential, size_t credential_length)
{
	struct keybag_callback_state *callback = context;
	const uint8_t expected_authorization[] = {
		AUTH_ACM_INITIALIZE,
		AUTH_ACM_CREATE,
		AUTH_ACM_EXTERNALIZE,
		AUTH_VERIFY_SECRET,
		AUTH_COPY_UUID,
		AUTH_CONTAINS_TYPE_1_BEFORE,
		AUTH_CONTAINS_TYPE_2_BEFORE,
		AUTH_REPLACE_PASSPHRASE,
		AUTH_CONTAINS_TYPE_1_AFTER,
		AUTH_CONTAINS_TYPE_2_AFTER,
		AUTH_VERIFY_UNBOUND,
		AUTH_VERIFY_BOUND,
	};
	size_t expected_count = callback->authorization ==
					SEP_KEYBAG_ENROLLMENT ?
				sizeof(expected_authorization) :
				AUTH_COPY_UUID;

	++callback->calls;
	if (callback->prepared) {
		EXPECT(callback->prepared->prepare_calls == 1);
		EXPECT(callback->prepared->cleanup_calls == 0);
	}
	EXPECT(callback->fake->authorization_event_count ==
	       expected_count);
	EXPECT(memcmp(callback->fake->authorization_events,
		      expected_authorization, expected_count) == 0);
	callback->disposition = disposition;
	EXPECT(credential_length == sizeof(callback->credential));
	if (credential_length == sizeof(callback->credential))
		memcpy(callback->credential, credential, credential_length);
	if (disposition == SEP_KEYBAG_CREATED)
		EXPECT(callback->store->persist_calls == 1);
	callback->fake->now += callback->lease_advance_ms;
	return callback->fail;
}

static enum sep_operation_result run_keybag_fake(
	struct fake_operation *fake, struct fake_keybag_store *store,
	enum sep_keybag_mode mode, struct keybag_callback_state *callback)
{
	struct sep_operation_ops operation_ops = fake_ops(fake);
	const struct sep_keybag_store_ops store_ops = {
		.context = store,
		.load = fake_keybag_load,
		.persist_absent = fake_keybag_persist,
	};

	callback->store = store;
	callback->fake = fake;
	fake->keybag_authorization = 1;
	memcpy(fake->expected_persistent_secret, store->loaded.secret,
	       sizeof(fake->expected_persistent_secret));
	return sep_keybag_run_with_ops(
		&operation_ops, &store_ops, mode, callback->authorization, 1000,
		fake_cancelled, fake, keybag_callback, callback);
}

static enum sep_operation_result run_prepared_keybag_fake(
	struct fake_operation *fake, struct fake_keybag_store *store,
	struct prepared_keybag_state *prepared,
	struct keybag_callback_state *callback)
{
	struct sep_operation_ops operation_ops = fake_ops(fake);
	const struct sep_keybag_store_ops store_ops = {
		.context = store,
		.load = fake_keybag_load,
		.persist_absent = fake_keybag_persist,
	};

	prepared->fake = fake;
	prepared->store = store;
	callback->store = store;
	callback->fake = fake;
	callback->prepared = prepared;
	fake->keybag_authorization = 1;
	memcpy(fake->expected_persistent_secret, store->loaded.secret,
	       sizeof(fake->expected_persistent_secret));
	return sep_keybag_run_prepared_with_ops(
		&operation_ops, &store_ops, SEP_KEYBAG_EXISTING_ONLY,
		callback->authorization, 1000, fake_cancelled, fake,
		prepare_keybag_caller_resources, prepared, keybag_callback,
		callback, cleanup_keybag_caller_resources, prepared);
}

static int relay_ready(void *context)
{
	struct relay_callback_state *ready = context;
	const uint8_t default_expected_selectors[] = {
		SEP_KEYSTORE_SELECTOR_SET_ENVIRONMENT,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
		SEP_KEYSTORE_SELECTOR_LOAD,
		SEP_KEYSTORE_SELECTOR_MAKE_SYSTEM,
		SEP_KEYSTORE_SELECTOR_LOCK_STATE,
		SEP_KEYSTORE_SELECTOR_GET_CONFIGURATION,
		SEP_KEYSTORE_SELECTOR_UNLOCK,
		SEP_KEYSTORE_SELECTOR_LOCK_STATE,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
	};
	const uint8_t *expected = ready->expected_selectors ?
				  ready->expected_selectors :
				  default_expected_selectors;
	size_t expected_count = ready->expected_selectors ?
				ready->expected_selector_count :
				sizeof(default_expected_selectors);

	++ready->calls;
	EXPECT(ready->fake->keystore_calls == expected_count);
	EXPECT(memcmp(ready->fake->keystore_selectors, expected,
		      expected_count) == 0);
	EXPECT(ready->store->load_calls == 1 &&
	       ready->store->persist_calls == 0);
	EXPECT(ready->fake->authorization_event_count == 1 &&
	       ready->fake->authorization_events[0] == AUTH_ACM_INITIALIZE);
	return ready->fail;
}

static enum sep_operation_result run_relay_fake(
	struct fake_operation *fake, struct fake_keybag_store *store,
	struct relay_callback_state *ready)
{
	struct sep_operation_ops operation_ops = fake_ops(fake);
	const struct sep_keybag_store_ops store_ops = {
		.context = store,
		.load = fake_keybag_load,
		.persist_absent = fake_keybag_persist,
	};

	ready->fake = fake;
	ready->store = store;
	fake->keybag_authorization = 1;
	return sep_keybag_run_notification_relay_with_ops(
		&operation_ops, &store_ops, 1000, 25, fake_cancelled, fake,
		relay_ready, ready);
}

static void test_shared_relay_loads_without_rewrite_and_drains(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	struct relay_callback_state ready = { 0 };

	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	memcpy(store.loaded.blob, (const uint8_t[]){ 1, 3, 5 }, 3);
	store.loaded.blob_length = 3;
	fake.notification_count = 2;
	fake.cancel_after_receive = 3;
	fake.interrupt_receive_call = 3;
	EXPECT(run_relay_fake(&fake, &store, &ready) == SEP_OPERATION_OK);
	EXPECT(ready.calls == 1 && fake.receive_calls == 3);
	EXPECT(fake.lock_attempts == 1 &&
	       fake.lock_modes[0] == SEP_OPERATION_LOCK_SHARED);
	EXPECT(store.persist_calls == 0 && fake.delete_exchange_calls == 0);
	EXPECT(fake.destroy_calls == 1 && fake.close_calls == 2);
	sep_keystore_clear(&store, sizeof(store));

	fake = make_fake();
	store = (struct fake_keybag_store){
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	ready = (struct relay_callback_state){ 0 };
	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	fake.cancel_after_receive = 1;
	fake.interrupt_receive_call = 1;
	fake.destroy_fails = 1;
	EXPECT(run_relay_fake(&fake, &store, &ready) ==
	       SEP_OPERATION_ERROR_TEARDOWN);
	EXPECT(ready.calls == 1 && fake.destroy_calls == 1 &&
	       fake.close_calls == 2 && store.persist_calls == 0);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_shared_relay_fails_before_ready_on_state_or_protocol(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_ABSENT,
	};
	struct relay_callback_state ready = { 0 };
	struct sep_operation_ops operation_ops = fake_ops(&fake);
	const struct sep_keybag_store_ops store_ops = {
		.context = &store,
		.load = fake_keybag_load,
		.persist_absent = fake_keybag_persist,
	};

	EXPECT(sep_keybag_run_notification_relay_with_ops(
		       &operation_ops, &store_ops, 1000,
		       SEP_KEYBAG_RELAY_MAX_POLL_MS + 1, fake_cancelled, &fake,
		       relay_ready, &ready) == SEP_OPERATION_ERROR_ARGUMENT);
	EXPECT(fake.open_lock_calls == 0 && ready.calls == 0);

	EXPECT(run_relay_fake(&fake, &store, &ready) ==
	       SEP_OPERATION_ERROR_STATE);
	EXPECT(ready.calls == 0 && fake.receive_calls == 0 &&
	       store.persist_calls == 0);

	fake = make_fake();
	store = (struct fake_keybag_store){
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	ready = (struct relay_callback_state){ .fail = 1 };
	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	EXPECT(run_relay_fake(&fake, &store, &ready) ==
	       SEP_OPERATION_ERROR_CALLBACK);
	EXPECT(ready.calls == 1 && fake.receive_calls == 0 &&
	       store.persist_calls == 0 && fake.destroy_calls == 1);

	fake = make_fake();
	store = (struct fake_keybag_store){
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	ready = (struct relay_callback_state){ 0 };
	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	/* Existing relay: env, two boot-state reads, load, promote, pre-unlock read. */
	fake.malformed_lock_state_call = 6;
	EXPECT(run_relay_fake(&fake, &store, &ready) ==
	       SEP_OPERATION_ERROR_SESSION);
	EXPECT(ready.calls == 0 && fake.receive_calls == 0 &&
	       fake.keystore_calls == 6 && store.persist_calls == 0);

	fake = make_fake();
	store = (struct fake_keybag_store){
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	ready = (struct relay_callback_state){ 0 };
	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	fake.notification_count = 1;
	fake.cancel_after_receive = 2;
	fake.malformed_notification = 1;
	EXPECT(run_relay_fake(&fake, &store, &ready) ==
	       SEP_OPERATION_ERROR_SESSION);
	EXPECT(ready.calls == 1 && fake.receive_calls == 1 &&
	       store.persist_calls == 0 && fake.destroy_calls == 1);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_kernel_shared_lock_excludes_exclusive_owner(void)
{
	char path[] = "/tmp/t1bridge-sep-lock-XXXXXX";
	int first = mkstemp(path);
	int second = first >= 0 ? open(path, O_RDWR | O_CLOEXEC) : -1;
	int exclusive = first >= 0 ? open(path, O_RDWR | O_CLOEXEC) : -1;

	if (first >= 0)
		(void)unlink(path);
	EXPECT(first >= 0 && second >= 0 && exclusive >= 0);
	if (first >= 0 && second >= 0 && exclusive >= 0) {
		EXPECT(sep_operation_try_lock_descriptor(
			       first, SEP_OPERATION_LOCK_SHARED) == 0);
		EXPECT(sep_operation_try_lock_descriptor(
			       second, SEP_OPERATION_LOCK_SHARED) == 0);
		EXPECT(sep_operation_try_lock_descriptor(
			       exclusive, SEP_OPERATION_LOCK_EXCLUSIVE) == 1);
		(void)close(first);
		first = -1;
		(void)close(second);
		second = -1;
		EXPECT(sep_operation_try_lock_descriptor(
			       exclusive, SEP_OPERATION_LOCK_EXCLUSIVE) == 0);
	}
	if (first >= 0)
		(void)close(first);
	if (second >= 0)
		(void)close(second);
	if (exclusive >= 0)
		(void)close(exclusive);
}

static void test_create_if_absent_rejects_existing_before_usb(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	struct keybag_callback_state callback = { 0 };

	fake.device_keybag_present = 0;
	fake.default_system_keybag_present = 0;
	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	memcpy(store.loaded.blob, (const uint8_t[]){ 1, 3, 5 }, 3);
	store.loaded.blob_length = 3;
	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_CREATE_IF_ABSENT, &callback) ==
	       SEP_OPERATION_ERROR_STATE);
	EXPECT(fake.open_lock_calls == 1 && fake.lock_attempts == 1 &&
	       fake.open_sep_calls == 0 &&
	       fake.initialize_calls == 0 && fake.keystore_calls == 0 &&
	       !fake.device_keybag_present &&
	       !fake.default_system_keybag_present);
	EXPECT(fake.entropy_calls == 0 && store.load_calls == 1 &&
	       store.persist_calls == 0 && callback.calls == 0 &&
	       fake.close_calls == 1 && fake.closed[0] == 70);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_shared_relay_initializes_device_and_literal_default(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	const uint8_t expected_selectors[] = {
		SEP_KEYSTORE_SELECTOR_SET_ENVIRONMENT,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
		SEP_KEYSTORE_SELECTOR_CREATE,
		SEP_KEYSTORE_SELECTOR_MAKE_SYSTEM,
		SEP_KEYSTORE_SELECTOR_LOCK_STATE,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
		SEP_KEYSTORE_SELECTOR_CREATE,
		SEP_KEYSTORE_SELECTOR_MAKE_SYSTEM,
		SEP_KEYSTORE_SELECTOR_LOCK_STATE,
		SEP_KEYSTORE_SELECTOR_UNLOCK,
		SEP_KEYSTORE_SELECTOR_LOCK_STATE,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
		SEP_KEYSTORE_SELECTOR_LOAD,
		SEP_KEYSTORE_SELECTOR_MAKE_SYSTEM,
		SEP_KEYSTORE_SELECTOR_LOCK_STATE,
		SEP_KEYSTORE_SELECTOR_GET_CONFIGURATION,
		SEP_KEYSTORE_SELECTOR_UNLOCK,
		SEP_KEYSTORE_SELECTOR_LOCK_STATE,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
	};
	struct relay_callback_state ready = {
		.expected_selectors = expected_selectors,
		.expected_selector_count = sizeof(expected_selectors),
	};

	fake.device_keybag_present = 0;
	fake.default_system_keybag_present = 0;
	fake.cancel_after_receive = 1;
	fake.interrupt_receive_call = 1;
	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	memcpy(store.loaded.blob, (const uint8_t[]){ 1, 3, 5 }, 3);
	store.loaded.blob_length = 3;
	EXPECT(run_relay_fake(&fake, &store, &ready) == SEP_OPERATION_OK);
	EXPECT(ready.calls == 1 && fake.device_keybag_present &&
	       fake.default_system_keybag_present);
	EXPECT(fake.entropy_calls == 4 && store.persist_calls == 0 &&
	       fake.delete_exchange_calls == 0);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_create_if_absent_rejects_unsafe_device_keybag_state(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_ABSENT,
	};
	struct keybag_callback_state callback = { 0 };
	const uint8_t bad_device_selectors[] = {
		SEP_KEYSTORE_SELECTOR_SET_ENVIRONMENT,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
	};

	fake.device_keybag_bad_state = 1;
	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_CREATE_IF_ABSENT, &callback) ==
	       SEP_OPERATION_ERROR_KEYBAG);
	EXPECT(fake.keystore_calls == sizeof(bad_device_selectors));
	EXPECT(memcmp(fake.keystore_selectors, bad_device_selectors,
		      sizeof(bad_device_selectors)) == 0);
	EXPECT(callback.calls == 0 && store.persist_calls == 0);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_keybag_create_order_persists_before_credential(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_ABSENT,
	};
	struct keybag_callback_state callback = { 0 };
	const uint8_t expected_selectors[] = {
		SEP_KEYSTORE_SELECTOR_SET_ENVIRONMENT,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
		SEP_KEYSTORE_SELECTOR_CREATE,
		SEP_KEYSTORE_SELECTOR_MAKE_SYSTEM,
		SEP_KEYSTORE_SELECTOR_LOCK_STATE,
		SEP_KEYSTORE_SELECTOR_SET_CONFIGURATION,
		SEP_KEYSTORE_SELECTOR_SERIALIZE,
		SEP_KEYSTORE_SELECTOR_VERIFY_SECRET,
		SEP_KEYSTORE_SELECTOR_COPY_UUID,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
	};

	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_CREATE_IF_ABSENT, &callback) ==
	       SEP_OPERATION_OK);
	EXPECT(fake.keystore_calls == sizeof(expected_selectors));
	EXPECT(memcmp(fake.keystore_selectors, expected_selectors,
		      sizeof(expected_selectors)) == 0);
	EXPECT(store.load_calls == 1 && store.persist_calls == 1 &&
	       store.persisted.blob_length == 3 &&
	       memcmp(store.persisted.blob, (const uint8_t[]){ 9, 8, 7 }, 3) ==
		       0);
	EXPECT(callback.calls == 1 &&
	       callback.disposition == SEP_KEYBAG_CREATED);
	EXPECT(fake.entropy_calls == 3 && fake.delete_exchange_calls == 1 &&
	       fake.destroy_calls == 1 && fake.close_calls == 2);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_promoted_keybag_authentication_requires_final_state(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	struct keybag_callback_state callback = { 0 };
	const uint8_t expected_selectors[] = {
		SEP_KEYSTORE_SELECTOR_SET_ENVIRONMENT,
		SEP_KEYSTORE_SELECTOR_VERIFY_SECRET,
		SEP_KEYSTORE_SELECTOR_COPY_UUID,
		SEP_KEYSTORE_SELECTOR_DEVICE_STATE,
	};

	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_EXISTING_ONLY, &callback) ==
	       SEP_OPERATION_OK);
	EXPECT(fake.keystore_calls == sizeof(expected_selectors));
	EXPECT(memcmp(fake.keystore_selectors, expected_selectors,
		      sizeof(expected_selectors)) == 0);
	EXPECT(callback.calls == 1 &&
	       callback.disposition == SEP_KEYBAG_REUSED);
	EXPECT(fake.delete_exchange_calls == 1 && fake.destroy_calls == 1 &&
	       fake.close_calls == 2);
	sep_keystore_clear(&store, sizeof(store));

	fake = make_fake();
	fake.biometric_keybag_bad_state = 1;
	store = (struct fake_keybag_store){
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	callback = (struct keybag_callback_state){ 0 };
	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_EXISTING_ONLY, &callback) ==
	       SEP_OPERATION_ERROR_KEYBAG);
	EXPECT(fake.keystore_calls == sizeof(expected_selectors));
	EXPECT(memcmp(fake.keystore_selectors, expected_selectors,
		      sizeof(expected_selectors)) == 0);
	EXPECT(callback.calls == 0 && fake.delete_exchange_calls == 1 &&
	       fake.destroy_calls == 1);
	sep_keystore_clear(&store, sizeof(store));

	fake = make_fake();
	fake.biometric_keybag_missing = 1;
	store = (struct fake_keybag_store){
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	callback = (struct keybag_callback_state){ 0 };
	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_EXISTING_ONLY, &callback) ==
	       SEP_OPERATION_REMOTE_ERROR);
	EXPECT(fake.keystore_calls == sizeof(expected_selectors));
	EXPECT(memcmp(fake.keystore_selectors, expected_selectors,
		      sizeof(expected_selectors)) == 0);
	EXPECT(callback.calls == 0 && fake.delete_exchange_calls == 1 &&
	       fake.destroy_calls == 1);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_promoted_keybag_enrollment_adds_policy_credential(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	struct keybag_callback_state callback = {
		.authorization = SEP_KEYBAG_ENROLLMENT,
	};
	const uint8_t expected_selectors[] = {
		SEP_KEYSTORE_SELECTOR_SET_ENVIRONMENT,
		SEP_KEYSTORE_SELECTOR_VERIFY_SECRET,
		SEP_KEYSTORE_SELECTOR_COPY_UUID,
	};

	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	memcpy(store.loaded.blob, (const uint8_t[]){ 1, 3, 5 }, 3);
	store.loaded.blob_length = 3;
	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_EXISTING_ONLY, &callback) ==
	       SEP_OPERATION_OK);
	EXPECT(fake.keystore_calls == sizeof(expected_selectors));
	EXPECT(memcmp(fake.keystore_selectors, expected_selectors,
		      sizeof(expected_selectors)) == 0);
	EXPECT(store.load_calls == 1 && store.persist_calls == 0);
	EXPECT(callback.calls == 1 &&
	       callback.disposition == SEP_KEYBAG_REUSED);
	EXPECT(fake.entropy_calls == 2 && fake.delete_exchange_calls == 1);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_prepared_keybag_orders_caller_work_inside_fixed_lock(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	struct prepared_keybag_state prepared = {
		.expect_session_cleanup = 1,
	};
	struct keybag_callback_state callback = { 0 };

	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	EXPECT(run_prepared_keybag_fake(
		       &fake, &store, &prepared, &callback) == SEP_OPERATION_OK);
	EXPECT(prepared.prepare_calls == 1 && prepared.cleanup_calls == 1);
	EXPECT(callback.calls == 1 &&
	       callback.disposition == SEP_KEYBAG_REUSED);
	EXPECT(fake.open_lock_calls == 1 && fake.lock_attempts == 1 &&
	       fake.open_sep_calls == 1 && fake.initialize_calls == 1);
	EXPECT(fake.destroy_calls == 1 && fake.close_calls == 2 &&
	       fake.closed[0] == 80 && fake.closed[1] == 70);
	sep_keystore_clear(&store, sizeof(store));

	fake = make_fake();
	store = (struct fake_keybag_store){
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	prepared = (struct prepared_keybag_state){ .prepare_fails = 1 };
	callback = (struct keybag_callback_state){ 0 };
	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	EXPECT(run_prepared_keybag_fake(
		       &fake, &store, &prepared, &callback) ==
	       SEP_OPERATION_ERROR_CALLBACK);
	EXPECT(prepared.prepare_calls == 1 && prepared.cleanup_calls == 0);
	EXPECT(callback.calls == 0 && fake.entropy_calls == 0 &&
	       fake.open_sep_calls == 0 && fake.close_calls == 1 &&
	       fake.closed[0] == 70);
	sep_keystore_clear(&store, sizeof(store));

	fake = make_fake();
	fake.open_sep_result = SEP_OPERATION_ERROR_USB;
	store = (struct fake_keybag_store){
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	prepared = (struct prepared_keybag_state){ 0 };
	callback = (struct keybag_callback_state){ 0 };
	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	EXPECT(run_prepared_keybag_fake(
		       &fake, &store, &prepared, &callback) ==
	       SEP_OPERATION_ERROR_USB);
	EXPECT(prepared.prepare_calls == 1 && prepared.cleanup_calls == 1);
	EXPECT(callback.calls == 0 && fake.open_sep_calls == 1 &&
	       fake.close_calls == 1 && fake.closed[0] == 70);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_keybag_state_and_persistence_fail_before_use(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_ERROR,
	};
	struct keybag_callback_state callback = { 0 };

	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_EXISTING_ONLY, &callback) ==
	       SEP_OPERATION_ERROR_STATE);
	EXPECT(fake.keystore_calls == 0 && callback.calls == 0 &&
	       store.persist_calls == 0 && fake.open_sep_calls == 0 &&
	       fake.destroy_calls == 0 && fake.close_calls == 1);

	fake = make_fake();
	store = (struct fake_keybag_store){
		.load_result = SEP_KEYBAG_STORE_ABSENT,
		.persist_fails = 1,
	};
	callback = (struct keybag_callback_state){ 0 };
	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_CREATE_IF_ABSENT, &callback) ==
	       SEP_OPERATION_ERROR_PERSISTENCE);
	EXPECT(fake.keystore_calls == 7 && store.persist_calls == 1 &&
	       callback.calls == 0 && fake.delete_exchange_calls == 0);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_keybag_missing_existing_and_cancellation_fail_closed(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_ABSENT,
	};
	struct keybag_callback_state callback = { 0 };

	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_EXISTING_ONLY, &callback) ==
	       SEP_OPERATION_ERROR_STATE);
	EXPECT(fake.keystore_calls == 0 && callback.calls == 0 &&
	       fake.open_sep_calls == 0 && fake.close_calls == 1);

	fake = make_fake();
	fake.cancelled = 1;
	store = (struct fake_keybag_store){
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	callback = (struct keybag_callback_state){ 0 };
	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_EXISTING_ONLY, &callback) ==
	       SEP_OPERATION_ERROR_CANCELLED);
	EXPECT(store.load_calls == 0 && callback.calls == 0 &&
	       fake.close_calls == 0);
}

static void test_keybag_callback_failure_still_cleans_every_owner(void)
{
	struct fake_operation fake = make_fake();
	struct fake_keybag_store store = {
		.load_result = SEP_KEYBAG_STORE_EXISTING,
	};
	struct keybag_callback_state callback = { .fail = 1 };

	memset(store.loaded.secret, 0x5a, sizeof(store.loaded.secret));
	store.loaded.blob[0] = 1;
	store.loaded.blob_length = 1;
	EXPECT(run_keybag_fake(
		       &fake, &store, SEP_KEYBAG_EXISTING_ONLY, &callback) ==
	       SEP_OPERATION_ERROR_CALLBACK);
	EXPECT(callback.calls == 1 && fake.delete_exchange_calls == 1 &&
	       fake.destroy_calls == 1 && fake.close_calls == 2 &&
	       fake.closed[0] == 80 && fake.closed[1] == 70);
	sep_keystore_clear(&store, sizeof(store));
}

static void test_complete_operation_owns_and_releases_everything(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = {
		.result = SEP_OPERATION_OK,
		.initialize_acm = 1,
	};

	fake.transfer_advance_ms = 3;
	EXPECT(run_fake(&fake, 1000, &callback) == SEP_OPERATION_OK);
	EXPECT(callback.calls == 1 && callback.acm_succeeded &&
	       callback.export_succeeded);
	EXPECT(fake.open_lock_calls == 1 && fake.lock_attempts == 1 &&
	       fake.entropy_calls == 1 && fake.open_sep_calls == 1 &&
	       fake.initialize_calls == 1 && fake.destroy_calls == 1);
	EXPECT(fake.open_sep_timeout_ms > 0 && fake.open_sep_timeout_ms <= 1000);
	EXPECT(fake.exchange_calls == 6 && fake.send_calls == 7 &&
	       fake.timeout_order_valid);
	EXPECT(fake.close_calls == 2 && fake.closed[0] == 80 &&
	       fake.closed[1] == 70);
	EXPECT(fake.tokens[0] == UINT64_C(0x0807060504030201) &&
	       fake.message_indexes[0] == UINT32_C(0x0c0b0a09));
}

static void test_preflight_runs_under_lock_before_device_acquisition(void)
{
	struct fake_operation fake = make_fake();
	struct sep_operation_ops ops = fake_ops(&fake);
	struct preflight_state preflight = {
		.fake = &fake,
		.result = SEP_OPERATION_ERROR_STATE,
	};
	struct callback_state callback = { .result = SEP_OPERATION_OK };

	EXPECT(sep_operation_run_preflight_with_ops(
		       &ops, 1000, fake_cancelled, &fake, operation_preflight,
		       &preflight, operation_callback, &callback) ==
	       SEP_OPERATION_ERROR_STATE);
	EXPECT(preflight.calls == 1 && callback.calls == 0 &&
	       fake.open_lock_calls == 1 && fake.lock_attempts == 1 &&
	       fake.entropy_calls == 0 && fake.open_sep_calls == 0 &&
	       fake.close_calls == 1 && fake.closed[0] == 70);

	fake = make_fake();
	ops = fake_ops(&fake);
	preflight = (struct preflight_state){
		.fake = &fake,
		.result = SEP_OPERATION_OK,
	};
	callback = (struct callback_state){ .result = SEP_OPERATION_OK };
	EXPECT(sep_operation_run_preflight_with_ops(
		       &ops, 1000, fake_cancelled, &fake, operation_preflight,
		       &preflight, operation_callback, &callback) ==
	       SEP_OPERATION_OK);
	EXPECT(preflight.calls == 1 && callback.calls == 1 &&
	       fake.entropy_calls == 1 && fake.open_sep_calls == 1 &&
	       fake.initialize_calls == 1);

	fake = make_fake();
	ops = fake_ops(&fake);
	preflight = (struct preflight_state){
		.fake = &fake,
		.result = SEP_OPERATION_OK,
		.advance_ms = 100,
	};
	callback = (struct callback_state){ .result = SEP_OPERATION_OK };
	EXPECT(sep_operation_run_preflight_with_ops(
		       &ops, 100, fake_cancelled, &fake, operation_preflight,
		       &preflight, operation_callback, &callback) ==
	       SEP_OPERATION_ERROR_TIMEOUT);
	EXPECT(preflight.calls == 1 && callback.calls == 0 &&
	       fake.entropy_calls == 0 && fake.open_sep_calls == 0 &&
	       fake.close_calls == 1);
}

static void test_every_session_uses_fresh_entropy(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state first = { .result = SEP_OPERATION_OK };
	struct callback_state second = { .result = SEP_OPERATION_OK };

	EXPECT(run_fake(&fake, 1000, &first) == SEP_OPERATION_OK);
	EXPECT(run_fake(&fake, 1000, &second) == SEP_OPERATION_OK);
	EXPECT(fake.entropy_calls == 2 && fake.initialize_calls == 2 &&
	       fake.tokens[0] != fake.tokens[1] &&
	       fake.message_indexes[0] != fake.message_indexes[1]);
}

static void test_lock_wait_uses_the_operation_deadline(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = { .result = SEP_OPERATION_OK };

	fake.lock_busy_attempts = 100;
	EXPECT(run_fake(&fake, 30, &callback) == SEP_OPERATION_ERROR_TIMEOUT);
	EXPECT(fake.wait_calls == 2 && fake.waited_ms == 30 &&
	       fake.lock_attempts == 2);
	EXPECT(fake.entropy_calls == 0 && callback.calls == 0);
	EXPECT(fake.close_calls == 1 && fake.closed[0] == 70);

	fake = make_fake();
	callback = (struct callback_state){ .result = SEP_OPERATION_OK };
	fake.lock_busy_attempts = 1;
	fake.wait_fails = 1;
	EXPECT(run_fake(&fake, 100, &callback) == SEP_OPERATION_ERROR_LOCK);
	EXPECT(fake.close_calls == 1 && fake.closed[0] == 70);
}

static void test_cancellation_is_owned_before_and_after_callback(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = { .result = SEP_OPERATION_OK };

	fake.cancel_on_check = 1;
	EXPECT(run_fake(&fake, 1000, &callback) ==
	       SEP_OPERATION_ERROR_CANCELLED);
	EXPECT(fake.open_lock_calls == 0 && fake.close_calls == 0 &&
	       callback.calls == 0);

	fake = make_fake();
	callback = (struct callback_state){
		.result = SEP_OPERATION_OK,
		.set_cancelled = 1,
	};
	EXPECT(run_fake(&fake, 1000, &callback) ==
	       SEP_OPERATION_ERROR_CANCELLED);
	EXPECT(callback.calls == 1 && fake.destroy_calls == 1 &&
	       fake.close_calls == 2);
}

static void test_session_transfer_cannot_extend_the_deadline(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = { .result = SEP_OPERATION_OK };

	fake.expire_at = 200;
	fake.expire_after_transfer = 1;
	EXPECT(run_fake(&fake, 100, &callback) == SEP_OPERATION_ERROR_TIMEOUT);
	EXPECT(fake.exchange_calls == 1 && fake.send_calls == 0 &&
	       callback.calls == 0 && fake.destroy_calls == 1);
}

static int observed_cleanup_result;
static unsigned int observed_cleanup_calls;

static void observe_cleanup(int result)
{
	observed_cleanup_result = result;
	observed_cleanup_calls++;
}

static void test_cleanup_observation_preserves_primary_result(void)
{
	const enum sep_operation_result results[] = {
		SEP_OPERATION_OK, SEP_OPERATION_REMOTE_ERROR,
		SEP_OPERATION_ERROR_ACM, SEP_OPERATION_ERROR_CANCELLED,
	};
	sep_operation_cleanup_observer previous;
	size_t index;
	int failure;

	previous = sep_operation_set_cleanup_observer(observe_cleanup);
	for (index = 0; index < sizeof(results) / sizeof(results[0]); ++index) {
		for (failure = 0; failure < 5; ++failure) {
			struct fake_operation fake = make_fake();
			struct callback_state callback = {
				.result = results[index], .initialize_acm = 1,
			};
			enum sep_operation_result expected = results[index];

			fake.delete_exchange_fails = failure == 1;
			fake.destroy_fails = failure == 2;
			fake.close_fails_for = failure == 3 ? 80 :
					       failure == 4 ? 70 : -1;
			if (failure && (expected == SEP_OPERATION_OK ||
					expected == SEP_OPERATION_ERROR_CANCELLED))
				expected = SEP_OPERATION_ERROR_TEARDOWN;
			observed_cleanup_calls = 0;
			EXPECT(run_fake(&fake, 1000, &callback) == expected);
			EXPECT(callback.acm_succeeded && fake.delete_exchange_calls == 1);
			EXPECT(fake.destroy_calls == 1 && fake.close_calls == 2);
			EXPECT(observed_cleanup_calls == 1);
			EXPECT(observed_cleanup_result == (failure ?
				SEP_OPERATION_ERROR_TEARDOWN : SEP_OPERATION_OK));
		}
	}
	EXPECT(sep_operation_set_cleanup_observer(previous) == observe_cleanup);
}

static void test_primary_errors_survive_cleanup_failure(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = { .result = SEP_OPERATION_ERROR_ACM };

	fake.destroy_fails = 1;
	fake.close_fails_for = 80;
	EXPECT(run_fake(&fake, 1000, &callback) == SEP_OPERATION_ERROR_ACM);
	EXPECT(fake.destroy_calls == 1 && fake.close_calls == 2);

	fake = make_fake();
	callback = (struct callback_state){ .result = SEP_OPERATION_OK };
	fake.destroy_fails = 1;
	EXPECT(run_fake(&fake, 1000, &callback) ==
	       SEP_OPERATION_ERROR_TEARDOWN);
	EXPECT(fake.destroy_calls == 1 && fake.close_calls == 2);
}

static void test_acm_context_teardown_obeys_result_deadline_and_cancellation(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = {
		.result = SEP_OPERATION_OK,
		.initialize_acm = 1,
	};

	fake.delete_exchange_fails = 1;
	EXPECT(run_fake(&fake, 1000, &callback) ==
	       SEP_OPERATION_ERROR_TEARDOWN);
	EXPECT(callback.acm_succeeded && fake.delete_exchange_calls == 1);

	fake = make_fake();
	callback = (struct callback_state){
		.result = SEP_OPERATION_OK,
		.initialize_acm = 1,
		.expire_budget = 1,
	};
	fake.expire_at = fake.now + 100;
	EXPECT(run_fake(&fake, 100, &callback) ==
	       SEP_OPERATION_ERROR_TEARDOWN);
	EXPECT(callback.acm_succeeded && fake.delete_exchange_calls == 0);

	fake = make_fake();
	callback = (struct callback_state){
		.result = SEP_OPERATION_OK,
		.initialize_acm = 1,
		.set_cancelled = 1,
	};
	EXPECT(run_fake(&fake, 1000, &callback) ==
	       SEP_OPERATION_ERROR_CANCELLED);
	EXPECT(callback.acm_succeeded && fake.delete_exchange_calls == 1);
}

static void test_consumer_lease_gets_a_fresh_bounded_cleanup_deadline(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = {
		.result = SEP_OPERATION_OK,
		.initialize_acm = 1,
		.release_acquisition = 1,
		.attempt_exchange_during_lease = 1,
		.lease_advance_ms = 10 * 60 * 1000U,
	};

	EXPECT(run_fake(&fake, 100, &callback) == SEP_OPERATION_OK);
	EXPECT(callback.acm_succeeded && callback.release_succeeded);
	EXPECT(callback.lease_exchange_result == SEP_OPERATION_ERROR_STATE);
	EXPECT(fake.delete_exchange_calls == 1 && fake.destroy_calls == 1);
	EXPECT(fake.delete_timeout_ms == SEP_OPERATION_CLEANUP_TIMEOUT_MS);
}

static void test_expired_acquisition_cannot_be_released_into_a_lease(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = {
		.result = SEP_OPERATION_OK,
		.release_acquisition = 1,
		.expire_budget = 1,
	};

	fake.expire_at = fake.now + 100;
	EXPECT(run_fake(&fake, 100, &callback) == SEP_OPERATION_ERROR_TIMEOUT);
	EXPECT(!callback.release_succeeded && fake.destroy_calls == 1);
}

static void test_acquisition_failures_release_only_owned_resources(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = { .result = SEP_OPERATION_OK };

	fake.entropy_fails = 1;
	EXPECT(run_fake(&fake, 1000, &callback) ==
	       SEP_OPERATION_ERROR_ENTROPY);
	EXPECT(fake.close_calls == 1 && fake.closed[0] == 70 &&
	       fake.open_sep_calls == 0);

	fake = make_fake();
	callback = (struct callback_state){ .result = SEP_OPERATION_OK };
	fake.open_sep_result = SEP_OPERATION_ERROR_USB;
	EXPECT(run_fake(&fake, 1000, &callback) == SEP_OPERATION_ERROR_USB);
	EXPECT(fake.close_calls == 1 && fake.closed[0] == 70);

	fake = make_fake();
	callback = (struct callback_state){ .result = SEP_OPERATION_OK };
	fake.open_sep_result = SEP_OPERATION_ERROR_TIMEOUT;
	EXPECT(run_fake(&fake, 1000, &callback) == SEP_OPERATION_ERROR_TIMEOUT);
	EXPECT(fake.close_calls == 1 && fake.closed[0] == 70);

	fake = make_fake();
	callback = (struct callback_state){ .result = SEP_OPERATION_OK };
	fake.initialize_fails = 1;
	EXPECT(run_fake(&fake, 1000, &callback) ==
	       SEP_OPERATION_ERROR_SESSION);
	EXPECT(fake.destroy_calls == 0 && fake.close_calls == 2 &&
	       fake.closed[0] == 80 && fake.closed[1] == 70);
}

static void test_arguments_and_clock_failures_stop_before_ownership(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = { .result = SEP_OPERATION_OK };
	struct sep_operation_ops ops = fake_ops(&fake);
	struct sep_operation_ops incomplete = ops;

	EXPECT(sep_operation_run_with_ops(&ops, 0, fake_cancelled, &fake,
					  operation_callback, &callback) ==
	       SEP_OPERATION_ERROR_ARGUMENT);
	incomplete.entropy = NULL;
	EXPECT(sep_operation_run_with_ops(&incomplete, 100, fake_cancelled, &fake,
					  operation_callback, &callback) ==
	       SEP_OPERATION_ERROR_ARGUMENT);
	fake.clock_fail_call = 1;
	EXPECT(run_fake(&fake, 100, &callback) == SEP_OPERATION_ERROR_CLOCK);
	EXPECT(fake.open_lock_calls == 0 && fake.close_calls == 0 &&
	       callback.calls == 0);
}

static void test_operation_budget_can_span_multiple_transfer_ceilings(void)
{
	struct fake_operation fake = make_fake();
	struct callback_state callback = { .result = SEP_OPERATION_OK };

	EXPECT(run_fake(&fake, SEP_URB_MAX_TIMEOUT_MS + 5000U, &callback) ==
	       SEP_OPERATION_OK);
	EXPECT(callback.calls == 1);
	EXPECT(fake.open_sep_timeout_ms == SEP_URB_MAX_TIMEOUT_MS);
	EXPECT(fake.last_timeout_ms <= SEP_URB_MAX_TIMEOUT_MS);
}

int main(void)
{
	test_kernel_shared_lock_excludes_exclusive_owner();
	test_shared_relay_loads_without_rewrite_and_drains();
	test_shared_relay_fails_before_ready_on_state_or_protocol();
	test_create_if_absent_rejects_existing_before_usb();
	test_shared_relay_initializes_device_and_literal_default();
	test_create_if_absent_rejects_unsafe_device_keybag_state();
	test_keybag_create_order_persists_before_credential();
	test_promoted_keybag_authentication_requires_final_state();
	test_promoted_keybag_enrollment_adds_policy_credential();
	test_prepared_keybag_orders_caller_work_inside_fixed_lock();
	test_keybag_state_and_persistence_fail_before_use();
	test_keybag_missing_existing_and_cancellation_fail_closed();
	test_keybag_callback_failure_still_cleans_every_owner();
	test_authorized_operation_lends_credential_and_owns_teardown();
	test_authorized_callback_failure_still_deletes_context();
	test_preflight_runs_under_lock_before_device_acquisition();
	test_complete_operation_owns_and_releases_everything();
	test_every_session_uses_fresh_entropy();
	test_lock_wait_uses_the_operation_deadline();
	test_cancellation_is_owned_before_and_after_callback();
	test_session_transfer_cannot_extend_the_deadline();
	test_cleanup_observation_preserves_primary_result();
	test_primary_errors_survive_cleanup_failure();
	test_acm_context_teardown_obeys_result_deadline_and_cancellation();
	test_consumer_lease_gets_a_fresh_bounded_cleanup_deadline();
	test_expired_acquisition_cannot_be_released_into_a_lease();
	test_acquisition_failures_release_only_owned_resources();
	test_arguments_and_clock_failures_stop_before_ownership();
	test_operation_budget_can_span_multiple_transfer_ceilings();
	if (failures != 0) {
		fprintf(stderr, "sep_operation: %u tests failed\n", failures);
		return 1;
	}
	puts("sep_operation: all tests passed");
	return 0;
}
