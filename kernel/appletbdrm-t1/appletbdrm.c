// SPDX-License-Identifier: GPL-2.0
/*
 * Apple Touch Bar DRM Driver
 *
 * Copyright (c) 2023 Kerem Karabay <kekrby@gmail.com>
 */

#include <linux/align.h>
#include <linux/array_size.h>
#include <linux/bitops.h>
#include <linux/bug.h>
#include <linux/container_of.h>
#include <linux/err.h>
#include <linux/module.h>
#include <linux/overflow.h>
#include <linux/slab.h>
#include <linux/types.h>
#include <linux/unaligned.h>
#include <linux/usb.h>
#include <linux/version.h>

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_crtc.h>
#include <drm/drm_damage_helper.h>
#include <drm/drm_drv.h>
#include <drm/drm_encoder.h>
#include <drm/drm_format_helper.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_gem_atomic_helper.h>
#include <drm/drm_gem_framebuffer_helper.h>
#include <drm/drm_gem_shmem_helper.h>
#include <drm/drm_modeset_helper.h>
#include <drm/drm_plane.h>
#include <drm/drm_print.h>
#include <drm/drm_probe_helper.h>

#define APPLETBDRM_T1_PRODUCT_ID		0x8600
#define APPLETBDRM_T1_HID_INTERFACE	6
#define APPLETBDRM_T1_RESPONSE_ATTEMPTS	8

#define drm_to_adev(_drm)		container_of(_drm, struct appletbdrm_device, drm)
#define adev_to_udev(adev)		interface_to_usbdev(to_usb_interface((adev)->drm.dev))

#if LINUX_VERSION_CODE >= KERNEL_VERSION(7, 2, 0)
typedef struct drm_atomic_commit appletbdrm_atomic_state;
#else
typedef struct drm_atomic_state appletbdrm_atomic_state;
#endif

struct appletbdrm_device {
	unsigned int in_ep;
	unsigned int out_ep;

	unsigned int width;
	unsigned int height;

	struct drm_device drm;
	struct drm_display_mode mode;
	struct drm_connector connector;
	struct drm_plane primary_plane;
	struct drm_crtc crtc;
	struct drm_encoder encoder;
};

static bool appletbdrm_is_t1(struct appletbdrm_device *adev)
{
	return le16_to_cpu(adev_to_udev(adev)->descriptor.idProduct) ==
	       APPLETBDRM_T1_PRODUCT_ID;
}

struct appletbdrm_plane_state {
	struct drm_shadow_plane_state base;
	struct appletbdrm_fb_request *request;
	struct appletbdrm_fb_request_response *response;
	size_t request_size;
	size_t frames_size;
};

static inline struct appletbdrm_plane_state *to_appletbdrm_plane_state(struct drm_plane_state *state)
{
	return container_of(state, struct appletbdrm_plane_state, base.base);
}

#include "appletbdrm_protocol.h"

static u32 rect_size(struct drm_rect *rect)
{
	return drm_rect_width(rect) * drm_rect_height(rect) *
		(BITS_TO_BYTES(APPLETBDRM_BITS_PER_PIXEL));
}

static int appletbdrm_connector_helper_get_modes(struct drm_connector *connector)
{
	struct appletbdrm_device *adev = drm_to_adev(connector->dev);

	return drm_connector_helper_get_modes_fixed(connector, &adev->mode);
}

static const u32 appletbdrm_primary_plane_formats[] = {
	DRM_FORMAT_BGR888,
	DRM_FORMAT_XRGB8888, /* emulated */
};

