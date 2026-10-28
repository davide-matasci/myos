/* myos: WebKit (built as for Linux) asks mincore which of its pages are
 * in memory, which mlibc has for Linux alone. On myos every page that
 * has been touched is (there is no swap), and an untouched one costs
 * nothing: every page is reported resident. */
#ifndef MYOS_COMPAT_SYS_MMAN_H
#define MYOS_COMPAT_SYS_MMAN_H

#include_next <sys/mman.h>

#include <string.h>
#include <unistd.h>

static inline int mincore(void *addr, size_t length, unsigned char *vec)
{
    long page = sysconf(_SC_PAGESIZE);
    (void)addr;
    memset(vec, 1, (length + (size_t)page - 1) / (size_t)page);
    return 0;
}

#endif
