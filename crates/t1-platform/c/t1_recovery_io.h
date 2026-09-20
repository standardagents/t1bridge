#ifndef T1_RECOVERY_IO_H
#define T1_RECOVERY_IO_H
#include <stddef.h>
#include <stdint.h>
int t1_recovery_https(const char *, const unsigned char *, size_t, unsigned char *, size_t, size_t *);
int t1_recovery_sha256(const unsigned char *, size_t, unsigned char[32]);
int t1_recovery_xar(const unsigned char *, size_t, const char *, unsigned char *, size_t, size_t *);
int t1_recovery_usb_claim(int, unsigned int);
int t1_recovery_usb_release(int, unsigned int);
int t1_recovery_usb_configuration(int, unsigned int);
int t1_recovery_usb_control(int, uint8_t, uint8_t, uint16_t, uint16_t, unsigned char *, uint16_t, unsigned int, size_t *);
int t1_recovery_usb_bulk(int, unsigned int, unsigned char *, size_t, unsigned int, size_t *);
int t1_recovery_reset(int, uint32_t, uint32_t, int);
#endif