static int appletbdrm_primary_plane_helper_atomic_check(struct drm_plane *plane,
						   appletbdrm_atomic_state *state)
{
	struct drm_plane_state *new_plane_state = drm_atomic_get_new_plane_state(state, plane);
	struct drm_plane_state *old_plane_state = drm_atomic_get_old_plane_state(state, plane);
	struct drm_crtc *new_crtc = new_plane_state->crtc;
	struct drm_crtc_state *new_crtc_state = NULL;
	struct appletbdrm_plane_state *appletbdrm_state = to_appletbdrm_plane_state(new_plane_state);
	struct drm_atomic_helper_damage_iter iter;
	struct drm_rect damage;
	size_t frames_size = 0;
	size_t request_size;
	int ret;

	if (new_crtc)
		new_crtc_state = drm_atomic_get_new_crtc_state(state, new_crtc);

	ret = drm_atomic_helper_check_plane_state(new_plane_state, new_crtc_state,
						  DRM_PLANE_NO_SCALING,
						  DRM_PLANE_NO_SCALING,
						  false, false);
	if (ret)
		return ret;
	else if (!new_plane_state->visible)
		return 0;

	drm_atomic_helper_damage_iter_init(&iter, old_plane_state, new_plane_state);
	drm_atomic_for_each_plane_damage(&iter, &damage) {
		frames_size += struct_size((struct appletbdrm_frame *)0, buf, rect_size(&damage));
	}

	if (!frames_size)
		return 0;

	request_size = ALIGN(sizeof(struct appletbdrm_fb_request) +
		       frames_size +
		       sizeof(struct appletbdrm_fb_request_footer), 16);

	appletbdrm_state->request = kvzalloc(request_size, GFP_KERNEL);

	if (!appletbdrm_state->request)
		return -ENOMEM;

	appletbdrm_state->response = kzalloc_obj(*appletbdrm_state->response);

	if (!appletbdrm_state->response)
		return -ENOMEM;

	appletbdrm_state->request_size = request_size;
	appletbdrm_state->frames_size = frames_size;

	return 0;
}

static int appletbdrm_flush_damage(struct appletbdrm_device *adev,
				   struct drm_plane_state *old_state,
				   struct drm_plane_state *state)
{
	struct appletbdrm_plane_state *appletbdrm_state = to_appletbdrm_plane_state(state);
	struct drm_shadow_plane_state *shadow_plane_state = to_drm_shadow_plane_state(state);
	struct appletbdrm_fb_request_response *response = appletbdrm_state->response;
	struct appletbdrm_fb_request_footer *footer;
	struct drm_atomic_helper_damage_iter iter;
	struct drm_framebuffer *fb = state->fb;
	struct appletbdrm_fb_request *request = appletbdrm_state->request;
	struct drm_device *drm = &adev->drm;
	struct appletbdrm_frame *frame;
	u64 timestamp = ktime_get_ns();
	struct drm_rect damage;
	size_t frames_size = appletbdrm_state->frames_size;
	size_t request_size = appletbdrm_state->request_size;
	int ret;

	if (!frames_size)
		return 0;

	ret = drm_gem_fb_begin_cpu_access(fb, DMA_FROM_DEVICE);
	if (ret) {
		drm_err(drm, "Failed to start CPU framebuffer access (%d)\n", ret);
		goto end_fb_cpu_access;
	}

	request->header.unk_00 = cpu_to_le16(2);
	request->header.unk_02 = cpu_to_le16(0x12);
	request->header.unk_04 = cpu_to_le32(9);
	request->header.size = cpu_to_le32(request_size - sizeof(request->header));
	request->unk_10 = cpu_to_le16(1);
	request->msg_id = timestamp;

	frame = (struct appletbdrm_frame *)request->data;

	drm_atomic_helper_damage_iter_init(&iter, old_state, state);
	drm_atomic_for_each_plane_damage(&iter, &damage) {
		struct drm_rect dst_clip = state->dst;
		struct iosys_map dst = IOSYS_MAP_INIT_VADDR(frame->buf);
		u32 buf_size = rect_size(&damage);

		if (!drm_rect_intersect(&dst_clip, &damage))
			continue;

		/*
		 * The coordinates need to be translated to the coordinate
		 * system the device expects, see the comment in
		 * appletbdrm_setup_mode_config
		 */
		frame->begin_x = cpu_to_le16(damage.y1);
		frame->begin_y = cpu_to_le16(adev->height - damage.x2);
		frame->width = cpu_to_le16(drm_rect_height(&damage));
		frame->height = cpu_to_le16(drm_rect_width(&damage));
		frame->buf_size = cpu_to_le32(buf_size);

		switch (fb->format->format) {
		case DRM_FORMAT_XRGB8888:
			drm_fb_xrgb8888_to_bgr888(&dst, NULL, &shadow_plane_state->data[0], fb, &damage, &shadow_plane_state->fmtcnv_state);
			break;
		default:
			drm_fb_memcpy(&dst, NULL, &shadow_plane_state->data[0], fb, &damage);
			break;
		}

		frame = (void *)frame + struct_size(frame, buf, buf_size);
	}

