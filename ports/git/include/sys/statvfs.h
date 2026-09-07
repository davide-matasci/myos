#ifndef _MYOS_SYS_STATVFS_H_
#define _MYOS_SYS_STATVFS_H_

#include <sys/types.h>

struct statvfs {
    unsigned long f_bsize;
    unsigned long f_frsize;
    fsblkcnt_t f_blocks;
    fsblkcnt_t f_bfree;
    fsblkcnt_t f_bavail;
    fsfilcnt_t f_files;
    fsfilcnt_t f_ffree;
    fsfilcnt_t f_favail;
    unsigned long f_fsid;
    unsigned long f_flag;
    unsigned long f_namemax;
};

#ifndef ST_RDONLY
#define ST_RDONLY 1
#endif
#ifndef ST_NOSUID
#define ST_NOSUID 2
#endif

int statvfs(const char *path, struct statvfs *buf);
int fstatvfs(int fd, struct statvfs *buf);

#endif
