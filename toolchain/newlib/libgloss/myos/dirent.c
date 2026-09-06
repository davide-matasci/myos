/* myos libgloss: opendir/readdir over flat bootfs (SYS_LISTDIR). */

#include <dirent.h>
#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <sys/stat.h>

#include "myos_syscalls.h"

#define MYOS_DIR_POOL 16

static DIR dir_pool[MYOS_DIR_POOL];
static unsigned char dir_used[MYOS_DIR_POOL];

static int
myos_path_is_root(const char *path)
{
	if (path == NULL || path[0] == '\0') {
		return 1;
	}
	/* "." is cwd-relative; only absolute root skips pre-stat. */
	if (strcmp(path, "/") == 0) {
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

static int
dir_slot_index(DIR *d)
{
	if (d == NULL) {
		return -1;
	}
	if (d < &dir_pool[0] || d >= &dir_pool[MYOS_DIR_POOL]) {
		return -1;
	}
	return (int)(d - &dir_pool[0]);
}

DIR *
opendir(const char *name)
{
	struct stat st;
	int i;

	for (i = 0; i < MYOS_DIR_POOL; i++) {
		if (!dir_used[i]) {
			break;
		}
	}
	if (i >= MYOS_DIR_POOL) {
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

	memset(&dir_pool[i], 0, sizeof(dir_pool[i]));
	dir_pool[i].len = (unsigned long)myos_syscall3(
	    MYOS_SYS_LISTDIR,
	    (long)(uintptr_t)name,
	    (long)strlen(name),
	    (long)(uintptr_t)dir_pool[i].buf);
	if (dir_pool[i].len == (unsigned long)MYOS_SYSERR) {
		errno = EIO;
		return NULL;
	}
	dir_pool[i].pos = 0;
	dir_used[i] = 1;
	return &dir_pool[i];
}

struct dirent *
readdir(DIR *d)
{
	int slot = dir_slot_index(d);

	if (slot < 0 || !dir_used[slot]) {
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
		/* Skip only "." and ".." — not every name starting with '.'. */
		if (d->ent.d_name[0] == '.'
		    && (d->ent.d_name[1] == '\0'
			|| (d->ent.d_name[1] == '.' && d->ent.d_name[2] == '\0'))) {
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
	int slot = dir_slot_index(d);

	if (slot < 0 || !dir_used[slot]) {
		errno = EBADF;
		return -1;
	}
	dir_used[slot] = 0;
	return 0;
}