	footer = (struct appletbdrm_fb_request_footer *)&request->data[frames_size];

	footer->unk_0c = cpu_to_le32(0xfffe);
	footer->unk_1c = cpu_to_le32(0x80001);
	footer->unk_34 = cpu_to_le32(0x80002);
	footer->unk_4c = cpu_to_le32(0xffff);
	footer->timestamp = cpu_to_le64(timestamp);

	ret = appletbdrm_send_request(adev, &request->header, request_size);
	if (ret)
		goto end_fb_cpu_access;

	ret = appletbdrm_read_response(adev, &response->header, sizeof(*response),
				       APPLETBDRM_MSG_UPDATE_COMPLETE);
	if (ret)
		goto end_fb_cpu_access;

	if (!appletbdrm_is_t1(adev) &&
	    response->timestamp != footer->timestamp) {
		drm_err(drm, "Response timestamp (%llu) doesn't match request timestamp (%llu)\n",
			le64_to_cpu(response->timestamp), timestamp);
		ret = -EBADMSG;
		goto end_fb_cpu_access;
	}

end_fb_cpu_access:
	drm_gem_fb_end_cpu_access(fb, DMA_FROM_DEVICE);

	return ret;
}

static void appletbdrm_primary_plane_helper_atomic_update(struct drm_plane *plane,
						     appletbdrm_atomic_state *old_state)
{
	struct appletbdrm_device *adev = drm_to_adev(plane->dev);
	struct drm_device *drm = plane->dev;
	struct drm_plane_state *plane_state = plane->state;
	struct drm_plane_state *old_plane_state = drm_atomic_get_old_plane_state(old_state, plane);
	int idx;

	if (!drm_dev_enter(drm, &idx))
		return;

	appletbdrm_flush_damage(adev, old_plane_state, plane_state);

	drm_dev_exit(idx);
}

static void appletbdrm_primary_plane_helper_atomic_disable(struct drm_plane *plane,
							   appletbdrm_atomic_state *state)
{
	struct drm_device *dev = plane->dev;
	struct appletbdrm_device *adev = drm_to_adev(dev);
	int idx;

	if (!drm_dev_enter(dev, &idx))
		return;

	appletbdrm_clear_display(adev);

	drm_dev_exit(idx);
}

static void appletbdrm_primary_plane_destroy_state(struct drm_plane *plane,
					   struct drm_plane_state *state);

static void appletbdrm_primary_plane_reset(struct drm_plane *plane)
{
	struct appletbdrm_plane_state *appletbdrm_state;

	if (plane->state) {
		appletbdrm_primary_plane_destroy_state(plane, plane->state);
		plane->state = NULL;
	}

	appletbdrm_state = kzalloc_obj(*appletbdrm_state);
	if (!appletbdrm_state)
		return;

	__drm_gem_reset_shadow_plane(plane, &appletbdrm_state->base);
}

static struct drm_plane_state *appletbdrm_primary_plane_duplicate_state(struct drm_plane *plane)
{
	struct drm_shadow_plane_state *new_shadow_plane_state;
	struct appletbdrm_plane_state *appletbdrm_state;

	if (WARN_ON(!plane->state))
		return NULL;

	appletbdrm_state = kzalloc_obj(*appletbdrm_state);
	if (!appletbdrm_state)
		return NULL;

	/* Request and response are not duplicated and are allocated in .atomic_check */
	appletbdrm_state->request = NULL;
	appletbdrm_state->response = NULL;

	appletbdrm_state->request_size = 0;
	appletbdrm_state->frames_size = 0;

	new_shadow_plane_state = &appletbdrm_state->base;

	__drm_gem_duplicate_shadow_plane_state(plane, new_shadow_plane_state);

	return &new_shadow_plane_state->base;
}

static void appletbdrm_primary_plane_destroy_state(struct drm_plane *plane,
						   struct drm_plane_state *state)
{
	struct appletbdrm_plane_state *appletbdrm_state = to_appletbdrm_plane_state(state);

	kvfree(appletbdrm_state->request);
	kfree(appletbdrm_state->response);

