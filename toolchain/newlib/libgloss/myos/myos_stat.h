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

/* MYOS_SYS_STAT3: myos_stat2_buf and the owner (the user the file's label
 * names, else 0). The permission bits are what the caller may do. */
struct myos_stat3_buf {
    struct myos_stat2_buf s;
    uint32_t uid;
    uint32_t gid;
};

#endif
