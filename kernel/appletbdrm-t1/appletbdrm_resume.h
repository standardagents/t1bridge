/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Private resume callbacks, included after the driver's USB/DRM operations.
 * Kept together so fault tests exercise the production recovery sequence.
 */
#ifndef APPLETBDRM_RESUME_H
#define APPLETBDRM_RESUME_H

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
	ret = appletbdrm_clear_display(adev);
	if (ret)
		return ret;

	return appletbdrm_t1_unpark(adev);
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
