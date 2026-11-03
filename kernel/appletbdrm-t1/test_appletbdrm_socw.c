/* SPDX-License-Identifier: GPL-2.0 */
/* Run the real SOCW discovery, system-sleep notifier, park decision, re-enumeration and
 * registration pair with synthetic USB, ACPI and PM boundaries. The rearm is the unchanged
 * resume path, run here only to show that the handshake lives in the notifier; what the
 * rearm itself does, and the faults at its steps, are asserted in
 * test_appletbdrm_resume.c. The harness is single-threaded, so the guard and the module
 * reference are recorded as the calls the code makes, not as contention, and the depth
 * bound and the acpi_get_object_info failure branch are not exercised: the fixture is
 * walked in full and that mock always succeeds. */
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Kernel types and macros the SOCW module uses. The harness runs the CONFIG_ACPI build;
 * the !CONFIG_ACPI branch is not exercised by the harness. */
typedef unsigned int u32;
typedef unsigned long long u64;
typedef long long s64;
typedef unsigned long acpi_size;
#define IS_ENABLED(config) (1)

typedef void *acpi_handle;
typedef int acpi_status;

#define AE_OK 0
#define ACPI_SUCCESS(status) ((status) == AE_OK)
#define ACPI_FAILURE(status) !ACPI_SUCCESS(status)

#define ACPI_TYPE_DEVICE 0x06	/* ACPICA's value, actypes.h */
#define ACPI_TYPE_INTEGER 1
#define ACPI_TYPE_METHOD 8
#define ACPI_ALLOCATE_BUFFER (acpi_size)(-1)

union acpi_object {
	u32 type;
	struct {
		u32 type;
		u64 value;
	} integer;
};

struct acpi_object_list {
	u32 count;
	union acpi_object *pointer;
};

struct acpi_buffer {
	acpi_size length;
	void *pointer;
};

struct acpi_device_info {
	u32 name;			/* the name bytes, compared as bytes */
	unsigned char param_count;
};

typedef acpi_status (*acpi_walk_callback)(acpi_handle object, u32 nesting_level,
					  void *context, void **return_value);

struct pm_message { int event; };
typedef struct pm_message pm_message_t;

/* The PM event and action values copied here must match include/linux/pm.h and
 * include/linux/suspend.h, and the ACPICA type constants copied above must match
 * include/acpi/actypes.h; ACPI_TYPE_DEVICE is 0x06, not 3, and the harness uses int for
 * acpi_status where ACPICA uses u32. Nothing in the repository checks them, so a case
 * would test the harness instead of the production values if they drifted. Notifier
 * actions are not pm_message_t events, so these share the numbers with the events above
 * without meaning the same thing. */
#define PM_EVENT_FREEZE 0x0001
#define PM_EVENT_SUSPEND 0x0002
#define PM_EVENT_HIBERNATE 0x0004
#define PM_EVENT_QUIESCE 0x0008
#define PM_EVENT_AUTO 0x0400
#define PM_EVENT_USER 0x0100
#define PM_EVENT_AUTO_SUSPEND (PM_EVENT_AUTO | PM_EVENT_SUSPEND)
#define PM_EVENT_USER_SUSPEND (PM_EVENT_USER | PM_EVENT_SUSPEND)

#define PMSG_FREEZE ((pm_message_t){ .event = PM_EVENT_FREEZE })
#define PMSG_QUIESCE ((pm_message_t){ .event = PM_EVENT_QUIESCE })
#define PMSG_SUSPEND ((pm_message_t){ .event = PM_EVENT_SUSPEND })
#define PMSG_HIBERNATE ((pm_message_t){ .event = PM_EVENT_HIBERNATE })
#define PMSG_AUTO_SUSPEND ((pm_message_t){ .event = PM_EVENT_AUTO_SUSPEND })
#define PMSG_USER_SUSPEND ((pm_message_t){ .event = PM_EVENT_USER_SUSPEND })
#define PMSG_IS_AUTO(message) (((message).event & PM_EVENT_AUTO) != 0)

