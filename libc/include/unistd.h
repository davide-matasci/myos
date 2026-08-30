#ifndef UNISTD_H
#define UNISTD_H

#include <stddef.h>
#include <sys/types.h>

#define STDIN_FILENO 0
#define STDOUT_FILENO 1
#define STDERR_FILENO 2

ssize_t write(int fd, const void *buf, size_t len);
ssize_t read(int fd, void *buf, size_t len);
int open(const char *path);
int close(int fd);
void _exit(int code);
int fork(void);
int wait(int *status);
int pipe(int fds[2]);
int dup2(int oldfd, int newfd);
void *sbrk(intptr_t inc);

#endif
