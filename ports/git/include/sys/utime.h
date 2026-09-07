#ifndef _MYOS_SYS_UTIME_H_
#define _MYOS_SYS_UTIME_H_

#include <sys/types.h>
#include <time.h>

struct utimbuf {
    time_t actime;
    time_t modtime;
};

int utime(const char *path, const struct utimbuf *times);

#endif
