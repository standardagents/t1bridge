/* SPDX-License-Identifier: GPL-2.0 */
/* Real display protocol functions with synthetic allocation and USB boundaries. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef uint8_t u8;
typedef uint16_t __le16;
typedef uint32_t __le32;
typedef uint64_t __le64;
#define __packed __attribute__((packed))
#define cpu_to_le16(value) ((__le16)(value))
#define cpu_to_le32(value) ((__le32)(value))
#define GFP_KERNEL 0
#define APPLETBDRM_T1_RESPONSE_ATTEMPTS 8

struct usb_device { int unused; };
struct drm_device { void *dev; };
struct appletbdrm_device {
	struct usb_device usb;
	struct drm_device drm;
	unsigned int in_ep, out_ep, width, height;
	bool t1;
};

static unsigned int allocations, live_allocations, fail_allocation, reads;
static int send_error, read_error, send_short, reply_short;
static unsigned int valid_after;
static unsigned char reply[128];
static char diagnostic[256];

static void *allocate(size_t size)
{
	if (++allocations == fail_allocation)
		return NULL;
	void *ptr = calloc(1, size);
	assert(ptr);
	live_allocations++;
	return ptr;
}

static void release(void *ptr)
{
	assert(ptr && live_allocations);
	live_allocations--;
	free(ptr);
}

static uint32_t unaligned32(const void *ptr)
{
	uint32_t value;
	memcpy(&value, ptr, sizeof(value));
	return value;
}

static void ignore_log(void *device, const char *format, ...)
{
	(void)device;
	(void)format;
}

static void debug_log(void *device, const char *format, ...)
{
	va_list args;
	(void)device;
	va_start(args, format);
	vsnprintf(diagnostic, sizeof(diagnostic), format, args);
	va_end(args);
}

#define kzalloc_obj(object) allocate(sizeof(object))
#define kfree(ptr) release(ptr)
#define get_unaligned(ptr) unaligned32(ptr)
#define get_unaligned_le32(ptr) unaligned32(ptr)
#define adev_to_udev(adev) (&(adev)->usb)
#define appletbdrm_is_t1(adev) ((adev)->t1)
#define drm_err ignore_log
#define dev_dbg debug_log
#define usb_sndbulkpipe(usb, ep) ((void)(usb), (void)(ep), 0)
#define usb_rcvbulkpipe(usb, ep) ((void)(usb), (void)(ep), 1)

static int usb_bulk_msg(struct usb_device *usb, unsigned int pipe, void *buffer,
			int size, int *actual, int timeout);

#include "appletbdrm_protocol.h"

static int usb_bulk_msg(struct usb_device *usb, unsigned int pipe, void *buffer,
			int size, int *actual, int timeout)
{
	(void)usb;
	assert(timeout == APPLETBDRM_BULK_MSG_TIMEOUT);
	if (!pipe) {
		*actual = size - send_short;
		return send_error;
	}
	reads++;
	if (read_error)
		return read_error;
	assert(size > 0 && (size_t)size <= sizeof(reply));
	memcpy(buffer, reply, (size_t)size);
	*actual = size - reply_short;
	if (reads < valid_after)
		((struct appletbdrm_msg_response_header *)buffer)->msg =
			APPLETBDRM_MSG_SIGNAL_READINESS;
	return 0;
}

static void reset(void)
{
	assert(live_allocations == 0);
	allocations = fail_allocation = reads = 0;
	send_error = read_error = send_short = reply_short = 0;
	valid_after = 1;
	memset(reply, 0, sizeof(reply));
	diagnostic[0] = '\0';
}

static void test_information_cleanup(void)
{
	struct appletbdrm_device adev = { .t1 = true };
	struct appletbdrm_msg_information info = {
		.header.msg = APPLETBDRM_MSG_GET_INFORMATION,
		.width = 1234, .height = 56,
		.bits_per_pixel = APPLETBDRM_BITS_PER_PIXEL,
		.pixel_format = APPLETBDRM_PIXEL_FORMAT,
	};
	for (unsigned int failed = 1; failed <= 2; ++failed) {
		reset();
		fail_allocation = failed;
		assert(appletbdrm_get_information(&adev) == -ENOMEM);
		assert(live_allocations == 0 && reads == 0);
	}
	const int errors[] = { -ETIMEDOUT, -EPIPE, -ENODEV };
	for (size_t i = 0; i < sizeof(errors) / sizeof(errors[0]); ++i) {
		reset();
		send_error = errors[i];
		assert(appletbdrm_get_information(&adev) == errors[i]);
		assert(live_allocations == 0 && reads == 0);
		reset();
		read_error = errors[i];
		assert(appletbdrm_get_information(&adev) == errors[i]);
		assert(live_allocations == 0 && reads == 1);
		assert(strstr(diagnostic, "kind=information result=error"));
	}
	reset();
	send_short = 1;
	assert(appletbdrm_get_information(&adev) == -EIO);
	assert(live_allocations == 0 && reads == 0);
	reset();
	memcpy(reply, &info, sizeof(info));
	assert(appletbdrm_get_information(&adev) == 0);
	assert(adev.width == 1234 && adev.height == 56 && live_allocations == 0);
	assert(strstr(diagnostic, "kind=information result=ok"));
	reset();
	info.bits_per_pixel = 1;
	memcpy(reply, &info, sizeof(info));
	assert(appletbdrm_get_information(&adev) == -EINVAL);
	assert(live_allocations == 0);
}

static void test_update_reply_bounds(void)
{
	struct appletbdrm_device adev = { .t1 = true };
	struct appletbdrm_fb_request_response response = { 0 };
	struct appletbdrm_fb_request_response valid = {
		.header.msg = APPLETBDRM_MSG_UPDATE_COMPLETE,
	};
	for (unsigned int attempt = 1; attempt <= 9; ++attempt) {
		reset();
		memcpy(reply, &valid, sizeof(valid));
		valid_after = attempt;
		int rc = appletbdrm_read_response(&adev, &response.header,
					 sizeof(response), APPLETBDRM_MSG_UPDATE_COMPLETE);
		assert(rc == (attempt <= 8 ? 0 : -EBADMSG));
		assert(reads == (attempt <= 8 ? attempt : 8));
		assert(strstr(diagnostic, attempt <= 8 ? "result=ok" : "result=error"));
	}
	reset();
	memcpy(reply, &valid, sizeof(valid));
	reply_short = 1;
	assert(appletbdrm_read_response(&adev, &response.header, sizeof(response),
				      APPLETBDRM_MSG_UPDATE_COMPLETE) == -EBADMSG);
	assert(reads == 8 && strstr(diagnostic, "result=error"));
	reset();
	read_error = -ETIMEDOUT;
	assert(appletbdrm_read_response(&adev, &response.header, sizeof(response),
				      APPLETBDRM_MSG_UPDATE_COMPLETE) == -ETIMEDOUT);
	assert(reads == 1 && strstr(diagnostic, "code=-110 attempts=1"));
	reset();
	adev.t1 = false;
	assert(appletbdrm_read_response(&adev, &response.header, sizeof(response),
				      APPLETBDRM_MSG_UPDATE_COMPLETE) == -EBADMSG);
	assert(reads == 1 && strstr(diagnostic, "attempts=1"));
}

int main(void)
{
	test_information_cleanup();
	test_update_reply_bounds();
	reset();
	struct appletbdrm_device adev = { .t1 = true };
	assert(appletbdrm_clear_display(&adev) == 0);
	assert(appletbdrm_signal_readiness(&adev) == 0);
	assert(live_allocations == 0);
	puts("appletbdrm protocol tests passed");
	return 0;
}
