/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Display messages and USB exchange; shared with synthetic transport tests.
 * Extracted from appletbdrm.c, Copyright (c) 2023 Kerem Karabay.
 */
#ifndef APPLETBDRM_PROTOCOL_H
#define APPLETBDRM_PROTOCOL_H

#define APPLETBDRM_PIXEL_FORMAT		cpu_to_le32(0x52474241) /* RGBA, the actual format is BGR888 */
#define APPLETBDRM_BITS_PER_PIXEL	24

#define APPLETBDRM_MSG_CLEAR_DISPLAY	cpu_to_le32(0x434c5244) /* CLRD */
#define APPLETBDRM_MSG_GET_INFORMATION	cpu_to_le32(0x47494e46) /* GINF */
#define APPLETBDRM_MSG_UPDATE_COMPLETE	cpu_to_le32(0x5544434c) /* UDCL */
#define APPLETBDRM_MSG_SIGNAL_READINESS	cpu_to_le32(0x52454459) /* REDY */

#define APPLETBDRM_BULK_MSG_TIMEOUT	1000
struct appletbdrm_msg_request_header {
	__le16 unk_00;
	__le16 unk_02;
	__le32 unk_04;
	__le32 unk_08;
	__le32 size;
} __packed;

struct appletbdrm_msg_response_header {
	u8 unk_00[16];
	__le32 msg;
} __packed;

struct appletbdrm_msg_simple_request {
	struct appletbdrm_msg_request_header header;
	__le32 msg;
	u8 unk_14[8];
	__le32 size;
} __packed;

struct appletbdrm_msg_information {
	struct appletbdrm_msg_response_header header;
	u8 unk_14[12];
	__le32 width;
	__le32 height;
	u8 bits_per_pixel;
	__le32 bytes_per_row;
	__le32 orientation;
	__le32 bitmap_info;
	__le32 pixel_format;
	__le32 width_inches;	/* floating point */
	__le32 height_inches;	/* floating point */
} __packed;

struct appletbdrm_frame {
	__le16 begin_x;
	__le16 begin_y;
	__le16 width;
	__le16 height;
	__le32 buf_size;
	u8 buf[];
} __packed;

struct appletbdrm_fb_request_footer {
	u8 unk_00[12];
	__le32 unk_0c;
	u8 unk_10[12];
	__le32 unk_1c;
	__le64 timestamp;
	u8 unk_28[12];
	__le32 unk_34;
	u8 unk_38[20];
	__le32 unk_4c;
} __packed;

struct appletbdrm_fb_request {
	struct appletbdrm_msg_request_header header;
	__le16 unk_10;
	u8 msg_id;
	u8 unk_13[29];
	/*
	 * Contents of `data`:
	 * - struct appletbdrm_frame frames[];
	 * - struct appletbdrm_fb_request_footer footer;
	 * - padding to make the total size a multiple of 16
	 */
	u8 data[];
} __packed;

struct appletbdrm_fb_request_response {
	struct appletbdrm_msg_response_header header;
	u8 unk_14[12];
	__le64 timestamp;
} __packed;

static int appletbdrm_send_request(struct appletbdrm_device *adev,
				   struct appletbdrm_msg_request_header *request, size_t size)
{
	struct usb_device *udev = adev_to_udev(adev);
	struct drm_device *drm = &adev->drm;
	int ret, actual_size;

	ret = usb_bulk_msg(udev, usb_sndbulkpipe(udev, adev->out_ep),
			   request, size, &actual_size, APPLETBDRM_BULK_MSG_TIMEOUT);
	if (ret) {
		drm_err(drm, "Failed to send message (%d)\n", ret);
		return ret;
	}

	if (actual_size < 0 || (size_t)actual_size != size) {
		drm_err(drm, "Actual size (%d) doesn't match expected size (%zu)\n",
			actual_size, size);
		return -EIO;
	}

	return 0;
}

