#ifndef MYOS_SYSCALLS_H
#define MYOS_SYSCALLS_H

#include <stddef.h>

#define MYOS_SYS_WRITE 0
#define MYOS_SYS_EXIT 1
#define MYOS_SYS_OPEN 2
#define MYOS_SYS_READ 3
#define MYOS_SYS_CLOSE 4
#define MYOS_SYS_EXEC 5
#define MYOS_SYS_FORK 6
#define MYOS_SYS_WAIT 7
#define MYOS_SYS_LISTDIR 8
#define MYOS_SYS_BRK 9
#define MYOS_SYS_PIPE 10
#define MYOS_SYS_DUP2 11
#define MYOS_SYS_STAT 12
#define MYOS_SYS_EXECNAME 13
#define MYOS_SYS_DUPFD 14
#define MYOS_SYS_CHDIR 15
#define MYOS_SYS_GETCWD 16
#define MYOS_SYS_MKDIR 17
#define MYOS_SYS_RMDIR 18
#define MYOS_SYS_UNLINK 19
#define MYOS_SYS_RENAME 20
#define MYOS_SYS_SYMLINK 21
#define MYOS_SYS_READLINK 22
#define MYOS_SYS_MMAP 23
#define MYOS_SYS_MUNMAP 24
#define MYOS_SYS_MPROTECT 25
#define MYOS_SYS_LSEEK 26
#define MYOS_SYS_MOUNT 27

#define MYOS_SYSERR ((unsigned long)-1)

long myos_syscall0(long nr);
long myos_syscall1(long nr, long a0);
long myos_syscall3(long nr, long a0, long a1, long a2);

int myos_fd_is_tty(int fd);
void myos_fd_set_tty(int fd, int on);
void myos_fd_dup_tty(int oldfd, int newfd);

#endif