#define PM_HIBERNATION_PREPARE 0x0001
#define PM_POST_HIBERNATION 0x0002
#define PM_SUSPEND_PREPARE 0x0003
#define PM_POST_SUSPEND 0x0004
#define NOTIFY_DONE 0

struct notifier_block {
	int (*notifier_call)(struct notifier_block *self, unsigned long action, void *data);
};

struct mutex { int unused; };

#define DEFINE_MUTEX(name) struct mutex name = { 0 }

/* The module reference. The harness decides what try_module_get would return, so the
 * failure case is observable, and a put is recorded only for a reference the harness
 * granted. */
#define THIS_MODULE NULL

enum step {
	PM_ACTION, NOTIFIER_REGISTER, NOTIFIER_UNREGISTER, DRIVER_REGISTER, DRIVER_DEREGISTER,
	MODULE_GET, MODULE_PUT, SOCW, CLEAR_OUT, CLEAR_IN, INFORMATION, READINESS, CLEAR,
	RESTORE,
};

static enum step observed[16];
static int given[16];		/* what the boundary was given: the SOCW argument or the
				 * notifier action, -1 for the other boundaries */
static long long logged[16];		/* the numeric value passed to pr_warn */
static size_t calls, fail_at, logged_count, evaluations;
static int failure, errors, warnings, locks, guard_held;
static int evaluate_status;	/* a non-zero status is an ACPICA failure */
static long long evaluate_value;
static int guard_at_evaluation;	/* what the guard looked like while the boundary ran */
static int get_ok = 1;		/* what try_module_get would return */
static size_t module_gets, module_puts;

static int record(enum step step, int value)
{
	assert(calls < sizeof(observed) / sizeof(observed[0]));

	observed[calls] = step;
	given[calls] = value;
	calls++;

	return calls == fail_at ? failure : 0;
}

static int perform(enum step step)
{
	return record(step, -1);
}

static void record_value(long long value)
{
	assert(logged_count < sizeof(logged) / sizeof(logged[0]));
	logged[logged_count++] = value;
}

#define pr_warn(format, value) ((void)(format), ++warnings, record_value((long long)value))

/* The resume path runs unchanged here, so its error boundary is counted as in
 * test_appletbdrm_resume.c; only pr_warn's value is recorded here. */
#define drm_err(drm, format, value) ((void)(drm), (void)(format), ++errors)

/* The synthetic namespace below the controller's ACPI node: a handle is the entry
 * itself, so the walk sees exactly what the fixture holds. */
struct acpi_entry {
	char name[5];		/* an ACPICA name is four characters, no terminator */
	unsigned int type;
	unsigned char param_count;
};

static struct acpi_entry node = { "NODE", ACPI_TYPE_DEVICE, 0 };
static struct acpi_entry above_node = { "ROOT", ACPI_TYPE_DEVICE, 0 };
static struct acpi_entry socw = { "SOCW", ACPI_TYPE_METHOD, 1 };
static struct acpi_entry second_socw = { "SOCW", ACPI_TYPE_METHOD, 1 };
static struct acpi_entry no_argument_socw = { "SOCW", ACPI_TYPE_METHOD, 0 };
static struct acpi_entry frst = { "FRST", ACPI_TYPE_METHOD, 0 };

static struct acpi_entry *fixture[8];
static size_t fixture_count;
static acpi_handle walked;

/* The chain SOCW discovery follows: the bus points at the controller, the first
 * node with an ACPI handle above it is the one walked, and the node above that
 * must not be used. A second chain has no ACPI handle anywhere. */
struct device {
	struct device *parent;
	acpi_handle handle;
};

static struct device above = { .handle = &above_node };
static struct device host = { .parent = &above, .handle = &node };
static struct device bridge = { .parent = &host };
static struct device controller = { .parent = &bridge };
static struct device no_node = { NULL, NULL };