static int appletbdrm_read_response(struct appletbdrm_device *adev,
				    struct appletbdrm_msg_response_header *response,
				    size_t size, __le32 expected_response)
{
	struct usb_device *udev = adev_to_udev(adev);
	struct drm_device *drm = &adev->drm;
	const char *kind = expected_response == APPLETBDRM_MSG_GET_INFORMATION ?
		"information" : expected_response == APPLETBDRM_MSG_UPDATE_COMPLETE ?
		"update" : "other";
	int ret, actual_size, attempt;

	for (attempt = 0; attempt < APPLETBDRM_T1_RESPONSE_ATTEMPTS; attempt++) {
		ret = usb_bulk_msg(udev, usb_rcvbulkpipe(udev, adev->in_ep),
				   response, size, &actual_size,
				   APPLETBDRM_BULK_MSG_TIMEOUT);
		if (ret) {
			dev_dbg(drm->dev, "t1bridge-display v=1 phase=reply kind=%s result=error code=%d attempts=%d\n",
				kind, ret, attempt + 1);
			drm_err(drm, "Failed to read response (%d)\n", ret);
			return ret;
		}

		if (actual_size >= 0 && (size_t)actual_size == size &&
		    response->msg == expected_response) {
			dev_dbg(drm->dev, "t1bridge-display v=1 phase=reply kind=%s result=ok code=0 attempts=%d\n",
				kind, attempt + 1);
			return 0;
		}

		if (!appletbdrm_is_t1(adev) &&
		    (actual_size < 0 || (size_t)actual_size < sizeof(*response) ||
		     response->msg != APPLETBDRM_MSG_SIGNAL_READINESS))
			break;
	}

	/* The break path has completed its current read; loop exhaustion has not. */
	if (attempt < APPLETBDRM_T1_RESPONSE_ATTEMPTS)
		attempt++;
	dev_dbg(drm->dev, "t1bridge-display v=1 phase=reply kind=%s result=error code=%d attempts=%d\n",
		kind, -EBADMSG, attempt);
	drm_err(drm, "No valid response after %d read attempts\n", attempt);
	return -EBADMSG;
}

static int appletbdrm_send_msg(struct appletbdrm_device *adev, __le32 msg)
{
	struct appletbdrm_msg_simple_request *request;
	int ret;

	request = kzalloc_obj(*request);
	if (!request)
		return -ENOMEM;

	request->header.unk_00 = cpu_to_le16(2);
	request->header.unk_02 = cpu_to_le16(0x1512);
	request->header.size = cpu_to_le32(sizeof(*request) - sizeof(request->header));
	request->msg = msg;
	request->size = request->header.size;

	ret = appletbdrm_send_request(adev, &request->header, sizeof(*request));

	kfree(request);

	return ret;
}

static int appletbdrm_clear_display(struct appletbdrm_device *adev)
{
	return appletbdrm_send_msg(adev, APPLETBDRM_MSG_CLEAR_DISPLAY);
}

static int appletbdrm_signal_readiness(struct appletbdrm_device *adev)
{
	return appletbdrm_send_msg(adev, APPLETBDRM_MSG_SIGNAL_READINESS);
}

static int appletbdrm_get_information(struct appletbdrm_device *adev)
{
	struct appletbdrm_msg_information *info;
	struct drm_device *drm = &adev->drm;
	u8 bits_per_pixel;
	__le32 pixel_format;
	int ret;

	info = kzalloc_obj(*info);
	if (!info)
		return -ENOMEM;

	ret = appletbdrm_send_msg(adev, APPLETBDRM_MSG_GET_INFORMATION);
	if (ret)
		goto free_info;

	ret = appletbdrm_read_response(adev, &info->header, sizeof(*info),
				       APPLETBDRM_MSG_GET_INFORMATION);
	if (ret)
		goto free_info;

	bits_per_pixel = info->bits_per_pixel;
	pixel_format = get_unaligned(&info->pixel_format);

	adev->width = get_unaligned_le32(&info->width);
	adev->height = get_unaligned_le32(&info->height);

	if (bits_per_pixel != APPLETBDRM_BITS_PER_PIXEL) {
		drm_err(drm, "Encountered unexpected bits per pixel value (%d)\n", bits_per_pixel);
		ret = -EINVAL;
		goto free_info;
	}

	if (pixel_format != APPLETBDRM_PIXEL_FORMAT) {
		drm_err(drm, "Encountered unknown pixel format (%p4cl)\n", &pixel_format);
		ret = -EINVAL;
		goto free_info;
	}

free_info:
	kfree(info);

	return ret;
}

#endif
