/* SPDX-License-Identifier: GPL-2.0 */
/*
 * The T1 host-sleep handshake, included after the driver's USB/DRM operations, where
 * probe discovers the method.
 *
 * SOCW is the unique single-argument SOCW method below the host controller's ACPI node,
 * looked up only for a device the driver has identified as a T1, and the walk that finds
 * it is bounded. The driver keeps the handle for the module and never removes a
 * namespace node, so it uses the handle only while the module is loaded.
 *
 * The pair, PM_SUSPEND_PREPARE and PM_POST_SUSPEND, is the only path that evaluates the
 * method: SOCW is a blocking ACPICA evaluation, so it runs in process context from the
 * notifier and never at probe.
 *
 * Park is not the sleep path when the sleep write is acknowledged: it only sets the
 * display brightness factor to 0, so the panel stays powered and keeps refreshing, and
 * on one MacBookPro14,3 part of the strip strobed for the whole sleep. It stays the
 * sleep path for a device whose SOCW method was not discovered, for a write the T1 did
 * not acknowledge, and for an event that is not a plain system sleep.
 *
 * The state below is module-wide rather than per interface, because the interface that
 * went to sleep may be gone when the system wakes. Probe, the system-sleep notifier and
 * the USB suspend callback all reach it, so it is guarded by one mutex that is not held
 * across the blocking evaluation; the notifier callbacks themselves are serialised by
 * the PM core.
 */
#ifndef APPLETBDRM_SOCW_H
#define APPLETBDRM_SOCW_H

#define APPLETBDRM_SOCW_SLEEP			0	/* argument 0: the host is sleeping */
#define APPLETBDRM_SOCW_AWAKE			1	/* argument 1: the host is awake */
#define APPLETBDRM_SOCW_ACK			0	/* the T1 acknowledged the write */

/* The walk is bounded, and a SOCW method deeper than the walk is not found, so nothing
 * is kept. The bound mirrors the one used for FRST in t1-cfgsel. */
#define APPLETBDRM_SOCW_DEPTH			16

static DEFINE_MUTEX(appletbdrm_socw_lock);
static acpi_handle appletbdrm_socw_handle;
static bool appletbdrm_socw_sent;		/* SOCW(0) was attempted for this sleep */
static bool appletbdrm_socw_acked;		/* and it returned 0 */
static bool appletbdrm_socw_held;		/* and the module reference taken for this sleep is held */

#if IS_ENABLED(CONFIG_ACPI)

struct appletbdrm_socw_method {
	acpi_handle handle;
	unsigned int count;
};

static acpi_status appletbdrm_socw_count_method(acpi_handle handle, u32 level,
						void *opaque, void **result)
{
	struct appletbdrm_socw_method *method = opaque;
	struct acpi_device_info *info;
	acpi_status status;

	(void)level;
	(void)result;

	status = acpi_get_object_info(handle, &info);
	if (ACPI_FAILURE(status))
		return status;

	if (!memcmp(&info->name, "SOCW", 4) && info->param_count == 1) {
		method->handle = handle;
		method->count++;
	}

	kfree(info);

	return AE_OK;
}

/* Discovery keeps a handle only when the bounded walk below the actual host controller's
 * ACPI node has exactly one single-argument SOCW method. The path is never supplied as an
 * AML name, and nothing here evaluates the method. The first successful discovery is the
 * one kept: a device that enumerates in the place of a disconnected T1 does not replace
 * it, so the wake write can still be sent for the sleep that the old device began. */
