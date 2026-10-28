/* myos: WebKit (built as for Linux) asks sysinfo for the memory's size.
 * Its fields, as Linux's, filled from sysconf: the physical pages and the
 * free ones; the rest is zero. */
#ifndef MYOS_COMPAT_SYS_SYSINFO_H
#define MYOS_COMPAT_SYS_SYSINFO_H

#include <string.h>
#include <unistd.h>

struct sysinfo {
    long uptime;
    unsigned long loads[3];
    unsigned long totalram;
    unsigned long freeram;
    unsigned long sharedram;
    unsigned long bufferram;
    unsigned long totalswap;
    unsigned long freeswap;
    unsigned short procs;
    unsigned long totalhigh;
    unsigned long freehigh;
    unsigned int mem_unit;
};

static inline int sysinfo(struct sysinfo *info)
{
    long pages = sysconf(_SC_PHYS_PAGES), avail = sysconf(_SC_AVPHYS_PAGES);
    long size = sysconf(_SC_PAGESIZE);
    if (pages < 0 || size < 0)
        return -1;
    memset(info, 0, sizeof *info);
    info->totalram = (unsigned long)pages;
    info->freeram = avail < 0 ? 0 : (unsigned long)avail;
    info->mem_unit = (unsigned int)size;
    return 0;
}

#endif