struct usb_bus {
	struct device *controller;
};

static struct usb_bus bus = { .controller = &controller };
static struct usb_bus chain_without_node = { .controller = &no_node };

struct usb_device {
	struct usb_bus *bus;
};

struct usb_driver { int unused; };

/* The driver appletbdrm.c registers; the harness passes its own instance. */
static struct usb_driver test_driver;

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
#define ACPI_HANDLE(device) ((device)->handle)

static acpi_status acpi_get_object_info(acpi_handle object, struct acpi_device_info **info)
{
	struct acpi_entry *entry = object;

	*info = malloc(sizeof(**info));
	assert(*info);

	/* ACPICA reports the name as four bytes and the module compares them as
	 * bytes, so the fixture is copied byte by byte. */
	memcpy(&(*info)->name, entry->name, 4);
	(*info)->param_count = entry->param_count;

	return AE_OK;
}

static acpi_status acpi_walk_namespace(unsigned int type, acpi_handle root, u32 max_depth,
				      acpi_walk_callback callback, acpi_walk_callback ascending,
				      void *context, void **return_value)
{
	(void)max_depth;

	assert(type == ACPI_TYPE_METHOD);
	assert(ascending == NULL && return_value == NULL);

	walked = root;

	for (size_t i = 0; i < fixture_count; i++)
		if (fixture[i]->type == type)
			assert(callback((acpi_handle)fixture[i], 0, context, NULL) == AE_OK);

	return AE_OK;
}

static acpi_status acpi_evaluate_object(acpi_handle object, char *pathname,
				       struct acpi_object_list *arguments, struct acpi_buffer *buffer)
{
	/* Only the discovered handle is evaluated, never a path given by a caller, SOCW
	 * is called with exactly one integer, and the guard is not held during the
	 * evaluation: the guard covers the module state, not the blocking call. */
	assert(pathname == NULL);
	assert(object == (acpi_handle)&socw);
	assert(arguments->count == 1 && arguments->pointer->type == ACPI_TYPE_INTEGER);

	guard_at_evaluation = guard_held;
	assert(!guard_at_evaluation);

	evaluations++;
	record(SOCW, (int)arguments->pointer->integer.value);

	if (ACPI_FAILURE(evaluate_status))
		return evaluate_status;

	/* ACPICA allocates the returned object, and the module frees it. */
	union acpi_object *result = malloc(sizeof(union acpi_object));
	assert(result);
	result->type = ACPI_TYPE_INTEGER;
	result->integer.value = (u64)evaluate_value;
	buffer->pointer = result;

	return AE_OK;
}

static void kfree(void *memory)
{
	free(memory);
}

/* The module guard. A case counts the guarded accesses; the harness is
 * single-threaded, so an access made outside the guard is not observable here. */
static void mutex_lock(struct mutex *lock)
{
	assert(!guard_held);
	guard_held = 1;
	(void)lock;
	locks++;
}

static void mutex_unlock(struct mutex *lock)
{
	assert(guard_held);
	guard_held = 0;
	(void)lock;
}

static int try_module_get(void *module)
{
	(void)module;

	record(MODULE_GET, -1);

	if (!get_ok)
		return 0;

	module_gets++;

	return 1;
}

static void module_put(void *module)
{
	(void)module;

	assert(module_puts < module_gets);	/* only a reference the harness granted is dropped */
	record(MODULE_PUT, -1);
	module_puts++;
}

static struct notifier_block *registered;
static struct usb_driver *bound;

static int register_pm_notifier(struct notifier_block *self)
{
	int ret = record(NOTIFIER_REGISTER, -1);

	if (ret)
		return ret;	/* the block is not on the chain, so there is nothing to undo */

	registered = self;

	return 0;
}

static int unregister_pm_notifier(struct notifier_block *self)
{
	assert(registered == self);	/* only what was registered is unregistered */
	registered = NULL;

	return record(NOTIFIER_UNREGISTER, -1);
}

