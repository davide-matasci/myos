/* myos_shims.c — myos libgloss gap fillers for dropbear 2026.94.
 * Compiled into every arch's link (see build.sh). Struct passwd/group come
 * from newlib headers (via myos_compat.h); only the lookup functions and a
 * few syscall wrappers are missing on myos. */
#include "myos_compat.h"

/* --- passwd/group db: myos has none; everything is root ---
 * (getuid/getgid/seteuid/setegid already live in libgloss posix_stubs.o.) */

static struct passwd root_pw = {
	.pw_name = "root",
	.pw_passwd = "",
	.pw_uid = 0,
	.pw_gid = 0,
	.pw_gecos = "root",
	.pw_dir = "/root",
	.pw_shell = "/bin/custom/sh",
};

struct passwd *getpwnam(const char *name) {
	if (name && strcmp(name, "root") == 0) {
		return &root_pw;
	}
	return NULL;
}

struct passwd *getpwuid(uid_t uid) {
	if (uid == 0) {
		return &root_pw;
	}
	return NULL;
}

void endpwent(void) {}

static struct group root_gr = {
	.gr_name = "root",
	.gr_passwd = "",
	.gr_gid = 0,
	.gr_mem = NULL,
};

struct group *getgrnam(const char *name) {
	if (name && strcmp(name, "root") == 0) {
		return &root_gr;
	}
	return NULL;
}

struct group *getgrgid(gid_t gid) {
	if (gid == 0) {
		return &root_gr;
	}
	return NULL;
}

void endgrent(void) {}

/* --- misc gaps (dropbear tolerates failure; log ENOSYS as -1) --- */
/* Valid login shells: must match root's pw_shell (/bin/custom/sh) and the
 * entries in /etc/shells, or dropbear's check_shell() rejects the login. */
static const char *const g_shells[] = { "/bin/custom/sh", "/bin/sh" };
static int g_shell_idx = 0;

char *getusershell(void) {
	if (g_shell_idx >= (int)(sizeof(g_shells) / sizeof(g_shells[0]))) {
		return NULL;
	}
	return (char *)g_shells[g_shell_idx++];
}

void endusershell(void) {
	g_shell_idx = 0;
}

void setusershell(void) {
	g_shell_idx = 0;
}

/* --- exec/process gaps (libgloss has execve only) --- */
extern char **environ;

int execv(const char *path, char *const argv[]) {
	return execve(path, argv, environ);
}

pid_t vfork(void) {
	return fork();
}

int fsync(int fd) {
	(void)fd;
	return 0;
}

int dup(int oldfd) {
	return fcntl(oldfd, F_DUPFD, 0);
}

int daemon(int nochdir, int noclose) {
	pid_t pid = fork();
	if (pid < 0) {
		return -1;
	}
	if (pid > 0) {
		_exit(0);
	}
	if (setsid() < 0) {
		return -1;
	}
	if (!nochdir) {
		chdir("/");
	}
	if (!noclose) {
		int devnull = open("/dev/null", O_RDWR);
		if (devnull >= 0) {
			dup2(devnull, 0);
			dup2(devnull, 1);
			dup2(devnull, 2);
			if (devnull > 2) {
				close(devnull);
			}
		}
	}
	return 0;
}

/* sysoptions.h gates on HAVE_SETRESGID (config-myos.h sets it) — provide
 * them via setuid/setgid since myos is single-user (uid 0). */
int setresuid(uid_t ruid, uid_t euid, uid_t suid) {
	(void)ruid;
	(void)suid;
	return setuid(euid);
}

int setresgid(gid_t rgid, gid_t egid, gid_t sgid) {
	(void)rgid;
	(void)sgid;
	return setgid(egid);
}

/* --- syslog stubs: log to stderr (DROPBEAR_SYSLOG=0 keeps them rare) --- */
void syslog(int priority, const char *format, ...) {
	(void)priority;
	(void)format;
	/* no-op: dropbear's own -E/-F stderr logging covers us */
}

void openlog(const char *ident, int logopt, int facility) {
	(void)ident;
	(void)logopt;
	(void)facility;
}

void closelog(void) {}

/* --- rlimit stubs: core-dump disabling only; myos has no cores --- */
int getrlimit(int resource, struct rlimit *rlim) {
	if (!rlim) {
		errno = EFAULT;
		return -1;
	}
	rlim->rlim_cur = RLIM_INFINITY;
	rlim->rlim_max = RLIM_INFINITY;
	(void)resource;
	return 0;
}

int setrlimit(int resource, const struct rlimit *rlim) {
	(void)resource;
	(void)rlim;
	return 0;
}

/* --- nanosleep via select (libgloss pollselect; no timer syscalls on myos) --- */
#include <sys/select.h>
#include <time.h>

int nanosleep(const struct timespec *req, struct timespec *rem) {
	struct timeval tv;
	tv.tv_sec = req ? (long)req->tv_sec : 0;
	tv.tv_usec = req ? (long)(req->tv_nsec / 1000) : 0;
	select(0, NULL, NULL, NULL, &tv);
	if (rem) {
		rem->tv_sec = 0;
		rem->tv_nsec = 0;
	}
	return 0;
}

/* --- sys/uio.h: newlib/myos has no readv/writev --- */
#include <sys/uio.h>

ssize_t readv(int fd, const struct iovec *iov, int iovcnt) {
	ssize_t total = 0;
	for (int i = 0; i < iovcnt; i++) {
		if (iov[i].iov_len == 0) {
			continue;
		}
		ssize_t n = read(fd, iov[i].iov_base, iov[i].iov_len);
		if (n < 0) {
			return total > 0 ? total : -1;
		}
		total += n;
		if ((size_t)n < iov[i].iov_len) {
			break;
		}
	}
	return total;
}


ssize_t writev(int fd, const struct iovec *iov, int iovcnt) {
	ssize_t total = 0;
	for (int i = 0; i < iovcnt; i++) {
		if (iov[i].iov_len == 0) {
			continue;
		}
		ssize_t n = write(fd, iov[i].iov_base, iov[i].iov_len);
		if (n < 0) {
			return total > 0 ? total : -1;
		}
		total += n;
		if ((size_t)n < iov[i].iov_len) {
			break;
		}
	}
	return total;
}
