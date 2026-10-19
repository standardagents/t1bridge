#define _GNU_SOURCE

#define MOUNTPOINT_TEMPLATE "/tmp/t1bridge-efi.XXXXXX"

#include "t1_efi_roots.h"
#include "t1_efi_roots_test.h"

#include <errno.h>
#include <fcntl.h>
#include <libudev.h>
#include <limits.h>
#include <sched.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <strings.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>

#define T1_ESP_PARTITION_TYPE "c12a7328-f81f-11d2-ba4b-00a0c93ec93b"

enum open_root_result {
	OPEN_ROOT_OK = 0,
	OPEN_ROOT_SKIP = 1,
};

static unsigned long readonly_mount_flags(void)
{
	return MS_RDONLY | MS_NOSUID | MS_NODEV | MS_NOEXEC |
		MS_NOSYMFOLLOW;
}

static int filesystem_is_fat_or_unknown(const char *filesystem_type)
{
	static const char *const accepted[] = {
		"fat", "fat12", "fat16", "fat32", "msdos", "vfat",
	};
	size_t index;

	if (filesystem_type == NULL || filesystem_type[0] == '\0')
		return 1;
	for (index = 0; index < sizeof(accepted) / sizeof(accepted[0]);
	     ++index) {
		if (strcasecmp(filesystem_type, accepted[index]) == 0)
			return 1;
	}
	return 0;
}

static int candidate_is_eligible(const char *device_type,
	const char *partition_type, const char *filesystem_type)
{
	return device_type != NULL &&
		strcmp(device_type, "partition") == 0 &&
		partition_type != NULL &&
		strcasecmp(partition_type, T1_ESP_PARTITION_TYPE) == 0 &&
		filesystem_is_fat_or_unknown(filesystem_type);
}

static int compare_candidates(const void *left_value, const void *right_value)
{
	const struct t1_efi_test_candidate *left = left_value;
	const struct t1_efi_test_candidate *right = right_value;

	if (left->major_number < right->major_number)
		return -1;
	if (left->major_number > right->major_number)
		return 1;
	if (left->minor_number < right->minor_number)
		return -1;
	if (left->minor_number > right->minor_number)
		return 1;
	return 0;
}

static int same_candidate(const struct t1_efi_test_candidate *left,
	const struct t1_efi_test_candidate *right)
{
	return left->major_number == right->major_number &&
		left->minor_number == right->minor_number;
}

static int close_owned(int descriptor)
{
	int result = close(descriptor);

	/* Linux releases the descriptor even when close reports EINTR. */
	return result < 0 && errno == EINTR ? 0 : result;
}

static int real_close_descriptor(void *context, int descriptor)
{
	(void)context;
	return close_owned(descriptor);
}

static int device_number_is_valid(uint32_t major_number,
	uint32_t minor_number)
{
	(void)minor_number;
	return major_number != 0;
}

static int mount_error_status(int error_number)
{
	return error_number == ENOENT ?
		OPEN_ROOT_SKIP : T1_EFI_ROOTS_INSPECTION_FAILED;
}

static int real_enumerate(void *context,
	struct t1_efi_test_candidate *candidates, size_t capacity,
	size_t *count)
{
	struct udev *udev = NULL;
	struct udev_enumerate *enumeration = NULL;
	struct udev_list_entry *devices;
	struct udev_list_entry *entry;
	int status = T1_EFI_ROOTS_ENUMERATION_FAILED;

	(void)context;
	*count = 0;
	udev = udev_new();
	if (udev == NULL)
		goto out;
	enumeration = udev_enumerate_new(udev);
	if (enumeration == NULL ||
	    udev_enumerate_add_match_subsystem(enumeration, "block") < 0 ||
	    udev_enumerate_scan_devices(enumeration) < 0)
		goto out;

	devices = udev_enumerate_get_list_entry(enumeration);
	udev_list_entry_foreach(entry, devices) {
		const char *path = udev_list_entry_get_name(entry);
		struct udev_device *device;
		const char *device_type;
		const char *partition_type;
		const char *filesystem_type;
		dev_t device_number;

		if (path == NULL)
			goto out;
		device = udev_device_new_from_syspath(udev, path);
		if (device == NULL)
			goto out;
		device_type = udev_device_get_devtype(device);
		partition_type = udev_device_get_property_value(device,
			"ID_PART_ENTRY_TYPE");
		filesystem_type = udev_device_get_property_value(device,
			"ID_FS_TYPE");
		if (!candidate_is_eligible(device_type, partition_type,
		    filesystem_type)) {
			udev_device_unref(device);
			continue;
		}
		if (*count == capacity) {
			udev_device_unref(device);
			status = T1_EFI_ROOTS_CANDIDATE_LIMIT;
			goto out;
		}
		device_number = udev_device_get_devnum(device);
		candidates[*count].major_number = major(device_number);
		candidates[*count].minor_number = minor(device_number);
		if (!device_number_is_valid(candidates[*count].major_number,
		    candidates[*count].minor_number)) {
			udev_device_unref(device);
			goto out;
		}
		++*count;
		udev_device_unref(device);
	}
	status = T1_EFI_ROOTS_OK;

out:
	if (enumeration != NULL)
		udev_enumerate_unref(enumeration);
	if (udev != NULL)
		udev_unref(udev);
	return status;
}

