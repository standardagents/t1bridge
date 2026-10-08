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
#include <curl/curl.h>

static int mock_connect;
static CURLcode mock_result;
CURLcode __real_curl_easy_perform(CURL *curl);
CURLcode __wrap_curl_easy_perform(CURL *curl)
{
    return mock_connect ? mock_result : __real_curl_easy_perform(curl);
}

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

    void *tls = NULL;
    assert(t1_fdr_https_open("synthetic.apple.com", &tls) == EINVAL);
    assert(tls == NULL);
    assert(t1_fdr_https_open("127.0.0.1:80", &tls) == EINVAL);
    assert(t1_fdr_https_open("127.0.0.1\n", &tls) == EINVAL);
    assert(t1_fdr_https_read(NULL, output, sizeof(output), &size) == EINVAL);
    assert(t1_fdr_https_write(NULL, output, sizeof(output), &size) == EINVAL);
    const struct { CURLcode status; int error; } failures[] = {
        {CURLE_OPERATION_TIMEDOUT, ETIMEDOUT},
        {CURLE_COULDNT_CONNECT, ECONNREFUSED},
        {CURLE_PEER_FAILED_VERIFICATION, EACCES},
        {CURLE_SSL_CACERT_BADFILE, EACCES},
        {CURLE_SSL_CONNECT_ERROR, EPROTO},
        {CURLE_OUT_OF_MEMORY, ENOMEM}
    };
    mock_connect = 1;
    for (size_t i = 0; i < sizeof(failures)/sizeof(failures[0]); ++i) {
        mock_result = failures[i].status;
        assert(t1_fdr_https_open("192.0.2.1", &tls) == failures[i].error);
        assert(tls == NULL);
    }
    mock_connect = 0;

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
