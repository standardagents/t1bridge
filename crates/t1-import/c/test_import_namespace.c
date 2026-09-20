#define _GNU_SOURCE

#include <fcntl.h>
#include <limits.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>

/* Run only inside a disposable user and mount namespace. */
#define CHECK(expression) do { \
	if ((expression) < 0) { perror(#expression); return 1; } \
} while (0)

static void directory(const char *path, mode_t mode)
{
	if (mkdir(path, mode) < 0 || chmod(path, mode) < 0) {
		perror("synthetic directory");
		exit(1);
	}
}

int main(int argc, char **argv)
{
	const char *root;
	char executable[PATH_MAX];
	char descriptor[32];
	int anchor;
	int placeholder;

	if (argc != 3 || realpath(argv[1], executable) == NULL)
		return 1;
	root = argv[2];
	/* Keep mounts private even if this fixture is invoked without unshare. */
	CHECK(unshare(CLONE_NEWNS));
	CHECK(mount(NULL, "/", NULL, MS_REC | MS_PRIVATE, NULL));
	CHECK(mount("tmpfs", root, "tmpfs", MS_NOSUID | MS_NODEV, "mode=0755"));
	CHECK(chdir(root));
	directory("var", 0755);
	directory("var/lib", 0755);
	directory("var/lib/t1bridge", 0700);
	directory("var/lib/t1bridge/machine-data", 0700);
	CHECK(mount("var/lib/t1bridge/machine-data", "var/lib/t1bridge/machine-data",
		NULL, MS_BIND, NULL));

	/* Supply only the test executable, its loader/libraries and descriptor access. */
	directory("usr", 0755);
	directory("proc", 0755);
	CHECK(mount("/usr", "usr", NULL, MS_BIND | MS_REC, NULL));
	CHECK(mount(NULL, "usr", NULL, MS_REMOUNT | MS_BIND | MS_RDONLY, NULL));
	CHECK(symlink("usr/lib", "lib"));
	CHECK(symlink("usr/lib64", "lib64"));
	CHECK(mount("/proc", "proc", NULL, MS_BIND | MS_REC, NULL));
	placeholder = open("test", O_CREAT | O_WRONLY, 0700);
	CHECK(placeholder);
	CHECK(close(placeholder));
	CHECK(mount(executable, "test", NULL, MS_BIND, NULL));
	CHECK(mount(NULL, root, NULL, MS_REMOUNT | MS_BIND | MS_RDONLY, NULL));
	CHECK(chroot(root));
	CHECK(chdir("/"));
	anchor = open("/", O_RDONLY | O_DIRECTORY);
	CHECK(anchor);

	/* Mirror discovery after MachineDataStorage has opened its root anchor. */
	CHECK(unshare(CLONE_NEWNS));
	CHECK(mount(NULL, "/", NULL, MS_REC | MS_PRIVATE, NULL));
	(void)snprintf(descriptor, sizeof(descriptor), "%d", anchor);
	CHECK(setenv("T1BRIDGE_TEST_STALE_ROOT", descriptor, 1));
	execl("/test", "/test", "--exact",
		"storage::tests::commit_uses_current_mount_namespace", "--nocapture",
		"--test-threads=1", (char *)NULL);
	perror("exec test");
	return 1;
}