static int usb_register(struct usb_driver *driver)
{
	int ret = record(DRIVER_REGISTER, -1);

	if (ret)
		return ret;

	bound = driver;

	return 0;
}

static void usb_deregister(struct usb_driver *driver)
{
	assert(bound == driver);
	bound = NULL;

	perform(DRIVER_DEREGISTER);
}

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

#include "appletbdrm_socw.h"
#include "appletbdrm_resume.h"

/* Start a case from an empty recording and an empty module state. */
static void reset(void)
{
	calls = fail_at = logged_count = evaluations = locks = 0;
	errors = warnings = failure = 0;
	guard_at_evaluation = 0;
	registered = NULL;
	bound = NULL;
	guard_held = 0;
	get_ok = 1;
	module_gets = module_puts = 0;

	appletbdrm_socw_handle = NULL;
	appletbdrm_socw_sent = false;
	appletbdrm_socw_acked = false;
	appletbdrm_socw_held = false;
}

/* Keep the module state and start recording again, so a case counts only its own
 * guarded accesses. */
static void restart(void)
{
	calls = fail_at = logged_count = evaluations = locks = 0;
	warnings = failure = 0;
}

/* Model one probe: wire a device to one of the synthetic buses, choose the namespace
 * the walk sees below the ACPI node, and start from the module state as it is.
 * Production discovers only for a device the driver has identified as a T1. */
static void prepare(struct appletbdrm_device *adev, struct usb_bus *chain, size_t entries)
{
	adev->usb.bus = chain;
	fixture_count = entries;
	walked = NULL;
}

/* A probe that keeps the unique single-argument SOCW method. */
static void discover(struct appletbdrm_device *adev)
{
	prepare(adev, &bus, 2);
	fixture[0] = &node;
	fixture[1] = &socw;
	appletbdrm_socw_find(adev);
}

/* Run one PM event through the registered block, as PM does. The action is recorded
 * before the callback runs, so the recorded order shows what it answered. */
static int notify(unsigned long action)
{
	record(PM_ACTION, (int)action);

	return appletbdrm_socw_notifier.notifier_call(&appletbdrm_socw_notifier, action, NULL);
}

static void test_discovery(void)
{
	struct appletbdrm_device adev = { .t1 = true, .out_ep = 2, .in_ep = 5 };

	/* A unique single-argument SOCW method below the first ACPI node in the
	 * controller's parent chain is kept in the module state, under the guard, and
	 * nothing is evaluated at probe. */
	reset();
	discover(&adev);
	assert(walked == (acpi_handle)&node);
	assert(appletbdrm_socw_handle == (acpi_handle)&socw);
	assert(evaluations == 0 && locks == 1);

	/* A second SOCW method is not a unique handshake. */
	reset();
	prepare(&adev, &bus, 3);
	fixture[0] = &node;
	fixture[1] = &socw;
	fixture[2] = &second_socw;
	appletbdrm_socw_find(&adev);
	assert(appletbdrm_socw_handle == NULL && locks == 0);

	/* Neither a SOCW method with another parameter count, nor another name. */
	reset();
	prepare(&adev, &bus, 2);
	fixture[0] = &node;
	fixture[1] = &no_argument_socw;
	appletbdrm_socw_find(&adev);
	assert(appletbdrm_socw_handle == NULL);

	fixture[1] = &frst;
	appletbdrm_socw_find(&adev);
	assert(appletbdrm_socw_handle == NULL);

	/* A controller with no ACPI node anywhere above it keeps nothing and is not
	 * walked. */
	reset();
	prepare(&adev, &chain_without_node, 1);
	fixture[0] = &node;
	appletbdrm_socw_find(&adev);
	assert(walked == NULL && appletbdrm_socw_handle == NULL);
}

