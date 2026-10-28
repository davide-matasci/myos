/* The C both of myos's C libraries share (docs/mlibc.md): the POSIX calls over
 * the kernel's syscalls and files, each defined as __myos_NAME and exported
 * as NAME by the libc (libc.h). This header declares them for the code
 * that calls them by these names: newlib's hooks (libgloss/myos/hooks.c)
 * and mlibc's sysdeps (toolchain/mlibc/sysdeps/myos). The types are the
 * libc's own, from its headers. The kernel-level pieces (the raw syscalls,
 * the signal records, the tty and socket helpers) are in myos_syscalls.h. */
#ifndef LIBMYOS_H
#define LIBMYOS_H

#include <grp.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <poll.h>
#include <pwd.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/ipc.h>
#include <sys/mman.h>
#include <sys/select.h>
#include <sys/shm.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/time.h>
#include <sys/timerfd.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <sys/utsname.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

#include "myos_syscalls.h"

#ifdef __cplusplus
extern "C" {
#endif

/* ---- fdio.c: the fd calls, and the process calls newlib hooks -------- */
int __myos_close(int fd);
void __myos_exit(int status) __attribute__((noreturn));
ssize_t __myos_read(int fd, void *buf, size_t cnt);
ssize_t __myos_pread(int fd, void *buf, size_t cnt, off_t off);
ssize_t __myos_write(int fd, const void *buf, size_t cnt);
ssize_t __myos_pwrite(int fd, const void *buf, size_t cnt, off_t off);
off_t __myos_lseek(int fd, off_t pos, int whence);
void *__myos_sbrk(ptrdiff_t incr);
int __myos_getpid(void);
int __myos_kill(int pid, int sig);
int __myos_fork(void);
pid_t __myos_waitpid(pid_t pid, int *status, int options);
pid_t __myos_wait(int *status);
int __myos_gettimeofday(struct timeval *tv, void *tz);

/* ---- at.c: the path calls --------------------------------------------- */
int __myos_fstatat(int dirfd, const char *path, struct stat *st, int flags);
int __myos_stat(const char *path, struct stat *st);
int __myos_lstat(const char *path, struct stat *st);
int __myos_fstat(int fd, struct stat *st);
int __myos_faccessat(int dirfd, const char *path, int mode, int flags);
int __myos_access(const char *path, int mode);
int __myos_openat(int dirfd, const char *path, int flags, ...);
int __myos_open(const char *path, int flags, ...);
int __myos_mkdirat(int dirfd, const char *path, mode_t mode);
int __myos_mkdir(const char *path, mode_t mode);
int __myos_mkfifoat(int dirfd, const char *path, mode_t mode);
int __myos_mkfifo(const char *path, mode_t mode);
int __myos_mknodat(int dirfd, const char *path, mode_t mode, dev_t dev);
int __myos_mknod(const char *path, mode_t mode, dev_t dev);
int __myos_symlinkat(const char *target, int dirfd, const char *path);
int __myos_symlink(const char *target, const char *path);
int __myos_unlinkat(int dirfd, const char *path, int flags);
int __myos_unlink(const char *path);
int __myos_rmdir(const char *path);
int __myos_renameat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath);
int __myos_rename(const char *oldpath, const char *newpath);
ssize_t __myos_readlinkat(int dirfd, const char *path, char *buf, size_t size);
ssize_t __myos_readlink(const char *path, char *buf, size_t size);
int __myos_utimensat(int dirfd, const char *path, const struct timespec times[2], int flags);
int __myos_futimens(int fd, const struct timespec times[2]);
int __myos_chdir(const char *path);
int __myos_fchdir(int fd);
char *__myos_getcwd(char *buf, size_t size);
int __myos_chroot(const char *path);
int __myos_execve(const char *path, char *const argv[], char *const envp[]);
int __myos_fexecve(int fd, char *const argv[], char *const envp[]);