	__drm_gem_destroy_shadow_plane_state(&appletbdrm_state->base);

	kfree(appletbdrm_state);
}

static const struct drm_plane_helper_funcs appletbdrm_primary_plane_helper_funcs = {
	DRM_GEM_SHADOW_PLANE_HELPER_FUNCS,
	.atomic_check = appletbdrm_primary_plane_helper_atomic_check,
	.atomic_update = appletbdrm_primary_plane_helper_atomic_update,
	.atomic_disable = appletbdrm_primary_plane_helper_atomic_disable,
};

static const struct drm_plane_funcs appletbdrm_primary_plane_funcs = {
	.update_plane = drm_atomic_helper_update_plane,
	.disable_plane = drm_atomic_helper_disable_plane,
	.reset = appletbdrm_primary_plane_reset,
	.atomic_duplicate_state = appletbdrm_primary_plane_duplicate_state,
	.atomic_destroy_state = appletbdrm_primary_plane_destroy_state,
	.destroy = drm_plane_cleanup,
};

static enum drm_mode_status appletbdrm_crtc_helper_mode_valid(struct drm_crtc *crtc,
							  const struct drm_display_mode *mode)
{
	struct appletbdrm_device *adev = drm_to_adev(crtc->dev);

	return drm_crtc_helper_mode_valid_fixed(crtc, mode, &adev->mode);
}

static const struct drm_mode_config_funcs appletbdrm_mode_config_funcs = {
	.fb_create = drm_gem_fb_create_with_dirty,
	.atomic_check = drm_atomic_helper_check,
	.atomic_commit = drm_atomic_helper_commit,
};

static const struct drm_connector_funcs appletbdrm_connector_funcs = {
	.reset = drm_atomic_helper_connector_reset,
	.destroy = drm_connector_cleanup,
	.fill_modes = drm_helper_probe_single_connector_modes,
	.atomic_destroy_state = drm_atomic_helper_connector_destroy_state,
	.atomic_duplicate_state = drm_atomic_helper_connector_duplicate_state,
};

static const struct drm_connector_helper_funcs appletbdrm_connector_helper_funcs = {
	.get_modes = appletbdrm_connector_helper_get_modes,
};

static const struct drm_crtc_helper_funcs appletbdrm_crtc_helper_funcs = {
	.mode_valid = appletbdrm_crtc_helper_mode_valid,
};

static const struct drm_crtc_funcs appletbdrm_crtc_funcs = {
	.reset = drm_atomic_helper_crtc_reset,
	.destroy = drm_crtc_cleanup,
	.set_config = drm_atomic_helper_set_config,
	.page_flip = drm_atomic_helper_page_flip,
	.atomic_duplicate_state = drm_atomic_helper_crtc_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_crtc_destroy_state,
};

static const struct drm_encoder_funcs appletbdrm_encoder_funcs = {
	.destroy = drm_encoder_cleanup,
};

DEFINE_DRM_GEM_FOPS(appletbdrm_drm_fops);

static const struct drm_driver appletbdrm_drm_driver = {
	DRM_GEM_SHMEM_DRIVER_OPS,
	.name			= "appletbdrm",
	.desc			= "Apple Touch Bar DRM Driver",
	.major			= 1,
	.minor			= 0,
	.driver_features	= DRIVER_MODESET | DRIVER_GEM | DRIVER_ATOMIC,
	.fops			= &appletbdrm_drm_fops,
};

