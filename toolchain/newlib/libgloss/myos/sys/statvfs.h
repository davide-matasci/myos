/* myos libgloss: statvfs and fstatvfs (statvfs.c), how full the filesystem
 * holding a file is. Installed into the newlib sysroot as <sys/statvfs.h>. */
#ifndef _MYOS_SYS_STATVFS_H_
#define _MYOS_SYS_STATVFS_H_

#include <sys/types.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Sizes in blocks of f_frsize bytes (f_bsize is the same). */
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

/* f_flag */
#define ST_RDONLY 1
#define ST_NOSUID 2

int statvfs(const char *__restrict path, struct statvfs *__restrict buf);
int fstatvfs(int fd, struct statvfs *buf);

#ifdef __cplusplus
}
#endif

#endif
