/* myos libgloss: opendir/readdir over flat bootfs (SYS_LISTDIR). */

#include <dirent.h>
#include <errno.h>
#include <string.h>
#include <sys/stat.h>

#include "myos_syscalls.h"

static DIR the_dir;
static int dir_open;

static int
myos_path_is_root(const char *path)
{
	if (path == NULL || path[0] == '\0') {
		return 1;
	}
	if (strcmp(path, ".") == 0 || strcmp(path, "/") == 0) {
		return 1;
	}
	while (*path == '/') {
		path++;
	}
	if (path[0] == '\0') {
		return 1;
	}
	return 0;
}

DIR *
opendir(const char *name)
{
	struct stat st;

	if (dir_open) {
		errno = EMFILE;
		return NULL;
	}
	if (!myos_path_is_root(name)) {
		if (stat(name, &st) < 0) {
			return NULL;
		}
		if (!S_ISDIR(st.st_mode)) {
			errno = ENOTDIR;
			return NULL;
		}
	}

	memset(&the_dir, 0, sizeof(the_dir));
	the_dir.len = (unsigned long)myos_syscall3(
	    MYOS_SYS_LISTDIR,
	    (long)(uintptr_t)name,
	    (long)strlen(name),
	    (long)(uintptr_t)the_dir.buf);
	if (the_dir.len == (unsigned long)MYOS_SYSERR) {
		errno = EIO;
		return NULL;
	}
	the_dir.pos = 0;
	dir_open = 1;
	return &the_dir;
}

struct dirent *
readdir(DIR *d)
{
	if (d == NULL || d != &the_dir || !dir_open) {
		errno = EBADF;
		return NULL;
	}
	while (d->pos < d->len) {
		unsigned long start = d->pos;
		while (d->pos < d->len && d->buf[d->pos] != '\n') {
			d->pos++;
		}
		unsigned long n = d->pos - start;
		if (d->pos < d->len) {
			d->pos++;
		}
		if (n == 0) {
			continue;
		}
		if (n >= sizeof(d->ent.d_name)) {
			n = sizeof(d->ent.d_name) - 1;
		}
		memcpy(d->ent.d_name, d->buf + start, n);
		d->ent.d_name[n] = '\0';
		if (d->ent.d_name[0] == '.') {
			continue;
		}
		d->ent.d_ino = 0;
		d->ent.d_type = DT_UNKNOWN;
		return &d->ent;
	}
	return NULL;
}

int
closedir(DIR *d)
{
	if (d == NULL || d != &the_dir || !dir_open) {
		errno = EBADF;
		return -1;
	}
	dir_open = 0;
	return 0;
}