static void test_system_sleep(void)
{
	struct appletbdrm_device adev = { .t1 = true, .out_ep = 2, .in_ep = 5 };

	const int status[] = { AE_OK, AE_OK, AE_OK, 1, AE_OK };
	/* 1LL << 63 is built as 1ULL << 63 and narrowed, so the case is not undefined
	 * behaviour in the harness itself. */
	const long long value[] = { APPLETBDRM_SOCW_ACK, 1, 2, 0, (long long)(1ULL << 63) };
	const long long reported[] = { 0, 1, 2, -EIO, -EIO };

	/* Acknowledged: SOCW(0) once, and the sleep is remembered as answered. The
	 * not-acknowledged values 1 and 2, and an evaluation failure, log once with the
	 * value and leave the sleep unanswered, which is what park then covers. A result
	 * too large to narrow, 1LL << 63, reports -EIO: the raw value cannot be logged as
	 * it arrived. The reference is taken before the evaluation, so the sleep is
	 * answered through the post, and the evaluation runs with the guard not held: the
	 * guard covers the state, not the blocking call. Every case still answers the sleep
	 * with the awake write. */
	for (size_t item = 0; item < sizeof(status) / sizeof(status[0]); item++) {
		reset();
		evaluate_status = status[item];
		evaluate_value = value[item];
		discover(&adev);
		restart();

		assert(notify(PM_SUSPEND_PREPARE) == NOTIFY_DONE);
		assert(calls == 3 && observed[0] == PM_ACTION && given[0] == PM_SUSPEND_PREPARE &&
		       observed[1] == MODULE_GET && observed[2] == SOCW &&
		       given[2] == APPLETBDRM_SOCW_SLEEP);
		assert(guard_at_evaluation == 0);	/* the guard is not held during it */
		assert(appletbdrm_socw_sent && appletbdrm_socw_acked == (reported[item] == 0));
		assert(appletbdrm_socw_held);
		assert(warnings == (reported[item] != 0));
		if (reported[item])
			assert(logged[0] == reported[item]);
		assert(locks == 2);		/* the state is read, then recorded */
		assert(module_gets == 1 && module_puts == 0);	/* held for the duration of the sleep */

		restart();
		assert(notify(PM_POST_SUSPEND) == NOTIFY_DONE);
		assert(calls == 3 && observed[0] == PM_ACTION && given[0] == PM_POST_SUSPEND &&
		       observed[1] == SOCW && given[1] == APPLETBDRM_SOCW_AWAKE &&
		       observed[2] == MODULE_PUT);
		assert(guard_at_evaluation == 0);
		/* The put is recorded after the awake write, so the module stays loaded
		 * until it has answered the sleep the get began. */
		assert(errors == 0 && warnings == (reported[item] != 0));
		assert(locks == 2 && module_gets == 1 && module_puts == 1);
		assert(!appletbdrm_socw_sent && !appletbdrm_socw_held);
	}

	/* Without a discovered handle there is nothing to answer, and no reference is
	 * taken for the sleep. */
	reset();
	prepare(&adev, &chain_without_node, 1);
	fixture[0] = &node;
	appletbdrm_socw_find(&adev);
	restart();
	assert(notify(PM_SUSPEND_PREPARE) == NOTIFY_DONE);
	assert(calls == 1 && observed[0] == PM_ACTION && given[0] == PM_SUSPEND_PREPARE &&
	       evaluations == 0 && !appletbdrm_socw_sent && !appletbdrm_socw_held &&
	       module_gets == 0 && locks == 1);

	/* A sleep whose module reference cannot be taken is not answered: no SOCW(0), so
	 * nothing is owed at the post and nothing is put, and park stays the sleep path
	 * for it. */
	reset();
	discover(&adev);
	get_ok = 0;
	restart();
	assert(notify(PM_SUSPEND_PREPARE) == NOTIFY_DONE);
	assert(calls == 2 && observed[0] == PM_ACTION && observed[1] == MODULE_GET &&
	       evaluations == 0 && !appletbdrm_socw_sent && !appletbdrm_socw_held &&
	       module_gets == 0 && locks == 1);

	restart();
	assert(notify(PM_POST_SUSPEND) == NOTIFY_DONE);
	assert(calls == 1 && evaluations == 0 && module_gets == 0 && module_puts == 0);

	restart();
	assert(!appletbdrm_socw_skip_park(PMSG_SUSPEND));

	/* Hibernation does not reach the prepare/post pair, so the notifier answers
	 * nothing for it, and the reference taken for an earlier system sleep is still
	 * held. */
	const unsigned long actions[] = { PM_HIBERNATION_PREPARE, PM_POST_HIBERNATION };

	for (size_t action = 0; action < sizeof(actions) / sizeof(actions[0]); action++) {
		reset();
		evaluate_status = AE_OK;
		evaluate_value = APPLETBDRM_SOCW_ACK;
		discover(&adev);

		/* A system sleep would have answered this one. */
		assert(notify(PM_SUSPEND_PREPARE) == NOTIFY_DONE);
		assert(calls == 3 && observed[1] == MODULE_GET && observed[2] == SOCW &&
		       appletbdrm_socw_sent && module_gets == 1 && module_puts == 0);

		restart();
		assert(notify(actions[action]) == NOTIFY_DONE);
		assert(calls == 1 && observed[0] == PM_ACTION && given[0] == (int)actions[action]);
		assert(evaluations == 0 && appletbdrm_socw_sent && module_gets == 1 && module_puts == 0);
	}

	/* Post-suspend with nothing sent answers nothing, and puts nothing. */
	reset();
	discover(&adev);
	restart();
	assert(notify(PM_POST_SUSPEND) == NOTIFY_DONE);
	assert(calls == 1 && observed[0] == PM_ACTION && evaluations == 0 && module_puts == 0);
}

