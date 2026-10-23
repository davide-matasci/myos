#ifndef MYOS_STAT_H
#define MYOS_STAT_H

#include <stdint.h>

/* What MYOS_SYS_STATAT writes (kernel MyosStat, kernel/src/user/at.rs).
 * Times are seconds since the epoch, 0 where the filesystem keeps none; the
 * permission bits are what the caller may do; uid is the user the file's
 * label names, else 0. */
struct myos_stat {
    uint32_t st_mode;
    uint32_t st_nlink;
    uint32_t st_ino;
    uint32_t st_dev;
    uint64_t st_size;
    int64_t atime;
    int64_t mtime;
    uint32_t uid;
    uint32_t gid;
};

#endif
