#ifndef MYOS_STAT_H
#define MYOS_STAT_H

#include <stdint.h>

/* Layout written by MYOS_SYS_STAT into userspace. */
struct myos_stat_buf {
    uint32_t st_mode;
    uint32_t st_size;
    uint32_t st_ino;
    uint32_t st_nlink;
    uint32_t st_dev;
};

#endif