static void test_wake_answers_the_sleep(void)
{
	struct appletbdrm_device adev = { .t1 = true, .out_ep = 2, .in_ep = 5 };

	const int status[] = { AE_OK, AE_OK, 1 };
	const int value[] = { APPLETBDRM_SOCW_ACK, 2, 0 };
	const int reported[] = { 0, 2, -EIO };

	for (size_t item = 0; item < sizeof(status) / sizeof(status[0]); item++) {
		reset();
		evaluate_status = AE_OK;
		evaluate_value = APPLETBDRM_SOCW_ACK;
		discover(&adev);
		assert(notify(PM_SUSPEND_PREPARE) == NOTIFY_DONE);
		assert(appletbdrm_socw_sent && appletbdrm_socw_acked && appletbdrm_socw_held);

		/* The wake write answers the sleep the notifier began, once, and the state
		 * is cleared so the next sleep starts clean. A failed write is only logged,
		 * never an error. PM_POST_SUSPEND also follows an aborted suspend, so the
		 * state alone decides here: no interface of the sleeping device is involved,
		 * which is what a device that is no longer there still needs. */
		restart();
		evaluate_status = status[item];
		evaluate_value = value[item];
		assert(notify(PM_POST_SUSPEND) == NOTIFY_DONE);
		assert(calls == 3 && observed[0] == PM_ACTION && given[0] == PM_POST_SUSPEND &&
		       observed[1] == SOCW && given[1] == APPLETBDRM_SOCW_AWAKE &&
		       observed[2] == MODULE_PUT);
		assert(guard_at_evaluation == 0);
		assert(warnings == (reported[item] != 0));
		if (reported[item])
			assert(logged[0] == reported[item]);
		assert(!appletbdrm_socw_sent && !appletbdrm_socw_acked && !appletbdrm_socw_held);
		assert(module_gets == 1 && module_puts == 1);
	}

	/* A wake with no sleep behind it sends nothing, so the awake host case cannot
	 * leave the display dark on its own. */
	reset();
	discover(&adev);
	restart();
	assert(notify(PM_POST_SUSPEND) == NOTIFY_DONE);
	assert(calls == 1 && evaluations == 0 && !appletbdrm_socw_sent && module_puts == 0);
}

