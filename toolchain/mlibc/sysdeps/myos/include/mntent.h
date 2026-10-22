/* myos libgloss: the mount table as getmntent reads it (mntent.c), from
 * /proc/mounts. Installed into the newlib sysroot as <mntent.h>. */
#ifndef _MYOS_MNTENT_H_
#define _MYOS_MNTENT_H_

#include <stdio.h>

#ifdef __cplusplus
extern "C" {
#endif

#define MOUNTED "/proc/mounts"
#define _PATH_MOUNTED "/proc/mounts"
#define MNTTYPE_IGNORE "ignore"
#define MNTOPT_DEFAULTS "defaults"
#define MNTOPT_RO "ro"
#define MNTOPT_RW "rw"

struct mntent {
    char *mnt_fsname;
    char *mnt_dir;
    char *mnt_type;
    char *mnt_opts;
    int mnt_freq;
    int mnt_passno;
};

FILE *setmntent(const char *path, const char *mode);
struct mntent *getmntent(FILE *fp);
struct mntent *getmntent_r(FILE *fp, struct mntent *me, char *buf, int size);
int endmntent(FILE *fp);
char *hasmntopt(const struct mntent *me, const char *opt);

#ifdef __cplusplus
}
#endif

#endif
