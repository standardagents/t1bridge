#define _GNU_SOURCE
#include "t1_recovery_io.h"
#include "t1_recovery_fs.h"
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/stat.h>

int main(void)
{
    unsigned char output[64];
    size_t size = 0;
    assert(t1_recovery_xar((const unsigned char *)"synthetic", 9, "Payload", output, sizeof(output), &size) != 0);
    assert(t1_recovery_sha256((const unsigned char *)"abc", 3, output) == 0);
    assert(output[0] == 0xba && output[31] == 0xad);
    /* Protocol refusal is tested without connecting to any endpoint. */
    assert(t1_recovery_https("file:///synthetic", NULL, 0, output, sizeof(output), &size) != 0);
    assert(t1_recovery_usb_bulk(-1, 1, output, sizeof(output), 1, &size) == EBADF);

    char path[] = "/tmp/t1-recovery-synthetic.XXXXXX";
    assert(mkdtemp(path));
    int root = -1, child = -1, file = -1;
    assert(t1_recovery_root(path, 0, &root) == 0);
    assert(t1_recovery_child(root, "..", 1, 1, &child) == EINVAL);
    assert(t1_recovery_child(root, "stage", 1, 1, &child) == 0);
    assert(t1_recovery_file(child, "data", 1, &file) == 0);
    assert(write(file, "synthetic", 9) == 9);
    close(file);
    assert(t1_recovery_file(child, "data", 1, &file) == EEXIST);
    assert(symlinkat("data", child, "link") == 0);
    assert(t1_recovery_file(child, "link", 0, &file) != 0);
    assert(mkfifoat(child, "fifo", 0600) == 0);
    assert(t1_recovery_file(child, "fifo", 0, &file) != 0);
    assert(t1_recovery_rename(child, "data", child, "link") == EEXIST);
    assert(t1_recovery_rename(child, "data", child, "saved") == 0);
    assert(unlinkat(child, "saved", 0) == 0);
    assert(unlinkat(child, "link", 0) == 0);
    assert(unlinkat(child, "fifo", 0) == 0);
    close(child);
    assert(unlinkat(root, "stage", AT_REMOVEDIR) == 0);
    close(root);
    assert(rmdir(path) == 0);
    return 0;
}