static int real_make_private_namespace(void *context)
{
	(void)context;
	if (unshare(CLONE_NEWNS) < 0)
		return T1_EFI_ROOTS_NAMESPACE_FAILED;
	if (mount(NULL, "/", NULL, MS_REC | MS_PRIVATE, NULL) < 0)
		return T1_EFI_ROOTS_NAMESPACE_FAILED;
	return T1_EFI_ROOTS_OK;
}

static int open_directory_component(int parent, const char *name)
{
	int descriptor;

	do {
		descriptor = openat(parent, name, O_RDONLY | O_DIRECTORY |
			O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
	} while (descriptor < 0 && errno == EINTR);
	return descriptor;
}

static int root_contains_apple(int root)
{
	int efi;
	int apple;
	int result;

	efi = open_directory_component(root, "EFI");
	if (efi < 0)
		return errno == ENOENT || errno == ENOTDIR ? 0 : -1;
	apple = open_directory_component(efi, "APPLE");
	if (apple < 0)
		result = errno == ENOENT || errno == ENOTDIR ? 0 : -1;
	else
		result = 1;
	if (apple >= 0 && close_owned(apple) < 0)
		result = -1;
	if (close_owned(efi) < 0)
		result = -1;
	return result;
}

static int remove_unmounted_directory(char *mountpoint)
{
	if (rmdir(mountpoint) < 0)
		return T1_EFI_ROOTS_CLEANUP_FAILED;
	return T1_EFI_ROOTS_OK;
}

static int decode_mount_path(char *path)
{
	char *output = path;
	const char *input = path;

	while (*input != '\0') {
		if (*input != '\\') {
			*output++ = *input++;
			continue;
		}
		if (strncmp(input, "\\040", 4) == 0)
			*output++ = ' ';
		else if (strncmp(input, "\\011", 4) == 0)
			*output++ = '\t';
		else if (strncmp(input, "\\012", 4) == 0)
			*output++ = '\n';
		else if (strncmp(input, "\\134", 4) == 0)
			*output++ = '\\';
		else
			return -1;
		input += 4;
	}
	*output = '\0';
	return path[0] == '/' ? 0 : -1;
}

/* Only whole-filesystem mounts qualify; a subdirectory bind hides siblings. */
static int parse_existing_mount(const char *line,
	const struct t1_efi_test_candidate *candidate,
	unsigned long long *mount_id, char path[PATH_MAX])
{
	unsigned int device_major, device_minor;
	char root[PATH_MAX];
	const char *separator = strstr(line, " - ");

	if (separator == NULL || strncmp(separator + 3, "vfat ", 5) != 0)
		return 0;
	if (sscanf(line, "%llu %*u %u:%u %4095s %4095s", mount_id,
	    &device_major, &device_minor, root, path) != 5)
		return -1;
	if (device_major != candidate->major_number ||
	    device_minor != candidate->minor_number || strcmp(root, "/") != 0)
		return 0;
	return decode_mount_path(path) == 0 ? 1 : -1;
}

static int open_mount_path(const char *path)
{
	const int flags = O_PATH | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW;
	char copy[PATH_MAX];
	char *remaining;
	char *component;
	int parent;

	if (path == NULL || path[0] != '/' || strlen(path) >= sizeof(copy))
		return -1;
	memcpy(copy, path, strlen(path) + 1);
	remaining = copy;
	parent = open("/", flags);
	if (parent < 0)
		return -1;
	/* RestrictSUIDSGID rejects openat2; pin each no-follow directory instead. */
	while ((component = strsep(&remaining, "/")) != NULL) {
		int next;

		if (component[0] == '\0')
			continue;
		if (strcmp(component, ".") == 0 || strcmp(component, "..") == 0) {
			(void)close_owned(parent);
			return -1;
		}
		next = openat(parent, component, flags);
		if (close_owned(parent) < 0) {
			if (next >= 0)
				(void)close_owned(next);
			return -1;
		}
		if (next < 0)
			return -1;
		parent = next;
	}
	return parent;
}

static int bind_existing_mount(const struct t1_efi_test_candidate *candidate,
	const char *mountpoint, unsigned long flags, int *mounted)
{
	/* Discovery may run on a worker after unshare(CLONE_NEWNS). The
	 * inventory must describe that thread, not the main thread namespace. */
	FILE *inventory = fopen("/proc/thread-self/mountinfo", "re");
	char line[16384];
	char path[PATH_MAX];
	int status = T1_EFI_ROOTS_INSPECTION_FAILED;
	size_t count = 0;

	if (inventory == NULL)
		return status;
	while (fgets(line, sizeof(line), inventory) != NULL) {
		unsigned long long mount_id;
		struct statx info;
		char source[64];
		int root;
		int match;

		if (++count > 65536 || strchr(line, '\n') == NULL)
			break;
		match = parse_existing_mount(line, candidate, &mount_id, path);
		if (match < 0)
			break;
		if (match == 0)
			continue;
		root = open_mount_path(path);
		if (root < 0)
			break;
		/* Pin the exact mount from the inventory, not a replacement at its path. */
		if (statx(root, "", AT_EMPTY_PATH, STATX_TYPE | STATX_MNT_ID, &info) < 0 ||
		    (info.stx_mask & (STATX_TYPE | STATX_MNT_ID)) !=
		    (STATX_TYPE | STATX_MNT_ID) || !S_ISDIR(info.stx_mode) ||
		    info.stx_mnt_id != mount_id ||
		    info.stx_dev_major != candidate->major_number ||
		    info.stx_dev_minor != candidate->minor_number) {
			(void)close_owned(root);
			break;
		}
		(void)snprintf(source, sizeof(source), "/proc/self/fd/%d", root);
		if (mount(source, mountpoint, NULL, MS_BIND, NULL) == 0) {
			*mounted = 1;
			/* MS_BIND makes this per-mount: never remount the shared superblock. */
			if (mount(NULL, mountpoint, NULL, MS_REMOUNT | MS_BIND | flags,
			    NULL) == 0)
				status = T1_EFI_ROOTS_OK;
		}
		if (close_owned(root) < 0)
			status = T1_EFI_ROOTS_CLEANUP_FAILED;
		break;
	}
	if (fclose(inventory) != 0)
		status = T1_EFI_ROOTS_INSPECTION_FAILED;
	return status;
}

static int real_open_root(void *context,
	const struct t1_efi_test_candidate *candidate,
	unsigned long mount_flags, int *root_descriptor)
{
	char source[64];
	/* PrivateTmp remains writable under ProtectSystem=strict. */
	char mountpoint[] = MOUNTPOINT_TEMPLATE;
	struct stat info;
	int root = -1;
	int contains_apple;
	int mounted = 0;
	int status = T1_EFI_ROOTS_INSPECTION_FAILED;

	(void)context;
	*root_descriptor = -1;
	if (!device_number_is_valid(candidate->major_number,
	    candidate->minor_number) ||
	    snprintf(source, sizeof(source), "/dev/block/%u:%u",
	    candidate->major_number, candidate->minor_number) < 0)
		return T1_EFI_ROOTS_INSPECTION_FAILED;
	if (stat(source, &info) < 0)
		return mount_error_status(errno);
	if (!S_ISBLK(info.st_mode) ||
	    major(info.st_rdev) != candidate->major_number ||
	    minor(info.st_rdev) != candidate->minor_number)
		return T1_EFI_ROOTS_INSPECTION_FAILED;
	if (mkdtemp(mountpoint) == NULL)
		return T1_EFI_ROOTS_INSPECTION_FAILED;
	if (mount(source, mountpoint, "vfat", mount_flags, NULL) < 0) {
		int mount_error = errno;

		status = mount_error == EBUSY ?
			bind_existing_mount(candidate, mountpoint, mount_flags, &mounted) :
			mount_error_status(mount_error);
		if (status != T1_EFI_ROOTS_OK)
			goto out;
	}
	mounted = 1;
	root = open_directory_component(AT_FDCWD, mountpoint);
	if (root < 0)
		goto out;
	contains_apple = root_contains_apple(root);
	if (contains_apple < 0)
		goto out;
	status = contains_apple == 0 ? OPEN_ROOT_SKIP : OPEN_ROOT_OK;

out:
	/* The open root descriptor pins the mount after namespace detachment. */
	if (mounted && umount2(mountpoint, MNT_DETACH) < 0)
		status = T1_EFI_ROOTS_CLEANUP_FAILED;
	if (remove_unmounted_directory(mountpoint) != T1_EFI_ROOTS_OK)
		status = T1_EFI_ROOTS_CLEANUP_FAILED;
	if (status == OPEN_ROOT_OK) {
		*root_descriptor = root;
	} else if (root >= 0 && close_owned(root) < 0) {
		status = T1_EFI_ROOTS_CLEANUP_FAILED;
	}
	return status;
}

static void initialize_outputs(int *root_descriptors, size_t capacity,
	size_t *root_count)
{
	size_t index;

	*root_count = 0;
	for (index = 0; index < capacity; ++index)
		root_descriptors[index] = -1;
}

static int close_outputs(int *root_descriptors, size_t count,
	const struct t1_efi_test_ops *ops)
{
	int failed = 0;

	while (count > 0) {
		--count;
		if (root_descriptors[count] >= 0 &&
		    ops->close_descriptor(ops->context,
		    root_descriptors[count]) < 0)
			failed = 1;
		root_descriptors[count] = -1;
	}
	return failed;
}

int t1_efi_roots_discover_with_ops(int *root_descriptors,
	size_t descriptor_capacity, size_t *root_count,
	const struct t1_efi_test_ops *ops)
{
	struct t1_efi_test_candidate candidates[T1_EFI_ROOT_LIMIT];
	size_t candidate_count = 0;
	size_t output_count = 0;
	size_t index;
	int status;

	if (root_descriptors == NULL || root_count == NULL || ops == NULL ||
	    ops->enumerate == NULL || ops->make_private_namespace == NULL ||
	    ops->open_root == NULL || ops->close_descriptor == NULL ||
	    descriptor_capacity == 0 ||
	    descriptor_capacity > T1_EFI_ROOT_LIMIT)
		return T1_EFI_ROOTS_INVALID_ARGUMENT;
	initialize_outputs(root_descriptors, descriptor_capacity, root_count);
	status = ops->enumerate(ops->context, candidates,
		descriptor_capacity, &candidate_count);
	if (status != T1_EFI_ROOTS_OK)
		return status;
	if (candidate_count > descriptor_capacity)
		return T1_EFI_ROOTS_CANDIDATE_LIMIT;
	if (candidate_count == 0)
		return T1_EFI_ROOTS_OK;
	qsort(candidates, candidate_count, sizeof(candidates[0]),
		compare_candidates);
	status = ops->make_private_namespace(ops->context);
	if (status != T1_EFI_ROOTS_OK)
		return status;

	for (index = 0; index < candidate_count; ++index) {
		int root = -1;

		if (index > 0 && same_candidate(&candidates[index - 1],
		    &candidates[index]))
			continue;
		status = ops->open_root(ops->context, &candidates[index],
			readonly_mount_flags(), &root);
		if (status == OPEN_ROOT_SKIP)
			continue;
		if (status != OPEN_ROOT_OK || root < 0) {
			if (root >= 0)
				(void)ops->close_descriptor(ops->context, root);
			if (close_outputs(root_descriptors, output_count, ops) != 0)
				status = T1_EFI_ROOTS_CLEANUP_FAILED;
			return status == OPEN_ROOT_OK ?
				T1_EFI_ROOTS_INSPECTION_FAILED : status;
		}
		root_descriptors[output_count++] = root;
	}
	*root_count = output_count;
	return T1_EFI_ROOTS_OK;
}

int t1_efi_roots_discover(int *root_descriptors, size_t descriptor_capacity,
	size_t *root_count)
{
	const struct t1_efi_test_ops ops = {
		.enumerate = real_enumerate,
		.make_private_namespace = real_make_private_namespace,
		.open_root = real_open_root,
		.close_descriptor = real_close_descriptor,
		.context = NULL,
	};

	return t1_efi_roots_discover_with_ops(root_descriptors,
		descriptor_capacity, root_count, &ops);
}

int t1_efi_roots_is_root(void)
{
	return geteuid() == 0 ? 1 : 0;
}

int t1_efi_roots_test_candidate_is_eligible(const char *device_type,
	const char *partition_type, const char *filesystem_type)
{
	return candidate_is_eligible(device_type, partition_type,
		filesystem_type);
}

int t1_efi_roots_test_device_number_is_valid(uint32_t major_number,
	uint32_t minor_number)
{
	return device_number_is_valid(major_number, minor_number);
}

int t1_efi_roots_test_mount_error_status(int error_number)
{
	return mount_error_status(error_number);
}

unsigned long t1_efi_roots_test_mount_flags(void)
{
	return readonly_mount_flags();
}

const char *t1_efi_roots_test_mountpoint_template(void)
{
	return MOUNTPOINT_TEMPLATE;
}

int t1_efi_roots_test_open_mount_path(const char *path)
{
	return open_mount_path(path);
}

int t1_efi_roots_test_parse_mount(const char *line,
	const struct t1_efi_test_candidate *candidate,
	unsigned long long *mount_id, char *path)
{
	return parse_existing_mount(line, candidate, mount_id, path);
}
