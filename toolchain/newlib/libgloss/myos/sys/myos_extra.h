#ifndef _MYOS_EXTRA_H_
#define _MYOS_EXTRA_H_

#include <sys/stat.h>
#include <unistd.h>

int lstat(const char *path, struct stat *st);
ssize_t readlink(const char *path, char *buf, size_t bufsiz);
int mknod(const char *path, mode_t mode, dev_t dev);
int mkfifo(const char *path, mode_t mode);

#endif /* _MYOS_EXTRA_H_ */