static int appletbdrm_setup_mode_config(struct appletbdrm_device *adev)
{
	struct drm_connector *connector = &adev->connector;
	struct drm_plane *primary_plane;
	struct drm_crtc *crtc;
	struct drm_encoder *encoder;
	struct drm_device *drm = &adev->drm;
	int ret;

	ret = drmm_mode_config_init(drm);
	if (ret) {
		drm_err(drm, "Failed to initialize mode configuration\n");
		return ret;
	}

	primary_plane = &adev->primary_plane;
	ret = drm_universal_plane_init(drm, primary_plane, 0,
				       &appletbdrm_primary_plane_funcs,
				       appletbdrm_primary_plane_formats,
				       ARRAY_SIZE(appletbdrm_primary_plane_formats),
				       NULL,
				       DRM_PLANE_TYPE_PRIMARY, NULL);
	if (ret) {
		drm_err(drm, "Failed to initialize universal plane object\n");
		return ret;
	}

	drm_plane_helper_add(primary_plane, &appletbdrm_primary_plane_helper_funcs);
	drm_plane_enable_fb_damage_clips(primary_plane);

	crtc = &adev->crtc;
	ret = drm_crtc_init_with_planes(drm, crtc, primary_plane, NULL,
					&appletbdrm_crtc_funcs, NULL);
	if (ret) {
		drm_err(drm, "Failed to initialize CRTC object\n");
		return ret;
	}

	drm_crtc_helper_add(crtc, &appletbdrm_crtc_helper_funcs);

	encoder = &adev->encoder;
	ret = drm_encoder_init(drm, encoder, &appletbdrm_encoder_funcs,
			       DRM_MODE_ENCODER_DAC, NULL);
	if (ret) {
		drm_err(drm, "Failed to initialize encoder\n");
		return ret;
	}

	encoder->possible_crtcs = drm_crtc_mask(crtc);

	/*
	 * The coordinate system used by the device is different from the
	 * coordinate system of the framebuffer in that the x and y axes are
	 * swapped, and that the y axis is inverted; so what the device reports
	 * as the height is actually the width of the framebuffer and vice
	 * versa.
	 */
	drm->mode_config.max_width = max(adev->height, DRM_SHADOW_PLANE_MAX_WIDTH);
	drm->mode_config.max_height = max(adev->width, DRM_SHADOW_PLANE_MAX_HEIGHT);
	drm->mode_config.preferred_depth = APPLETBDRM_BITS_PER_PIXEL;
	drm->mode_config.funcs = &appletbdrm_mode_config_funcs;

	adev->mode = (struct drm_display_mode) {
		DRM_MODE_INIT(60, adev->height, adev->width,
			      DRM_MODE_RES_MM(adev->height, 218),
			      DRM_MODE_RES_MM(adev->width, 218))
	};

	ret = drm_connector_init(drm, connector,
				 &appletbdrm_connector_funcs, DRM_MODE_CONNECTOR_USB);
	if (ret) {
		drm_err(drm, "Failed to initialize connector\n");
		return ret;
	}

	drm_connector_helper_add(connector, &appletbdrm_connector_helper_funcs);

	ret = drm_connector_set_panel_orientation(connector,
						  DRM_MODE_PANEL_ORIENTATION_RIGHT_UP);
	if (ret) {
		drm_err(drm, "Failed to set panel orientation\n");
		return ret;
	}

	connector->display_info.non_desktop = true;
	ret = drm_object_property_set_value(&connector->base,
					    drm->mode_config.non_desktop_property, true);
	if (ret) {
		drm_err(drm, "Failed to set non-desktop property\n");
		return ret;
	}

	ret = drm_connector_attach_encoder(connector, encoder);

	if (ret) {
		drm_err(drm, "Failed to initialize simple display pipe\n");
		return ret;
	}

	drm_mode_config_reset(drm);

	return 0;
}

static int appletbdrm_probe(struct usb_interface *intf,
			    const struct usb_device_id *id)
{
	struct usb_endpoint_descriptor *bulk_in, *bulk_out;
	struct device *dev = &intf->dev;
	struct appletbdrm_device *adev;
	struct drm_device *drm = NULL;
	struct device *dma_dev;
	int ret;

	ret = usb_find_common_endpoints(intf->cur_altsetting, &bulk_in, &bulk_out, NULL, NULL);
	if (ret) {
		drm_err(drm, "appletbdrm: Failed to find bulk endpoints\n");
		return ret;
	}

	adev = devm_drm_dev_alloc(dev, &appletbdrm_drm_driver, struct appletbdrm_device, drm);
	if (IS_ERR(adev))
		return PTR_ERR(adev);

	adev->in_ep = bulk_in->bEndpointAddress;
	adev->out_ep = bulk_out->bEndpointAddress;

	drm = &adev->drm;

	usb_set_intfdata(intf, adev);

	dma_dev = usb_intf_get_dma_device(intf);
	if (dma_dev) {
		drm_dev_set_dma_dev(drm, dma_dev);
		put_device(dma_dev);
	} else {
		drm_warn(drm, "buffer sharing not supported"); /* not an error */
	}

	if (appletbdrm_is_t1(adev)) {
		usb_clear_halt(adev_to_udev(adev),
			       usb_sndbulkpipe(adev_to_udev(adev), adev->out_ep));
		usb_clear_halt(adev_to_udev(adev),
			       usb_rcvbulkpipe(adev_to_udev(adev), adev->in_ep));
	}

	ret = appletbdrm_get_information(adev);
	if (ret) {
		drm_err(drm, "Failed to get display information\n");
		return ret;
	}

	ret = appletbdrm_signal_readiness(adev);
	if (ret) {
		drm_err(drm, "Failed to signal readiness\n");
		return ret;
	}

	ret = appletbdrm_setup_mode_config(adev);
	if (ret) {
		drm_err(drm, "Failed to setup mode config\n");
		return ret;
	}

	ret = drm_dev_register(drm, 0);
	if (ret) {
		drm_err(drm, "Failed to register DRM device\n");
		return ret;
	}

	ret = appletbdrm_clear_display(adev);
	if (ret) {
		drm_err(drm, "Failed to clear display\n");
		return ret;
	}

	return 0;
}