/* The case the module state exists for: the interface that slept is gone, and the
 * device that enumerates in its place still gets the awake write for that sleep. */
static void test_reenumerated_device(void)
{
	struct appletbdrm_device old = { .t1 = true, .out_ep = 2, .in_ep = 5 };
	struct appletbdrm_device new = { .t1 = true, .out_ep = 2, .in_ep = 5 };

	reset();

	/* The first successful discovery is the handle the module keeps. */
	discover(&old);
	assert(appletbdrm_socw_handle == (acpi_handle)&socw);

	/* A probe that keeps nothing does not replace it: the new device starts with the
	 * handle the old one found, and nothing is evaluated at its probe either. */
	restart();
	prepare(&new, &bus, 3);
	fixture[0] = &node;
	fixture[1] = &socw;
	fixture[2] = &second_socw;
	appletbdrm_socw_find(&new);
	assert(appletbdrm_socw_handle == (acpi_handle)&socw && evaluations == 0);

	/* The old device is gone; the sleep is remembered for the module, not for it. */
	evaluate_status = AE_OK;
	evaluate_value = APPLETBDRM_SOCW_ACK;
	restart();
	assert(notify(PM_SUSPEND_PREPARE) == NOTIFY_DONE);

	restart();
	assert(notify(PM_POST_SUSPEND) == NOTIFY_DONE);
	assert(calls == 3 && observed[0] == PM_ACTION && given[0] == PM_POST_SUSPEND &&
	       observed[1] == SOCW && given[1] == APPLETBDRM_SOCW_AWAKE &&
	       observed[2] == MODULE_PUT);
	assert(warnings == 0);
	assert(!appletbdrm_socw_sent && !appletbdrm_socw_acked && !appletbdrm_socw_held);

	/* The new device is not the one that slept, so its callbacks change nothing. */
	prepare(&new, &chain_without_node, 1);
	fixture[0] = &node;
	appletbdrm_socw_find(&new);
	assert(appletbdrm_socw_handle == (acpi_handle)&socw);
}

/* The decision appletbdrm.c's suspend callback asks for. What park writes, and the guard
 * that only a device the driver has identified as a T1 is parked, are appletbdrm.c's and
 * cannot be run through the header the harness includes. Production never discovers for a
 * non-T1 device, so the state such a device decides on is the empty state. */
static void test_park_decision(void)
{
	struct appletbdrm_device adev = { .t1 = true, .out_ep = 2, .in_ep = 5 };

	/* An acknowledged system sleep write powered the panel off, so the callback does
	 * not park, and the decision does not evaluate SOCW itself. */
	reset();
	evaluate_status = AE_OK;
	evaluate_value = APPLETBDRM_SOCW_ACK;
	discover(&adev);
	assert(notify(PM_SUSPEND_PREPARE) == NOTIFY_DONE);

	restart();
	assert(appletbdrm_socw_skip_park(PMSG_SUSPEND));
	assert(evaluations == 0 && locks == 1);

	/* Every other case leaves the park to appletbdrm.c: a system sleep the T1 did not
	 * acknowledge, with an evaluation failure. */
	const int status[] = { AE_OK, AE_OK, 1 };
	const int value[] = { 1, 2, 0 };

	for (size_t item = 0; item < sizeof(status) / sizeof(status[0]); item++) {
		reset();
		evaluate_status = status[item];
		evaluate_value = value[item];
		discover(&adev);
		assert(notify(PM_SUSPEND_PREPARE) == NOTIFY_DONE);

		restart();
		assert(!appletbdrm_socw_skip_park(PMSG_SUSPEND));
	}

	/* An event that is not a plain system sleep never skips the park, whatever the
	 * state says: runtime autosuspend, a manual selective suspend of the device
	 * requested from userspace (PM_EVENT_USER), hibernation, freeze and quiesce. The
	 * T1 would then wait for the awake write of a sleep the notifier never answered.
	 * A system sleep started from userspace arrives as PMSG_SUSPEND. */
	const pm_message_t events[] = { PMSG_AUTO_SUSPEND, PMSG_USER_SUSPEND,
					PMSG_HIBERNATE, PMSG_FREEZE, PMSG_QUIESCE };

	for (size_t event = 0; event < sizeof(events) / sizeof(events[0]); event++) {
		reset();
		evaluate_status = AE_OK;
		evaluate_value = APPLETBDRM_SOCW_ACK;
		discover(&adev);
		assert(notify(PM_SUSPEND_PREPARE) == NOTIFY_DONE);

		restart();
		assert(!appletbdrm_socw_skip_park(events[event]));
		assert(evaluations == 0);
	}

	/* A device whose SOCW method was never discovered decides on an empty state, so
	 * park is the sleep path as before. */
	reset();
	assert(!appletbdrm_socw_skip_park(PMSG_SUSPEND));
	assert(evaluations == 0 && locks == 1);
}