static void appletbdrm_socw_find(struct appletbdrm_device *adev)
{
	struct appletbdrm_socw_method method = { 0, 0 };
	struct device *controller;
	acpi_handle root = NULL;
	acpi_status status;

	for (controller = adev_to_udev(adev)->bus->controller; controller;
	     controller = controller->parent) {
		root = ACPI_HANDLE(controller);
		if (root)
			break;
	}
	if (!root)
		return;

	status = acpi_walk_namespace(ACPI_TYPE_METHOD, root, APPLETBDRM_SOCW_DEPTH,
				     appletbdrm_socw_count_method, NULL, &method, NULL);
	if (ACPI_FAILURE(status) || method.count != 1)
		return;

	mutex_lock(&appletbdrm_socw_lock);
	if (!appletbdrm_socw_handle)
		appletbdrm_socw_handle = method.handle;
	mutex_unlock(&appletbdrm_socw_lock);
}

/* Evaluates SOCW with one integer argument, on a handle the caller read under the guard,
 * without the guard held here: it covers the state, not the blocking ACPICA
 * evaluation. Returns 0 when the T1 acknowledged, otherwise the raw result as s64 so the
 * caller logs the value as it arrived, or -EIO when the method could not be evaluated,
 * returned no value, or returned a result that cannot be logged as it arrived. */
static s64 appletbdrm_socw_evaluate(acpi_handle handle, u32 argument)
{
	union acpi_object value = { .integer = { .type = ACPI_TYPE_INTEGER, .value = argument } };
	struct acpi_object_list arguments = { 1, &value };
	struct acpi_buffer buffer = { ACPI_ALLOCATE_BUFFER, NULL };
	union acpi_object *result;
	acpi_status status;
	u64 raw;

	status = acpi_evaluate_object(handle, NULL, &arguments, &buffer);
	if (ACPI_FAILURE(status))
		return -EIO;

	result = buffer.pointer;
	if (!result)
		return -EIO;	/* the method ran and returned nothing */

	if (result->type != ACPI_TYPE_INTEGER) {
		kfree(result);
		return -EIO;
	}

	raw = result->integer.value;
	kfree(result);

	/* Compared as the u64 it is, before any narrowing. The established results are
	 * small codes; anything that does not fit is reported rather than narrowed. */
	if (raw == APPLETBDRM_SOCW_ACK)
		return 0;

	if (raw >> 63)
		return -EIO;

	return (s64)raw;
}

#else

/* Without ACPI there is no node below the host controller, so the park write is the only
 * sleep path. The harness does not exercise this branch. */
static void appletbdrm_socw_find(struct appletbdrm_device *adev)
{
	(void)adev;
}

static s64 appletbdrm_socw_evaluate(acpi_handle handle, u32 argument)
{
	(void)handle;
	(void)argument;

	return -EOPNOTSUPP;
}

#endif

/* SOCW is evaluated only from the PM_SUSPEND_PREPARE / PM_POST_SUSPEND pair, which is how
 * s2idle and suspend to RAM reach the notifier; hibernation uses other notifier events.
 * The USB suspend callback skips the park only for a plain system sleep, not runtime
 * autosuspend, and not a manual selective suspend of the device requested from userspace
 * (PM_EVENT_USER). A system sleep started from userspace arrives as PMSG_SUSPEND. */
static bool appletbdrm_socw_system_sleep(pm_message_t message)
{
	return !PMSG_IS_AUTO(message) && message.event == PM_EVENT_SUSPEND;
}

/* The system-sleep side. A failed or unacknowledged write is logged and never fails the
 * transition, because park is still what the display needs in that case. */
