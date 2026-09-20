/*
 * Select the Apple T1 iBridge display configuration before USB interfaces
 * are populated. This is an independent implementation following the Linux
 * rtl8152 configuration-selector pattern.
 */

#include <linux/module.h>
#include <linux/usb.h>
#include <linux/usb/cdc.h>

#define APPLE_VENDOR_ID 0x05ac
#define APPLE_T1_IBRIDGE_PRODUCT_ID 0x8600

#define T1_DISPLAY_INTERFACE 3
#define T1_NCM_CONTROL_INTERFACE 4
#define T1_NCM_DATA_INTERFACE 5
#define T1_SEP_INTERFACE 7

#define T1_DISPLAY_IN_ENDPOINT 0x85
#define T1_DISPLAY_OUT_ENDPOINT 0x02
#define T1_NCM_IN_ENDPOINT 0x86
#define T1_NCM_OUT_ENDPOINT 0x04
#define T1_SEP_IN_ENDPOINT 0x88
#define T1_SEP_OUT_ENDPOINT 0x05

#define T1_SEP_SUBCLASS 0xf9
#define T1_SEP_PROTOCOL 0x11

#include "t1_recovery_reset.h"

static const struct usb_host_interface *
t1_cfgsel_find_altsetting(const struct usb_host_config *config,
			  u8 interface_number, u8 alternate_setting)
{
	const struct usb_interface_cache *cache;
	const struct usb_host_interface *alt;
	int i, j;

	for (i = 0; i < config->desc.bNumInterfaces; i++) {
		cache = config->intf_cache[i];
		if (!cache)
			continue;

		for (j = 0; j < cache->num_altsetting; j++) {
			alt = &cache->altsetting[j];
			if (alt->desc.bInterfaceNumber == interface_number &&
			    alt->desc.bAlternateSetting == alternate_setting)
				return alt;
		}
	}

	return NULL;
}

static bool t1_cfgsel_has_bulk_endpoint(const struct usb_host_interface *alt,
					u8 address)
{
	const struct usb_endpoint_descriptor *endpoint;
	int i;

	for (i = 0; i < alt->desc.bNumEndpoints; i++) {
		endpoint = &alt->endpoint[i].desc;
		if (endpoint->bEndpointAddress == address &&
		    usb_endpoint_xfer_bulk(endpoint))
			return true;
	}

	return false;
}

static bool t1_cfgsel_interface_matches(const struct usb_host_interface *alt,
					u8 class, u8 subclass,
					u8 protocol)
{
	return alt && alt->desc.bInterfaceClass == class &&
	       alt->desc.bInterfaceSubClass == subclass &&
	       alt->desc.bInterfaceProtocol == protocol;
}

static bool t1_cfgsel_is_display_configuration(
	const struct usb_host_config *config)
{
	const struct usb_host_interface *display;
	const struct usb_host_interface *ncm_control;
	const struct usb_host_interface *ncm_data;
	const struct usb_host_interface *sep;

	if (config->desc.bNumInterfaces != 8)
		return false;

	display = t1_cfgsel_find_altsetting(config, T1_DISPLAY_INTERFACE, 0);
	ncm_control = t1_cfgsel_find_altsetting(
		config, T1_NCM_CONTROL_INTERFACE, 0);
	ncm_data = t1_cfgsel_find_altsetting(config, T1_NCM_DATA_INTERFACE, 1);
	sep = t1_cfgsel_find_altsetting(config, T1_SEP_INTERFACE, 0);

	if (!t1_cfgsel_interface_matches(display, USB_CLASS_AUDIO_VIDEO, 0, 0) ||
	    !t1_cfgsel_has_bulk_endpoint(display, T1_DISPLAY_IN_ENDPOINT) ||
	    !t1_cfgsel_has_bulk_endpoint(display, T1_DISPLAY_OUT_ENDPOINT))
		return false;

	if (!t1_cfgsel_interface_matches(ncm_control, USB_CLASS_COMM,
					 USB_CDC_SUBCLASS_NCM, 0) ||
	    ncm_control->desc.bNumEndpoints != 0)
		return false;

	if (!t1_cfgsel_interface_matches(ncm_data, USB_CLASS_CDC_DATA, 0, 1) ||
	    !t1_cfgsel_has_bulk_endpoint(ncm_data, T1_NCM_IN_ENDPOINT) ||
	    !t1_cfgsel_has_bulk_endpoint(ncm_data, T1_NCM_OUT_ENDPOINT))
		return false;

	return t1_cfgsel_interface_matches(sep, USB_CLASS_VENDOR_SPEC,
					   T1_SEP_SUBCLASS, T1_SEP_PROTOCOL) &&
	       t1_cfgsel_has_bulk_endpoint(sep, T1_SEP_IN_ENDPOINT) &&
	       t1_cfgsel_has_bulk_endpoint(sep, T1_SEP_OUT_ENDPOINT);
}

static int t1_cfgsel_choose_configuration(struct usb_device *udev)
{
	struct usb_host_config *config = udev->config;
	int chosen = -ENODEV;
	int i;

	for (i = 0; i < udev->descriptor.bNumConfigurations; i++, config++) {
		if (!t1_cfgsel_is_display_configuration(config))
			continue;

		/* Ambiguous matching configurations are treated as unsupported. */
		if (chosen >= 0)
			return -ENODEV;

		chosen = config->desc.bConfigurationValue;
	}

	return chosen;
}

static const struct usb_device_id t1_cfgsel_devices[] = {
	{ USB_DEVICE(APPLE_VENDOR_ID, APPLE_T1_IBRIDGE_PRODUCT_ID) },
	{ }
};
MODULE_DEVICE_TABLE(usb, t1_cfgsel_devices);
/* Recovery-mode machines also need the guarded reset interface loaded. */
MODULE_ALIAS("usb:v05ACp1281d*dc*dsc*dp*ic*isc*ip*in*");

static struct usb_device_driver t1_cfgsel_driver = {
	.name = "t1bridge-cfgselector",
	.choose_configuration = t1_cfgsel_choose_configuration,
	.id_table = t1_cfgsel_devices,
	.generic_subclass = 1,
	.supports_autosuspend = 1,
};

static int __init t1_cfgsel_init(void)
{
	int result = usb_register_device_driver(&t1_cfgsel_driver, THIS_MODULE);

	if (result)
		return result;
	result = t1_recovery_register();
	if (result)
		usb_deregister_device_driver(&t1_cfgsel_driver);
	return result;
}

static void __exit t1_cfgsel_exit(void)
{
	t1_recovery_unregister();
	usb_deregister_device_driver(&t1_cfgsel_driver);
}

module_init(t1_cfgsel_init);
module_exit(t1_cfgsel_exit);

MODULE_DESCRIPTION("Apple T1 iBridge display configuration selector");
MODULE_LICENSE("Dual MIT/GPL");