static void appletbdrm_disconnect(struct usb_interface *intf)
{
	struct appletbdrm_device *adev = usb_get_intfdata(intf);
	struct drm_device *drm = &adev->drm;

	drm_dev_unplug(drm);
	drm_atomic_helper_shutdown(drm);
}

static void appletbdrm_shutdown(struct usb_interface *intf)
{
	struct appletbdrm_device *adev = usb_get_intfdata(intf);

	/*
	 * The framebuffer needs to be cleared on shutdown since its content
	 * persists across boots
	 */
	drm_atomic_helper_shutdown(&adev->drm);
}

static void appletbdrm_t1_park(struct appletbdrm_device *adev)
{
	static const u8 report[15] = { 0x03, 0x01, 0xf4, 0x01 };
	struct usb_device *udev = adev_to_udev(adev);
	u8 *buffer;
	int ret;

	appletbdrm_clear_display(adev);
	appletbdrm_clear_display(adev);

	buffer = kmemdup(report, sizeof(report), GFP_KERNEL);
	if (!buffer)
		return;

	ret = usb_control_msg(udev, usb_sndctrlpipe(udev, 0), 0x09, 0x21,
			      0x0303, APPLETBDRM_T1_HID_INTERFACE, buffer,
			      sizeof(report), USB_CTRL_SET_TIMEOUT);
	kfree(buffer);
	if (ret < 0)
		drm_warn(&adev->drm, "Failed to park T1 display (%d)\n", ret);

	appletbdrm_clear_display(adev);
}

static int appletbdrm_suspend(struct usb_interface *intf, pm_message_t message)
{
	struct appletbdrm_device *adev = usb_get_intfdata(intf);
	int ret;

	ret = drm_mode_config_helper_suspend(&adev->drm);
	if (!ret && appletbdrm_is_t1(adev))
		appletbdrm_t1_park(adev);

	return ret;
}

#include "appletbdrm_resume.h"

static const struct usb_device_id appletbdrm_usb_id_table[] = {
	{ USB_DEVICE_INTERFACE_CLASS(0x05ac, 0x8302, USB_CLASS_AUDIO_VIDEO) },
	{ USB_DEVICE_INTERFACE_CLASS(0x05ac, APPLETBDRM_T1_PRODUCT_ID,
				     USB_CLASS_AUDIO_VIDEO) },
	{}
};
MODULE_DEVICE_TABLE(usb, appletbdrm_usb_id_table);

static struct usb_driver appletbdrm_usb_driver = {
	.name		= "appletbdrm",
	.probe		= appletbdrm_probe,
	.disconnect	= appletbdrm_disconnect,
	.shutdown	= appletbdrm_shutdown,
	.suspend	= appletbdrm_suspend,
	.resume		= appletbdrm_resume,
	.reset_resume	= appletbdrm_resume,
	.id_table	= appletbdrm_usb_id_table,
};
module_usb_driver(appletbdrm_usb_driver);

MODULE_DESCRIPTION("Apple Touch Bar DRM Driver");
MODULE_LICENSE("GPL");
