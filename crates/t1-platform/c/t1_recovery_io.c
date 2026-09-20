/* Narrow, synchronous library and Linux USB boundary for attended recovery.
 * No subprocesses, filesystem extraction, listeners, or certificate bypass. */
#include "t1_recovery_io.h"
#include <archive.h>
#include <archive_entry.h>
#include <curl/curl.h>
#include <openssl/evp.h>
#include <linux/usbdevice_fs.h>
#include <sys/ioctl.h>
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <errno.h>
#include <limits.h>

struct bounded_body {
    unsigned char *bytes;
    size_t capacity;
    size_t used;
};

static size_t receive_body(char *bytes, size_t size, size_t count, void *opaque)
{
    struct bounded_body *body = opaque;
    if (size && count > SIZE_MAX / size) return 0;
    size_t length = size * count;
    if (length > body->capacity - body->used) return 0;
    memcpy(body->bytes + body->used, bytes, length);
    body->used += length;
    return length;
}

int t1_recovery_https(const char *url, const unsigned char *request,
                      size_t request_size, unsigned char *output,
                      size_t capacity, size_t *output_size)
{
    if (!url || !output || !output_size || capacity > 64U * 1024U * 1024U ||
        request_size > 16U * 1024U * 1024U) return EINVAL;
    *output_size = 0;
    CURL *curl = curl_easy_init();
    if (!curl) return ENOMEM;
    struct bounded_body body = {output, capacity, 0};
    struct curl_slist *headers = NULL;
    int result = EIO;
    long status = 0;
#define OPTION(key, value) do { \
    if (curl_easy_setopt(curl, key, value) != CURLE_OK) goto done; \
} while (0)
    OPTION(CURLOPT_URL, url);
    OPTION(CURLOPT_PROTOCOLS_STR, "https");
    OPTION(CURLOPT_FOLLOWLOCATION, 0L);
    OPTION(CURLOPT_SSL_VERIFYPEER, 1L);
    OPTION(CURLOPT_SSL_VERIFYHOST, 2L);
    OPTION(CURLOPT_CONNECTTIMEOUT, 30L);
    OPTION(CURLOPT_TIMEOUT, 600L);
    OPTION(CURLOPT_LOW_SPEED_LIMIT, 1024L);
    OPTION(CURLOPT_LOW_SPEED_TIME, 60L);
    OPTION(CURLOPT_NOSIGNAL, 1L);
    /* Privileged operations must not inherit an environment-controlled proxy. */
    OPTION(CURLOPT_PROXY, "");
    OPTION(CURLOPT_NETRC, (long)CURL_NETRC_IGNORED);
    OPTION(CURLOPT_USERAGENT, "T1Bridge/native-recovery");
    OPTION(CURLOPT_WRITEFUNCTION, receive_body);
    OPTION(CURLOPT_WRITEDATA, &body);
    if (request) {
        headers = curl_slist_append(NULL, "Content-Type: text/xml; charset=utf-8");
        if (!headers) { result = ENOMEM; goto done; }
        OPTION(CURLOPT_HTTPHEADER, headers);
        OPTION(CURLOPT_POSTFIELDS, request);
        OPTION(CURLOPT_POSTFIELDSIZE_LARGE, (curl_off_t)request_size);
    }
    if (curl_easy_perform(curl) != CURLE_OK ||
        curl_easy_getinfo(curl, CURLINFO_RESPONSE_CODE, &status) != CURLE_OK ||
        status != 200) goto done;
    *output_size = body.used;
    result = 0;
done:
    curl_slist_free_all(headers);
    curl_easy_cleanup(curl);
    return result;
#undef OPTION
}

int t1_recovery_sha256(const unsigned char *input, size_t size,
                       unsigned char output[32])
{
    unsigned int produced = 0;
    return EVP_Digest(input, size, output, &produced, EVP_sha256(), NULL) == 1 &&
        produced == 32 ? 0 : EIO;
}

