#ifndef _MYOS_EXTRA_H_
#define _MYOS_EXTRA_H_

#include <sys/stat.h>
#include <unistd.h>

int lstat(const char *path, struct stat *st);
ssize_t readlink(const char *path, char *buf, size_t bufsiz);
int mknod(const char *path, mode_t mode, dev_t dev);
int mkfifo(const char *path, mode_t mode);
int _mount(const char *source, const char *target, const char *fstype);
int mount(const char *source, const char *target, const char *fstype, ...);

/* Security (docs/security.md): run as `name` (its password, or "" for a
 * user without one), narrow the namespace to `spec` ("TARGET SOURCE
 * RIGHTS" lines), make the policy at `path` the system's. 0, or -1. */
int myos_setuser(const char *name, const char *password);
int myos_ns(const char *spec);
int myos_policy_load(const char *path);

#endif /* _MYOS_EXTRA_H_ */
