#ifndef T1_RECOVERY_FS_H
#define T1_RECOVERY_FS_H
#include <stdint.h>
int t1_recovery_root(const char *, int, int *);
int t1_recovery_child(int, const char *, int, int, int *);
int t1_recovery_lock(int);
int t1_recovery_exists(int, const char *, int *);
int t1_recovery_file(int, const char *, int, int *);
int t1_recovery_rename(int, const char *, int, const char *);
int t1_recovery_space(int, uint64_t);
#endif
