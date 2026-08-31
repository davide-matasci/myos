#ifndef _MYOS_EXTRA_H_
#define _MYOS_EXTRA_H_

#include <sys/stat.h>
#include <unistd.h>

int lstat(const char *path, struct stat *st);
ssize_t readlink(const char *path, char *buf, size_t bufsiz);

#endif /* _MYOS_EXTRA_H_ */
