#ifndef MYOS_STAT_H
#define MYOS_STAT_H

#include <stdint.h>

/* Layout written by MYOS_SYS_STAT2 into userspace (kernel MyosStat2Buf).
 * Times are seconds since the epoch, 0 where the filesystem keeps none. */
struct myos_stat2_buf {
    uint32_t st_mode;
    uint32_t st_nlink;
    uint32_t st_ino;
    uint32_t st_dev;
    uint64_t st_size;
    int64_t atime;
    int64_t mtime;
};

#endif
