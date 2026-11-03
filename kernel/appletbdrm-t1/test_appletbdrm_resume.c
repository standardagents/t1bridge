/* SPDX-License-Identifier: GPL-2.0 */
/* Run the real rearm and mode-restore path through the header it owns, with synthetic
 * USB and DRM boundaries and a fault injected at every step. The SOCW handshake —
 * discovery, the system-sleep notifier, the park decision, re-enumeration and the
 * registration pair — is a separate concern and runs in test_appletbdrm_socw.c. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>

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

#define adev_to_udev(adev) (&(adev)->usb)
#define usb_get_intfdata(intf) ((intf)->data)
#define appletbdrm_is_t1(adev) ((adev)->t1)

enum step { CLEAR_OUT, CLEAR_IN, INFORMATION, READINESS, CLEAR, RESTORE };

static const enum step recovery[] = {
	CLEAR_OUT, CLEAR_IN, INFORMATION, READINESS, CLEAR, CLEAR, RESTORE,
};

static enum step observed[16];
static size_t calls, fail_at;
static int failure, errors;

static int record(enum step step)
{
	assert(calls < sizeof(observed) / sizeof(observed[0]));

	observed[calls++] = step;

	return calls == fail_at ? failure : 0;
}

#define drm_err(drm, format, value) ((void)(drm), (void)(format), ++errors)

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

static int usb_clear_halt(struct usb_device *usb, unsigned int pipe)
{
	(void)usb;
	return record((enum step)pipe);
}

static int appletbdrm_get_information(struct appletbdrm_device *adev)
{
	(void)adev;
	return record(INFORMATION);
}

static int appletbdrm_signal_readiness(struct appletbdrm_device *adev)
{
	(void)adev;
	return record(READINESS);
}

static int appletbdrm_clear_display(struct appletbdrm_device *adev)
{
	(void)adev;
	return record(CLEAR);
}

static int drm_mode_config_helper_resume(struct drm_device *drm)
{
	(void)drm;
	return record(RESTORE);
}

#include "appletbdrm_resume.h"

/* Start a case from an empty recording. */
static void reset(void)
{
	calls = fail_at = 0;
	failure = errors = 0;
}

static void test_t1_resume(void)
{
	const int failures[] = { -EPIPE, -ETIMEDOUT, -ENODEV, -ENOMEM };
	const size_t count = sizeof(recovery) / sizeof(recovery[0]);

	for (size_t error = 0; error < sizeof(failures) / sizeof(failures[0]); ++error) {
		/* Include success plus a fault at each USB, protocol and DRM step. */
		for (size_t failed = 0; failed <= count; ++failed) {
			struct appletbdrm_device adev = { .t1 = true, .out_ep = 2, .in_ep = 5 };
			struct usb_interface intf = { .data = &adev };

			/* Clear the recording for each case. */
			reset();
			fail_at = failed;
			failure = failures[error];
			assert(appletbdrm_resume(&intf) == (failed ? failure : 0));
			assert(calls == (failed ? failed : count));
			assert(intf.needs_binding == (failed != 0));
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

		reset();
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
