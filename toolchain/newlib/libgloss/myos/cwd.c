/* myos libgloss: chdir/getcwd via kernel per-task cwd. */

#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

#include "myos_syscalls.h"

int
chdir(const char *path)
{
	size_t n;

	if (path == NULL) {
		errno = EFAULT;
		return -1;
	}
	n = strlen(path);
	if (n == 0) {
		errno = ENOENT;
		return -1;
	}
	if (n > 64) {
		errno = ENAMETOOLONG;
		return -1;
	}
	if (myos_syscall3(MYOS_SYS_CHDIR, (long)(uintptr_t)path, (long)n, 0)
	    == (long)MYOS_SYSERR) {
		errno = ENOENT;
		return -1;
	}
	return 0;
}

char *
getcwd(char *buf, size_t size)
{
	long n;

	if (buf == NULL || size == 0) {
		errno = EINVAL;
		return NULL;
	}
	n = myos_syscall3(
	    MYOS_SYS_GETCWD, (long)(uintptr_t)buf, (long)size, 0);
	if (n == (long)MYOS_SYSERR) {
		errno = ERANGE;
		return NULL;
	}
	return buf;
}