/* ---- proc.c: the fd table, sessions, the host, the limits, the timer ---- */
int __myos_dup2(int oldfd, int newfd);
int __myos_fcntl(int fd, int cmd, long wide);
int __myos_pipe(int fildes[2]);
int __myos_flock(int fd, int operation);
int __myos_ftruncate(int fd, off_t length);
int __myos_link(const char *oldpath, const char *newpath);
int __myos_linkat(int olddirfd, const char *oldpath, int newdirfd, const char *newpath, int flags);
int __myos_chmod(const char *path, mode_t mode);
int __myos_fchmod(int fd, mode_t mode);
int __myos_fchmodat(int dirfd, const char *path, mode_t mode, int flags);
int __myos_chown(const char *path, uid_t owner, gid_t group);
int __myos_lchown(const char *path, uid_t owner, gid_t group);
int __myos_fchown(int fd, uid_t owner, gid_t group);
int __myos_fchownat(int dirfd, const char *path, uid_t owner, gid_t group, int flags);
int __myos_setsid(void);
int __myos_setpgid(pid_t pid, pid_t pgid);
pid_t __myos_getpgid(pid_t pid);
pid_t __myos_getsid(pid_t pid);
pid_t __myos_getppid(void);
int __myos_gethostname(char *name, size_t len);
int __myos_sethostname(const char *name, size_t len);
int __myos_uname(struct utsname *buf);
int __myos_rlimit(int resource, uint64_t *cur, uint64_t *max);
int __myos_main_stack(void **lo, size_t *size);
int __myos_setitimer(int which, const struct itimerval *value, struct itimerval *old);
int __myos_getitimer(int which, struct itimerval *value);
int __myos_ttyname_r(int fd, char *buf, size_t len);
char *__myos_ttyname(int fd);

/* ---- mmap.c, shm.c: memory ------------------------------------------- */
void *__myos_mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset);
int __myos_munmap(void *addr, size_t length);
int __myos_mprotect(void *addr, size_t length, int prot);
int __myos_msync(void *addr, size_t length, int flags);
int __myos_madvise(void *addr, size_t length, int advice);
int __myos_shm_open(const char *name, int oflag, mode_t mode);
int __myos_shm_unlink(const char *name);
int __myos_memfd_create(const char *name, unsigned flags);
key_t __myos_ftok(const char *path, int id);
int __myos_shmget(key_t key, size_t size, int shmflg);
void *__myos_shmat(int shmid, const void *shmaddr, int shmflg);
int __myos_shmdt(const void *shmaddr);
int __myos_shmctl(int shmid, int cmd, struct shmid_ds *buf);

/* ---- pollselect.c, sleep.c, time.c: waiting and the clocks ------------ */
int __myos_poll(struct pollfd *fds, nfds_t nfds, int timeout);
int __myos_select(int nfds, fd_set *readfds, fd_set *writefds, fd_set *exceptfds, struct timeval *timeout);
int __myos_pselect(int nfds, fd_set *readfds, fd_set *writefds, fd_set *exceptfds,
                   const struct timespec *timeout, const sigset_t *sigmask);
int __myos_nanosleep(const struct timespec *req, struct timespec *rem);
unsigned __myos_sleep(unsigned seconds);
int __myos_usleep(useconds_t usec);
int __myos_clock_gettime(clockid_t clock_id, struct timespec *tp);
int __myos_clock_getres(clockid_t clock_id, struct timespec *res);
int __myos_settimeofday(const struct timeval *tv, const struct timezone *tz);
int __myos_clock_settime(clockid_t clock_id, const struct timespec *tp);

/* ---- socket.c, ifaddrs.c: BSD sockets over /net ----------------------- */
int __myos_socket(int domain, int type, int protocol);
int __myos_bind(int sockfd, const struct sockaddr *addr, socklen_t addrlen);
int __myos_socketpair(int domain, int type, int protocol, int sv[2]);
int __myos_listen(int sockfd, int backlog);
int __myos_accept(int sockfd, struct sockaddr *addr, socklen_t *addrlen);
int __myos_connect(int sockfd, const struct sockaddr *addr, socklen_t addrlen);
int __myos_shutdown(int sockfd, int how);
int __myos_setsockopt(int sockfd, int level, int optname, const void *optval, socklen_t optlen);
int __myos_getsockopt(int sockfd, int level, int optname, void *optval, socklen_t *optlen);
int __myos_getsockname(int sockfd, struct sockaddr *addr, socklen_t *addrlen);
int __myos_getpeername(int sockfd, struct sockaddr *addr, socklen_t *addrlen);
ssize_t __myos_send(int sockfd, const void *buf, size_t len, int flags);
ssize_t __myos_recv(int sockfd, void *buf, size_t len, int flags);
ssize_t __myos_sendmsg(int sockfd, const struct msghdr *msg, int flags);
ssize_t __myos_recvmsg(int sockfd, struct msghdr *msg, int flags);
ssize_t __myos_sendto(int sockfd, const void *buf, size_t len, int flags,
                      const struct sockaddr *dest_addr, socklen_t addrlen);
ssize_t __myos_recvfrom(int sockfd, void *buf, size_t len, int flags,
                        struct sockaddr *src_addr, socklen_t *addrlen);