static int appletbdrm_socw_notify(struct notifier_block *self, unsigned long action, void *data)
{
	acpi_handle handle;
	bool was_sent, was_held;
	s64 ret;

	(void)self;
	(void)data;

	switch (action) {
	case PM_SUSPEND_PREPARE:
		mutex_lock(&appletbdrm_socw_lock);
		handle = appletbdrm_socw_handle;
		mutex_unlock(&appletbdrm_socw_lock);

		if (!handle)
			break;

		/* A PM notifier holds no module reference itself, so the module could be
		 * unloaded between prepare and post while no interface is bound, and the
		 * state the wake write needs would be lost. Hold a reference for the
		 * duration of the sleep; when it cannot be taken, this sleep is not
		 * answered, and park stays the sleep path for it. */
		if (!try_module_get(THIS_MODULE))
			break;

		ret = appletbdrm_socw_evaluate(handle, APPLETBDRM_SOCW_SLEEP);

		mutex_lock(&appletbdrm_socw_lock);
		appletbdrm_socw_sent = true;
		appletbdrm_socw_acked = ret == APPLETBDRM_SOCW_ACK;
		appletbdrm_socw_held = true;	/* the reference taken above */
		mutex_unlock(&appletbdrm_socw_lock);

		if (ret != APPLETBDRM_SOCW_ACK)
			pr_warn("appletbdrm: T1 SOCW host sleep not acknowledged (%lld)\n", ret);

		break;

	case PM_POST_SUSPEND:
		/* PM_POST_SUSPEND also follows a suspend that failed or was aborted, so
		 * this write can answer a sleep that never happened. It is still required:
		 * skipping it would leave the display powered off. */
		mutex_lock(&appletbdrm_socw_lock);
		handle = appletbdrm_socw_handle;
		was_sent = appletbdrm_socw_sent;
		was_held = appletbdrm_socw_held;
		mutex_unlock(&appletbdrm_socw_lock);

		if (!was_sent)
			break;

		ret = appletbdrm_socw_evaluate(handle, APPLETBDRM_SOCW_AWAKE);

		if (ret != APPLETBDRM_SOCW_ACK)
			pr_warn("appletbdrm: T1 SOCW host awake not acknowledged (%lld)\n", ret);

		mutex_lock(&appletbdrm_socw_lock);
		appletbdrm_socw_sent = false;
		appletbdrm_socw_acked = false;
		appletbdrm_socw_held = false;
		mutex_unlock(&appletbdrm_socw_lock);

		/* The awake write answers a sleep this module began, and the module stays
		 * loaded until it has: the reference taken in prepare is dropped here. */
		if (was_held)
			module_put(THIS_MODULE);

		break;

	/* Hibernation, restore and reboot do not use the prepare/post pair, so the
	 * notifier answers nothing for them; park stays their sleep path. */
	default:
		break;
	}

	return NOTIFY_DONE;
}

static struct notifier_block appletbdrm_socw_notifier = { .notifier_call = appletbdrm_socw_notify };

/* The park decision for appletbdrm.c's USB suspend callback, on the state the notifier
 * set for the sleep in progress. The prepare broadcast is delivered before the device
 * suspend callbacks, so the state it records is what this decision sees; the post
 * broadcast is delivered after they complete. An acknowledged sleep write powered the
 * panel off, so there is nothing to park; otherwise park is the sleep path as before. */
static bool appletbdrm_socw_skip_park(pm_message_t message)
{
	bool skipped;

	mutex_lock(&appletbdrm_socw_lock);
	skipped = appletbdrm_socw_system_sleep(message) && appletbdrm_socw_sent &&
		appletbdrm_socw_acked;
	mutex_unlock(&appletbdrm_socw_lock);

	return skipped;
}

/* Module init and exit register the notifier and the USB driver as one pair, and undo
 * them in the reverse order. The module reference held for a sleep keeps the module
 * loaded until the awake write has answered a sleep this module began. A failed driver
 * registration unwinds the notifier, and a failed notifier registration registers
 * nothing; module load can now fail on the registration itself. */
static int appletbdrm_socw_register(struct usb_driver *driver)
{
	int ret;

	ret = register_pm_notifier(&appletbdrm_socw_notifier);
	if (ret)
		return ret;

	ret = usb_register(driver);
	if (ret)
		(void)unregister_pm_notifier(&appletbdrm_socw_notifier);

	return ret;
}

static void appletbdrm_socw_unregister(struct usb_driver *driver)
{
	usb_deregister(driver);
	(void)unregister_pm_notifier(&appletbdrm_socw_notifier);
}

#endif
