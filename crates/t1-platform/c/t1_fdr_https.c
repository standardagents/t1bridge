/* TLS upgrade for the one legacy FDR bootstrap service. No HTTP fallback. */
#include "t1_recovery_io.h"
#include <curl/curl.h>
#include <arpa/inet.h>
#include <errno.h>
#include <stdio.h>
#include <string.h>

static int connection_error(CURLcode status)
{
    switch (status) {
    case CURLE_OPERATION_TIMEDOUT: return ETIMEDOUT;
    case CURLE_COULDNT_CONNECT: return ECONNREFUSED;
    case CURLE_PEER_FAILED_VERIFICATION:
    case CURLE_SSL_CACERT_BADFILE: return EACCES;
    case CURLE_SSL_CONNECT_ERROR: return EPROTO;
    case CURLE_OUT_OF_MEMORY: return ENOMEM;
    default: return EIO;
    }
}

int t1_fdr_https_open(const char *address, void **output)
{
    if (!address || !output) return EINVAL;
    *output = NULL;
    unsigned char parsed[16];
    int ipv6 = inet_pton(AF_INET6, address, parsed) == 1;
    if (!ipv6 && inet_pton(AF_INET, address, parsed) != 1) return EINVAL;
    char destination[128];
    int length = snprintf(destination, sizeof(destination), ipv6 ?
                          "gg.apple.com:443:[%s]" : "gg.apple.com:443:%s", address);
    if (length < 0 || (size_t)length >= sizeof(destination)) return EINVAL;
    struct curl_slist *resolve = curl_slist_append(NULL, destination);
    if (!resolve) return ENOMEM;
    CURL *curl = curl_easy_init();
    if (!curl) { curl_slist_free_all(resolve); return ENOMEM; }
    int result = EIO;
#define OPTION(key, value) do { \
    if (curl_easy_setopt(curl, key, value) != CURLE_OK) goto done; \
} while (0)
    OPTION(CURLOPT_URL, "https://gg.apple.com/");
    OPTION(CURLOPT_PROTOCOLS_STR, "https");
    OPTION(CURLOPT_PROXY, "");
    OPTION(CURLOPT_NETRC, (long)CURL_NETRC_IGNORED);
    OPTION(CURLOPT_FOLLOWLOCATION, 0L);
    OPTION(CURLOPT_RESOLVE, resolve);
    OPTION(CURLOPT_CONNECT_ONLY, 1L);
    OPTION(CURLOPT_SSL_VERIFYPEER, 1L);
    OPTION(CURLOPT_SSL_VERIFYHOST, 2L);
    OPTION(CURLOPT_SSLVERSION, (long)CURL_SSLVERSION_TLSv1_2);
    OPTION(CURLOPT_HTTP_VERSION, (long)CURL_HTTP_VERSION_1_1);
    OPTION(CURLOPT_SSL_ENABLE_ALPN, 0L);
    OPTION(CURLOPT_CONNECTTIMEOUT, 5L);
    OPTION(CURLOPT_TIMEOUT, 10L);
    OPTION(CURLOPT_NOSIGNAL, 1L);
    CURLcode status = curl_easy_perform(curl);
    if (status != CURLE_OK) { result = connection_error(status); goto done; }
    long verified = -1;
    if (curl_easy_getinfo(curl, CURLINFO_SSL_VERIFYRESULT, &verified) != CURLE_OK) goto done;
    if (verified != 0) { result = EACCES; goto done; }
    OPTION(CURLOPT_RESOLVE, NULL);
    *output = curl;
    curl = NULL;
    result = 0;
done:
    curl_easy_cleanup(curl);
    curl_slist_free_all(resolve);
    return result;
#undef OPTION
}

int t1_fdr_https_read(void *handle, unsigned char *bytes, size_t size, size_t *actual)
{
    if (!handle || !bytes || !actual || size > 1024U * 1024U) return EINVAL;
    *actual = 0;
    CURLcode status = curl_easy_recv(handle, bytes, size, actual);
    return status == CURLE_OK ? 0 : status == CURLE_AGAIN ? EAGAIN : EIO;
}

int t1_fdr_https_write(void *handle, const unsigned char *bytes, size_t size, size_t *actual)
{
    if (!handle || !bytes || !actual || size > 1024U * 1024U) return EINVAL;
    *actual = 0;
    CURLcode status = curl_easy_send(handle, bytes, size, actual);
    return status == CURLE_OK ? 0 : status == CURLE_AGAIN ? EAGAIN : EIO;
}

void t1_fdr_https_close(void *handle)
{
    curl_easy_cleanup(handle);
}
