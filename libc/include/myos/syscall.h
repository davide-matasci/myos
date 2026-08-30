#ifndef MYOS_SYSCALL_H
#define MYOS_SYSCALL_H

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

#define MYOS_SYSERR ((unsigned long)-1)

long myos_syscall0(long nr);
long myos_syscall1(long nr, long a0);
long myos_syscall3(long nr, long a0, long a1, long a2);

#endif
