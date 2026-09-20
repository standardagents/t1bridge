/* Root-only, T1-only FRST boundary. The reset target is discovered below the
 * actual USB host controller's ACPI node, never supplied as an AML path. */
#include <linux/acpi.h>
#include <linux/capability.h>
#include <linux/dmi.h>
#include <linux/miscdevice.h>
#include <linux/mutex.h>
#include <linux/uaccess.h>

struct t1_recovery_target {
	__u32 bus;
	__u32 address;
};

#define T1_RECOVERY_CHECK _IOW('T', 0x51, struct t1_recovery_target)
#define T1_RECOVERY_RESET _IOW('T', 0x52, struct t1_recovery_target)

static DEFINE_MUTEX(t1_recovery_mutex);
static bool t1_recovery_registered;

static bool t1_recovery_supported_host(void)
{
	return dmi_match(DMI_SYS_VENDOR, "Apple Inc.") &&
		(dmi_match(DMI_PRODUCT_NAME, "MacBookPro13,2") ||
		 dmi_match(DMI_PRODUCT_NAME, "MacBookPro13,3") ||
		 dmi_match(DMI_PRODUCT_NAME, "MacBookPro14,2") ||
		 dmi_match(DMI_PRODUCT_NAME, "MacBookPro14,3"));
}

struct t1_recovery_lookup {
	struct t1_recovery_target target;
	struct usb_device *device;
};

static int t1_recovery_find_usb(struct usb_device *device, void *opaque)
{
	struct t1_recovery_lookup *lookup = opaque;

	if (device->bus->busnum == lookup->target.bus &&
	    device->devnum == lookup->target.address &&
	    le16_to_cpu(device->descriptor.idVendor) == APPLE_VENDOR_ID &&
	    (le16_to_cpu(device->descriptor.idProduct) == 0x1281 ||
	     le16_to_cpu(device->descriptor.idProduct) == APPLE_T1_IBRIDGE_PRODUCT_ID))
		lookup->device = usb_get_dev(device);
	return 0;
}

static bool t1_recovery_has_hid(struct usb_device *device)
{
	int c, i, a;

	if (!device->config)
		return true;
	for (c = 0; c < device->descriptor.bNumConfigurations; c++) {
		struct usb_host_config *config = &device->config[c];

		for (i = 0; i < config->desc.bNumInterfaces; i++) {
			struct usb_interface_cache *cache = config->intf_cache[i];

			if (!cache)
				return true;
			for (a = 0; a < cache->num_altsetting; a++)
				if (cache->altsetting[a].desc.bInterfaceClass == USB_CLASS_HID)
					return true;
		}
	}
	return false;
}

struct t1_recovery_method {
	acpi_handle handle;
	unsigned int count;
};

static acpi_status t1_recovery_find_frst(acpi_handle handle, u32 level,
				       void *opaque, void **result)
{
	struct t1_recovery_method *method = opaque;
	struct acpi_device_info *info;
	acpi_status status;

	(void)level;
	(void)result;
	status = acpi_get_object_info(handle, &info);
	if (ACPI_FAILURE(status))
		return status;
	if (!memcmp(&info->name, "FRST", 4) && info->param_count == 0) {
		method->handle = handle;
		method->count++;
	}
	kfree(info);
	return AE_OK;
}

static long t1_recovery_ioctl(struct file *file, unsigned int command,
			      unsigned long argument)
{
	struct t1_recovery_lookup lookup = { };
	struct t1_recovery_method method = { };
	struct device *controller;
	acpi_handle root = NULL;
	acpi_status status;
	int result = -ENODEV;

	(void)file;
	if (command != T1_RECOVERY_CHECK && command != T1_RECOVERY_RESET)
		return -ENOTTY;
	if (!capable(CAP_SYS_RAWIO) || !t1_recovery_supported_host())
		return -EPERM;
	if (copy_from_user(&lookup.target, (void __user *)argument, sizeof(lookup.target)))
		return -EFAULT;
	if (!mutex_trylock(&t1_recovery_mutex))
		return -EBUSY;
	usb_for_each_dev(&lookup, t1_recovery_find_usb);
	if (!lookup.device)
		goto done;
	usb_lock_device(lookup.device);
	if (lookup.device->state == USB_STATE_NOTATTACHED ||
	    t1_recovery_has_hid(lookup.device))
		goto unlock;
	/* Follow the discovered controller's parent chain only. */
	for (controller = lookup.device->bus->controller; controller;
	     controller = controller->parent) {
		root = ACPI_HANDLE(controller);
		if (root)
			break;
	}
	if (!root)
		goto unlock;
	status = acpi_walk_namespace(ACPI_TYPE_METHOD, root, 16,
				     t1_recovery_find_frst, NULL, &method, NULL);
	if (ACPI_FAILURE(status) || method.count != 1)
		goto unlock;
	if (command == T1_RECOVERY_CHECK) {
		result = 0;
		goto unlock;
	}
	/* Reset is permitted only after a restore leaves the non-HID 8600
	 * personality. It cannot reset a working device or an arbitrary host. */
	if (le16_to_cpu(lookup.device->descriptor.idProduct) != APPLE_T1_IBRIDGE_PRODUCT_ID)
		goto unlock;
	status = acpi_evaluate_object(method.handle, NULL, NULL, NULL);
	result = ACPI_SUCCESS(status) ? 0 : -EIO;
unlock:
	usb_unlock_device(lookup.device);
	usb_put_dev(lookup.device);
done:
	mutex_unlock(&t1_recovery_mutex);
	return result;
}

static const struct file_operations t1_recovery_operations = {
	.owner = THIS_MODULE,
	.unlocked_ioctl = t1_recovery_ioctl,
};

static struct miscdevice t1_recovery_device = {
	.minor = MISC_DYNAMIC_MINOR,
	.name = "t1bridge-recovery",
	.fops = &t1_recovery_operations,
	.mode = 0600,
};

static int t1_recovery_register(void)
{
	int result;

	if (!t1_recovery_supported_host())
		return 0;
	result = misc_register(&t1_recovery_device);
	t1_recovery_registered = !result;
	return result;
}

static void t1_recovery_unregister(void)
{
	if (t1_recovery_registered)
		misc_deregister(&t1_recovery_device);
}
