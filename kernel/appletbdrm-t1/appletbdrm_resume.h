/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Private resume callbacks, included after the driver's USB/DRM operations.
 * Kept together so fault tests exercise the production recovery sequence.
 */
#ifndef APPLETBDRM_RESUME_H
#define APPLETBDRM_RESUME_H

/* Forward decl: defined below with rearm, used by probe() in the driver. */
static void appletbdrm_t1_unpark(struct appletbdrm_device *adev);

static int appletbdrm_t1_rearm(struct appletbdrm_device *adev)
{
	struct usb_device *udev = adev_to_udev(adev);
	int ret;

	ret = usb_clear_halt(udev, usb_sndbulkpipe(udev, adev->out_ep));
	if (ret)
		return ret;
	ret = usb_clear_halt(udev, usb_rcvbulkpipe(udev, adev->in_ep));
	if (ret)
		return ret;

	ret = appletbdrm_get_information(adev);
	if (ret)
		return ret;
	ret = appletbdrm_signal_readiness(adev);
	if (ret)
		return ret;
	ret = appletbdrm_clear_display(adev);
	if (ret)
		return ret;

	/*
	 * park() left the display byte at 1 (off); re-assert it to 2 (on) so the
	 * panel re-lights after resume. See appletbdrm_t1_unpark().
	 */
	appletbdrm_t1_unpark(adev);

	return appletbdrm_clear_display(adev);
}

/*
 * Re-assert the T1 display byte to ON (2). appletbdrm_t1_park() leaves the
 * display byte at 1 (off) on suspend, but neither probe() nor rearm() ever
 * writes it back to 2, so a panel that was lit at firmware goes black once the
 * driver takes over and nothing re-lights it. This mirrors park() exactly, with
 * byte[1] = 2 (awake).
 */
static void appletbdrm_t1_unpark(struct appletbdrm_device *adev)
{
	static const u8 report[15] = { 0x03, 0x02, 0xf4, 0x01 };
	struct usb_device *udev = adev_to_udev(adev);
	u8 *buffer;
	int ret;

	buffer = kmemdup(report, sizeof(report), GFP_KERNEL);
	if (!buffer)
		return;

	ret = usb_control_msg(udev, usb_sndctrlpipe(udev, 0), 0x09, 0x21,
			      0x0303, APPLETBDRM_T1_HID_INTERFACE, buffer,
			      sizeof(report), USB_CTRL_SET_TIMEOUT);
	kfree(buffer);
	if (ret < 0)
		drm_warn(&adev->drm, "Failed to unpark T1 display (%d)\n", ret);
}

static int appletbdrm_resume(struct usb_interface *intf)
{
	struct appletbdrm_device *adev = usb_get_intfdata(intf);
	int ret;

	if (appletbdrm_is_t1(adev)) {
		ret = appletbdrm_t1_rearm(adev);
		if (ret) {
			drm_err(&adev->drm, "Failed to rearm T1 display (%d)\n", ret);
			goto rebind;
		}
	}

	ret = drm_mode_config_helper_resume(&adev->drm);
	if (!ret || !appletbdrm_is_t1(adev))
		return ret;
	drm_err(&adev->drm, "Failed to restore T1 display mode (%d)\n", ret);

rebind:
	/*
	 * A failed resume callback alone does not request reprobe. USB core
	 * unbinds marked interfaces after resume and rebinds at PM completion,
	 * outside this callback's locks. Recreate only the display interface;
	 * never reset the composite device or leave a stale DRM node active.
	 */
	intf->needs_binding = 1;
	return ret;
}

#endif