int __myos_getifaddrs(struct ifaddrs **ifap);
void __myos_freeifaddrs(struct ifaddrs *ifa);
unsigned int __myos_if_nametoindex(const char *ifname);
char *__myos_if_indextoname(unsigned int ifindex, char *ifname);
struct if_nameindex *__myos_if_nameindex(void);
void __myos_if_freenameindex(struct if_nameindex *ptr);

/* ---- evfd.c: event counters, timers, wait sets, signal fds ------------ */
int __myos_eventfd(unsigned int initval, int flags);
int __myos_eventfd_read(int fd, eventfd_t *value);
int __myos_eventfd_write(int fd, eventfd_t value);
int __myos_timerfd_create(clockid_t clockid, int flags);
int __myos_timerfd_gettime(int fd, struct itimerspec *value);
int __myos_timerfd_settime(int fd, int flags, const struct itimerspec *value, struct itimerspec *ovalue);
int __myos_epoll_create1(int flags);
int __myos_epoll_create(int size);
int __myos_epoll_ctl(int epfd, int op, int fd, struct epoll_event *event);
int __myos_epoll_wait(int epfd, struct epoll_event *events, int maxevents, int timeout);
int __myos_epoll_pwait(int epfd, struct epoll_event *events, int maxevents, int timeout,
                       const sigset_t *sigmask);
int __myos_signalfd(int fd, const sigset_t *mask, int flags);

/* ---- termios.c, ioctl.c, pty.c: terminals ----------------------------- */
int __myos_tcgetattr(int fd, struct termios *t);
int __myos_tcsetattr(int fd, int optional_actions, const struct termios *t);
int __myos_tcsendbreak(int fd, int duration);
int __myos_tcdrain(int fd);
int __myos_tcflush(int fd, int queue_selector);
int __myos_tcflow(int fd, int action);
pid_t __myos_tcgetpgrp(int fd);
int __myos_tcsetpgrp(int fd, pid_t pgrp);
speed_t __myos_cfgetispeed(const struct termios *t);
speed_t __myos_cfgetospeed(const struct termios *t);
int __myos_cfsetispeed(struct termios *t, speed_t speed);
int __myos_cfsetospeed(struct termios *t, speed_t speed);
int __myos_ioctl(int fd, unsigned long request, ...);
int __myos_openpty(int *amaster, int *aslave, char *name, const struct termios *termp,
                   const struct winsize *winp);
int __myos_forkpty(int *amaster, char *name, const struct termios *termp, const struct winsize *winp);

/* ---- pwdgrp.c: the users of the security policy ----------------------- */
uid_t __myos_getuid(void);
uid_t __myos_geteuid(void);
int __myos_issetugid(void);
char *__myos_getlogin(void);
int __myos_getlogin_r(char *buf, size_t bufsize);
struct passwd *__myos_getpwuid(uid_t uid);
struct passwd *__myos_getpwnam(const char *name);
struct group *__myos_getgrgid(gid_t gid);
struct group *__myos_getgrnam(const char *name);
void __myos_setpwent(void);
struct passwd *__myos_getpwent(void);
void __myos_endpwent(void);
void __myos_setgrent(void);
struct group *__myos_getgrent(void);
void __myos_endgrent(void);
int __myos_getpwuid_r(uid_t uid, struct passwd *pwd, char *buf, size_t buflen, struct passwd **result);
int __myos_getpwnam_r(const char *name, struct passwd *pwd, char *buf, size_t buflen,
                      struct passwd **result);
int __myos_getgrgid_r(gid_t gid, struct group *grp, char *buf, size_t buflen, struct group **result);
int __myos_getgrnam_r(const char *name, struct group *grp, char *buf, size_t buflen,
                      struct group **result);

/* ---- statvfs.c, uio.c, mount.c, reboot.c, random.c -------------------- */
int __myos_statvfs(const char *path, struct statvfs *buf);
int __myos_fstatvfs(int fd, struct statvfs *buf);
ssize_t __myos_writev(int fd, const struct iovec *iov, int iovcnt);
ssize_t __myos_readv(int fd, const struct iovec *iov, int iovcnt);
int __myos_mount(const char *source, const char *target, const char *fstype, ...);
int __myos_reboot(int howto);
/* random.c: getrandom(2) from /dev/urandom, and arc4random over it
 * (mlibc's; newlib has its own). */
ssize_t __myos_getrandom(void *buf, size_t buflen, unsigned int flags);
void __myos_arc4random_buf(void *buf, size_t n);
uint32_t __myos_arc4random(void);
uint32_t __myos_arc4random_uniform(uint32_t upper);

#ifdef __cplusplus
}
#endif

#endif /* LIBMYOS_H */
