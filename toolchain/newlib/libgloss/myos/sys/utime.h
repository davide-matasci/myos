#ifndef _SYS_UTIME_H
#define _SYS_UTIME_H

/* newlib's dummy <sys/utime.h> has struct utimbuf but no utime(); libgloss
 * implements it over utimensat (posix_stubs.c). */
#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

struct utimbuf {
    time_t actime;
    time_t modtime;
};

int utime(const char *path, const struct utimbuf *times);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_UTIME_H */
