/* myos-specific os-test cases (not part of upstream sortix/os-test).
 * Same contract as upstream tests: exit 0 on success, print a diagnostic and
 * exit non-zero on failure. Covers features upstream does not test: chroot(2)
 * (not POSIX) and named-FIFO behaviour beyond mkfifo's bare invocation. */
#include <sys/stat.h>
#include <sys/wait.h>

#include <err.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

/* Fresh scratch directory under $TMPDIR (default /tmp), e.g. /tmp/myos-t.<pid>. */
static inline char* make_scratch(void)
{
	const char* tmpdir = getenv("TMPDIR");
	if ( !tmpdir )
		tmpdir = "/tmp";
	char* dir = malloc(strlen(tmpdir) + 32);
	if ( !dir )
		err(1, "malloc");
	sprintf(dir, "%s/myos-t.%ld", tmpdir, (long) getpid());
	if ( mkdir(dir, 0755) < 0 && errno != EEXIST )
		err(1, "mkdir: %s", dir);
	return dir;
}

/* "<dir>/<name>" in a fresh buffer. */
static inline char* join(const char* dir, const char* name)
{
	char* path = malloc(strlen(dir) + strlen(name) + 2);
	if ( !path )
		err(1, "malloc");
	sprintf(path, "%s/%s", dir, name);
	return path;
}

/* Create an empty regular file. */
static inline void touch(const char* path)
{
	int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	if ( fd < 0 )
		err(1, "open: %s", path);
	close(fd);
}

/* Wait for `pid` and fail unless it exited with status 0. */
static inline void wait_ok(pid_t pid, const char* what)
{
	int status;
	if ( waitpid(pid, &status, 0) < 0 )
		err(1, "waitpid");
	if ( !WIFEXITED(status) || WEXITSTATUS(status) != 0 )
		errx(1, "%s: child failed (status %d)", what, status);
}
