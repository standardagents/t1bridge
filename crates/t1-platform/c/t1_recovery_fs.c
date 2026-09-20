#define _GNU_SOURCE
#include "t1_recovery_fs.h"
#include <errno.h>
#include <fcntl.h>
#include <libudev.h>
#include <linux/magic.h>
#include <linux/openat2.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <strings.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <sys/statfs.h>
#include <sys/statvfs.h>
#include <sys/syscall.h>
#include <unistd.h>

static int valid_name(const char *name)
{
    return name && *name && strcmp(name, ".") && strcmp(name, "..") &&
        !strchr(name, '/') && strlen(name) <= 128;
}

static int directory_ok(int fd, int private_directory)
{
    struct stat st;
    if (fstat(fd, &st) < 0) return errno;
    if (!S_ISDIR(st.st_mode) || st.st_uid != geteuid() ||
        (st.st_mode & (private_directory ? 077 : 022))) return EPERM;
    return 0;
}

static int is_esp(int fd)
{
    struct stat st;
    struct statfs fs;
    if (fstat(fd, &st) < 0 || fstatfs(fd, &fs) < 0) return errno;
    if (fs.f_type != MSDOS_SUPER_MAGIC || st.st_ino != 1) return EINVAL;
    struct udev *udev = udev_new();
    if (!udev) return ENOMEM;
    struct udev_device *device = udev_device_new_from_devnum(udev, 'b', st.st_dev);
    int result = EINVAL;
    if (device) {
        const char *type = udev_device_get_property_value(device, "ID_PART_ENTRY_TYPE");
        if (type && !strcasecmp(type, "c12a7328-f81f-11d2-ba4b-00a0c93ec93b")) result = 0;
        udev_device_unref(device);
    }
    udev_unref(udev);
    return result;
}

int t1_recovery_root(const char *path, int esp, int *output)
{
    if (!path || path[0] != '/' || !output) return EINVAL;
    *output = -1;
    struct open_how how = {.flags = O_RDONLY | O_DIRECTORY | O_CLOEXEC,
        .resolve = RESOLVE_NO_SYMLINKS};
    int fd = (int)syscall(SYS_openat2, AT_FDCWD, path, &how, sizeof(how));
    if (fd < 0) return errno;
    int result = directory_ok(fd, 0);
    if (!result && esp) result = is_esp(fd);
    if (result) { close(fd); return result; }
    *output = fd;
    return 0;
}

int t1_recovery_child(int parent, const char *name, int create, int private_directory, int *output)
{
    if (!valid_name(name) || !output) return EINVAL;
    *output = -1;
    if (create && mkdirat(parent, name, 0700) < 0 && errno != EEXIST) return errno;
    struct open_how how = {.flags = O_RDONLY | O_DIRECTORY | O_CLOEXEC,
        .resolve = RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS | RESOLVE_NO_XDEV};
    int fd = (int)syscall(SYS_openat2, parent, name, &how, sizeof(how));
    if (fd < 0) return errno;
    int result = directory_ok(fd, private_directory);
    if (!result && create && fsync(parent) < 0) result = errno;
    if (result) { close(fd); return result; }
    *output = fd;
    return 0;
}

int t1_recovery_lock(int fd)
{
    return flock(fd, LOCK_EX | LOCK_NB) < 0 ? errno : 0;
}

int t1_recovery_exists(int fd, const char *name, int *exists)
{
    if (!valid_name(name) || !exists) return EINVAL;
    struct stat st;
    if (fstatat(fd, name, &st, AT_SYMLINK_NOFOLLOW) < 0) {
        if (errno != ENOENT) return errno;
        *exists = 0;
    } else { *exists = 1; }
    return 0;
}

int t1_recovery_file(int fd, const char *name, int create, int *output)
{
    if (!valid_name(name) || !output) return EINVAL;
    *output = -1;
    int source;
    if (create) {
        source = openat(fd, name, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    } else {
        int anchor = openat(fd, name, O_PATH | O_NOFOLLOW | O_CLOEXEC);
        if (anchor < 0) return errno;
        struct stat st;
        if (fstat(anchor, &st) < 0 || !S_ISREG(st.st_mode)) { close(anchor); return EINVAL; }
        char path[64];
        int length = snprintf(path, sizeof(path), "/proc/self/fd/%d", anchor);
        if (length < 0 || (size_t)length >= sizeof(path)) { close(anchor); return EINVAL; }
        source = open(path, O_RDONLY | O_NONBLOCK | O_CLOEXEC);
        int saved = errno;
        close(anchor);
        errno = saved;
    }
    if (source < 0) return errno;
    struct stat st;
    if (fstat(source, &st) < 0 || !S_ISREG(st.st_mode) || st.st_uid != geteuid() || st.st_nlink != 1) {
        close(source); return EPERM;
    }
    *output = source;
    return 0;
}

int t1_recovery_rename(int source, const char *old_name, int destination, const char *new_name)
{
    if (!valid_name(old_name) || !valid_name(new_name)) return EINVAL;
    return syscall(SYS_renameat2, source, old_name, destination, new_name, RENAME_NOREPLACE) < 0 ? errno : 0;
}

int t1_recovery_space(int fd, uint64_t required)
{
    struct statvfs st;
    if (fstatvfs(fd, &st) < 0) return errno;
    if (st.f_flag & ST_RDONLY) return EROFS;
    if (st.f_frsize == 0 || st.f_bavail < (required / st.f_frsize + 1)) return ENOSPC;
    return 0;
}