/* Read exactly one regular XAR member. Paths are compared, never extracted. */
int t1_recovery_xar(const unsigned char *input, size_t input_size,
                    const char *name, unsigned char *output,
                    size_t capacity, size_t *output_size)
{
    if (!input || !name || !output || !output_size ||
        input_size > 64U * 1024U * 1024U || capacity > 64U * 1024U * 1024U)
        return EINVAL;
    *output_size = 0;
    struct archive *a = archive_read_new();
    if (!a) return ENOMEM;
    int result = EIO, found = 0, status;
    struct archive_entry *entry;
    if (archive_read_support_format_xar(a) != ARCHIVE_OK ||
        archive_read_open_memory(a, input, input_size) != ARCHIVE_OK) goto done;
    unsigned int entries = 0;
    while ((status = archive_read_next_header(a, &entry)) == ARCHIVE_OK) {
        if (++entries > 1024) goto done;
        const char *path = archive_entry_pathname(entry);
        if (!path) goto done;
        if (strcmp(path, name)) {
            if (archive_read_data_skip(a) != ARCHIVE_OK) goto done;
            continue;
        }
        if (found || archive_entry_filetype(entry) != AE_IFREG ||
            archive_entry_symlink(entry) || archive_entry_hardlink(entry)) goto done;
        la_int64_t declared = archive_entry_size(entry);
        if (declared <= 0 || (uint64_t)declared > capacity) goto done;
        size_t used = 0;
        while (used < (size_t)declared) {
            la_ssize_t count = archive_read_data(a, output + used, (size_t)declared - used);
            if (count <= 0) goto done;
            used += (size_t)count;
        }
        unsigned char extra;
        if (archive_read_data(a, &extra, 1) != 0) goto done;
        *output_size = used;
        found = 1;
    }
    if (status == ARCHIVE_EOF && found) result = 0;
done:
    archive_read_free(a);
    if (result) *output_size = 0;
    return result;
}

int t1_recovery_usb_claim(int fd, unsigned int interface)
{
    return ioctl(fd, USBDEVFS_CLAIMINTERFACE, &interface) < 0 ? errno : 0;
}

int t1_recovery_usb_release(int fd, unsigned int interface)
{
    return ioctl(fd, USBDEVFS_RELEASEINTERFACE, &interface) < 0 ? errno : 0;
}

int t1_recovery_usb_configuration(int fd, unsigned int configuration)
{
    return ioctl(fd, USBDEVFS_SETCONFIGURATION, &configuration) < 0 ? errno : 0;
}

int t1_recovery_reset(int fd, uint32_t bus, uint32_t address, int execute)
{
    struct { uint32_t bus, address; } target = {bus, address};
    unsigned long command = execute ? _IOW('T', 0x52, target) : _IOW('T', 0x51, target);
    return ioctl(fd, command, &target) < 0 ? errno : 0;
}

int t1_recovery_usb_control(int fd, uint8_t type, uint8_t request,
                            uint16_t value, uint16_t index, unsigned char *bytes,
                            uint16_t size, unsigned int timeout, size_t *actual)
{
    struct usbdevfs_ctrltransfer transfer = {
        .bRequestType = type, .bRequest = request, .wValue = value,
        .wIndex = index, .wLength = size, .timeout = timeout, .data = bytes
    };
    int result = ioctl(fd, USBDEVFS_CONTROL, &transfer);
    if (result < 0) return errno;
    *actual = (size_t)result;
    return 0;
}

int t1_recovery_usb_bulk(int fd, unsigned int endpoint, unsigned char *bytes,
                         size_t size, unsigned int timeout, size_t *actual)
{
    if (size > 1024U * 1024U) return EINVAL;
    struct usbdevfs_bulktransfer transfer = {
        .ep = endpoint, .len = (unsigned int)size, .timeout = timeout, .data = bytes
    };
    int result = ioctl(fd, USBDEVFS_BULK, &transfer);
    if (result < 0) return errno;
    *actual = (size_t)result;
    return 0;
}