/* The USB callbacks never evaluate SOCW, so a resume answers nothing: the notifier is
 * the only path that does, and the rearm itself is unchanged. The order of the rearm steps
 * and the faults at them are asserted in test_appletbdrm_resume.c, which owns that
 * sequence; only the handshake's own state is asserted here. */
static void test_resume_never_sends_socw(void)
{
	struct appletbdrm_device adev = { .t1 = true, .out_ep = 2, .in_ep = 5 };
	struct usb_interface intf = { .data = &adev };

	reset();
	evaluate_status = AE_OK;
	evaluate_value = APPLETBDRM_SOCW_ACK;
	discover(&adev);
	assert(notify(PM_SUSPEND_PREPARE) == NOTIFY_DONE);

	restart();
	assert(appletbdrm_resume(&intf) == 0);
	assert(evaluations == 0 && errors == 0 && warnings == 0);
	assert(appletbdrm_socw_sent && appletbdrm_socw_acked && appletbdrm_socw_held);

	/* The same holds for reset_resume, which is the same callback, and for the sleep
	 * in progress the reference is still held. */
	restart();
	assert(appletbdrm_resume(&intf) == 0);
	assert(evaluations == 0 && module_gets == 1 && module_puts == 0);
}

static void test_module_registration(void)
{
	/* Module init registers the notifier and the driver as the pair they are undone
	 * in the reverse order. */
	reset();
	assert(appletbdrm_socw_register(&test_driver) == 0);
	assert(registered == &appletbdrm_socw_notifier &&
	       registered->notifier_call == appletbdrm_socw_notify);
	assert(bound == &test_driver);
	assert(calls == 2 && observed[0] == NOTIFIER_REGISTER && observed[1] == DRIVER_REGISTER);

	restart();
	appletbdrm_socw_unregister(&test_driver);
	assert(calls == 2 && observed[0] == DRIVER_DEREGISTER && observed[1] == NOTIFIER_UNREGISTER);
	assert(registered == NULL && bound == NULL);

	/* A failed notifier registration registers nothing, so there is nothing to undo. */
	reset();
	fail_at = 1;
	failure = -ENOMEM;
	assert(appletbdrm_socw_register(&test_driver) == -ENOMEM);
	assert(calls == 1 && registered == NULL && bound == NULL);

	/* A failed driver registration unwinds the notifier. */
	reset();
	fail_at = 2;
	failure = -ENOMEM;
	assert(appletbdrm_socw_register(&test_driver) == -ENOMEM);
	assert(calls == 3 && observed[0] == NOTIFIER_REGISTER && observed[1] == DRIVER_REGISTER &&
	       observed[2] == NOTIFIER_UNREGISTER);
	assert(registered == NULL && bound == NULL);
}

int main(void)
{
	test_discovery();
	test_system_sleep();
	test_wake_answers_the_sleep();
	test_reenumerated_device();
	test_park_decision();
	test_resume_never_sends_socw();
	test_module_registration();
	puts("appletbdrm SOCW tests passed");
	return 0;
}
