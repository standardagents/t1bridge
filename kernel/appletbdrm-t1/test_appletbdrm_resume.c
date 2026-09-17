/* SPDX-License-Identifier: GPL-2.0 */
/* Run the real resume callbacks with synthetic USB/DRM boundaries. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>

enum step { CLEAR_OUT, CLEAR_IN, INFORMATION, READINESS, CLEAR, UNPARK, RESTORE };
static const enum step recovery[] = {
	CLEAR_OUT, CLEAR_IN, INFORMATION, READINESS, CLEAR, UNPARK, CLEAR, RESTORE,
};
static enum step observed[sizeof(recovery) / sizeof(recovery[0])];
static size_t calls, fail_at, errors;
static int failure;

typedef unsigned char u8;

#define APPLETBDRM_T1_HID_INTERFACE 6
#define USB_CTRL_SET_TIMEOUT 1000
#define GFP_KERNEL 0x10

struct usb_device { int unused; };
struct drm_device { int unused; };
struct appletbdrm_device {
	struct usb_device usb;
	struct drm_device drm;
	unsigned int in_ep, out_ep;
	bool t1;
};
struct usb_interface {
	struct appletbdrm_device *data;
	unsigned int needs_binding;
};

static int perform(enum step step)
{
	assert(calls < sizeof(observed) / sizeof(observed[0]));
	observed[calls++] = step;
	return calls == fail_at ? failure : 0;
}

#define adev_to_udev(adev) (&(adev)->usb)
#define usb_get_intfdata(intf) ((intf)->data)
#define appletbdrm_is_t1(adev) ((adev)->t1)
#define drm_err(drm, format, error) ((void)(drm), (void)(format), (void)(error), ++errors)
#define drm_warn(drm, format, error) ((void)(drm), (void)(format), (void)(error), ++errors)

static unsigned int usb_sndbulkpipe(struct usb_device *usb, unsigned int ep)
{
	(void)usb;
	assert(ep == 2);
	return CLEAR_OUT;
}

static unsigned int usb_rcvbulkpipe(struct usb_device *usb, unsigned int ep)
{
	(void)usb;
	assert(ep == 5);
	return CLEAR_IN;
}

/* Mock the display-byte (report 3) write used by park()/unpark(). */
static unsigned int usb_sndctrlpipe(struct usb_device *usb, unsigned int ep)
{
	(void)usb;
	assert(ep == 0);
	return 0;
}

static void *kmemdup(const void *src, size_t len, unsigned int flags)
{
	static u8 backing[32];
	(void)flags;
	if (len > sizeof(backing))
		return NULL;
	__builtin_memcpy(backing, src, len);
	return backing;
}

static void kfree(const void *ptr)
{
	(void)ptr;
}

static int usb_control_msg(struct usb_device *usb, unsigned int pipe,
			   u8 request, u8 requesttype, unsigned short value,
			   unsigned short index, void *data,
			   unsigned short size, int timeout)
{
	(void)usb;
	(void)pipe;
	assert(request == 0x09);      /* SET_REPORT */
	assert(requesttype == 0x21); /* HID class, host->device, interface */
	assert(value == 0x0303);     /* feature report 3 */
	assert(index == APPLETBDRM_T1_HID_INTERFACE);
	(void)size;
	(void)timeout;
	if (data) {
		const unsigned char *buf = data;
		/* Report 3 display state byte: byte[1] must be 2 (AWAKE/on),
		 * never 1 (off/parked). */
		assert(buf[1] == 2);
	}
	return perform(UNPARK);
}

static int usb_clear_halt(struct usb_device *usb, unsigned int pipe)
{
	(void)usb;
	return perform((enum step)pipe);
}

static int appletbdrm_get_information(struct appletbdrm_device *adev)
{
	(void)adev;
	return perform(INFORMATION);
}

static int appletbdrm_signal_readiness(struct appletbdrm_device *adev)
{
	(void)adev;
	return perform(READINESS);
}

static int appletbdrm_clear_display(struct appletbdrm_device *adev)
{
	(void)adev;
	return perform(CLEAR);
}

static int drm_mode_config_helper_resume(struct drm_device *drm)
{
	(void)drm;
	return perform(RESTORE);
}

#include "appletbdrm_resume.h"

static void test_t1_resume(void)
{
	const int failures[] = { -EPIPE, -ETIMEDOUT, -ENODEV, -ENOMEM };
	const size_t count = sizeof(recovery) / sizeof(recovery[0]);
	/* UNPARK (index 5) is best-effort: a control-write fault is warned and
	 * rearm continues, so resume still succeeds. All other steps propagate. */
	const size_t unpark_call = 6; /* 1-based call position of UNPARK */

	for (size_t error = 0; error < sizeof(failures) / sizeof(failures[0]); ++error) {
		/* Include success plus a fault at each USB, protocol and DRM step. */
		for (size_t failed = 0; failed <= count; ++failed) {
			struct appletbdrm_device adev = { .t1 = true, .out_ep = 2, .in_ep = 5 };
			struct usb_interface intf = { .data = &adev };
			const bool fault_is_best_effort = (failed == unpark_call);
			const int expect_rc = failed ? failure : 0;

			calls = errors = 0;
			fail_at = failed;
			failure = failures[error];
			/* A fault on the best-effort unpark is absorbed: resume
			 * completes and returns success, but records a warning. */
			assert(appletbdrm_resume(&intf) == (fault_is_best_effort ? 0 : expect_rc));
			/* A best-effort fault is absorbed and rearm continues to the
			 * end, so every step runs; any other fault stops at `failed`. */
			assert(calls == (fault_is_best_effort ? count : (failed ? failed : count)));
			assert(intf.needs_binding == (failed != 0 && !fault_is_best_effort));
			assert(errors == (failed != 0));
			for (size_t i = 0; i < calls; ++i)
				assert(observed[i] == recovery[i]);
		}
	}
}

static void test_other_device_keeps_drm_resume_behavior(void)
{
	for (size_t failed = 0; failed <= 1; ++failed) {
		struct appletbdrm_device adev = { 0 };
		struct usb_interface intf = { .data = &adev };

		calls = errors = 0;
		fail_at = failed;
		failure = -EIO;
		assert(appletbdrm_resume(&intf) == (failed ? -EIO : 0));
		assert(calls == 1 && observed[0] == RESTORE);
		assert(intf.needs_binding == 0 && errors == 0);
	}
}

int main(void)
{
	test_t1_resume();
	test_other_device_keeps_drm_resume_behavior();
	puts("appletbdrm resume fault tests passed");
	return 0;
}
