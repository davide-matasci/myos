/* myos libgloss: opendir/readdir over flat bootfs (SYS_LISTDIR). */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "myos_syscalls.h"

#define MYOS_DIR_POOL 16

static DIR dir_pool[MYOS_DIR_POOL];
static unsigned char dir_used[MYOS_DIR_POOL];
/* dirfd(): lazily opened fd per DIR (0 = none yet; stored as fd + 1). */
static int dir_fd[MYOS_DIR_POOL];

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

/*
 * Fill d_ino (and d_type when known) from stat(2) so tools that trust
 * dirent.d_ino see the same inode as lstat of the joined path.
 */
static void
myos_fill_dirent_meta(DIR *d, unsigned long namelen)
{
	char full[512];
	struct stat st;
	size_t plen;
	int need_slash;

	d->ent.d_ino = 0;
	d->ent.d_type = DT_UNKNOWN;

	plen = strlen(d->path);
	while (plen > 1 && d->path[plen - 1] == '/') {
		plen--;
	}
	need_slash = !(plen == 0 || (plen == 1 && d->path[0] == '/'));
	if (plen + (need_slash ? 1u : 0u) + namelen + 1u > sizeof(full)) {
		return;
	}
	memcpy(full, d->path, plen);
	if (need_slash) {
		full[plen++] = '/';
	}
	memcpy(full + plen, d->ent.d_name, namelen);
	full[plen + namelen] = '\0';

	if (stat(full, &st) != 0) {
		return;
	}
	d->ent.d_ino = st.st_ino;
	if (S_ISDIR(st.st_mode)) {
		d->ent.d_type = DT_DIR;
	} else if (S_ISREG(st.st_mode)) {
		d->ent.d_type = DT_REG;
	} else if (S_ISFIFO(st.st_mode)) {
		d->ent.d_type = DT_FIFO;
	}
}

DIR *
opendir(const char *name)
{
	struct stat st;
	int i;
	size_t n;

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
	if (name != NULL) {
		n = strlen(name);
		if (n >= sizeof(dir_pool[i].path)) {
			n = sizeof(dir_pool[i].path) - 1;
		}
		memcpy(dir_pool[i].path, name, n);
		dir_pool[i].path[n] = '\0';
	}
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
	dir_fd[i] = 0;
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
		myos_fill_dirent_meta(d, n);
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
	if (dir_fd[slot] > 0) {
		close(dir_fd[slot] - 1);
		dir_fd[slot] = 0;
	}
	dir_used[slot] = 0;
	return 0;
}

/* Re-read the listing so entries created since opendir() show up. */
void
rewinddir(DIR *d)
{
	int slot = dir_slot_index(d);
	unsigned long len;

	if (slot < 0 || !dir_used[slot]) {
		return;
	}
	len = (unsigned long)myos_syscall3(MYOS_SYS_LISTDIR,
	    (long)(uintptr_t)d->path, (long)strlen(d->path),
	    (long)(uintptr_t)d->buf);
	d->len = len == (unsigned long)MYOS_SYSERR ? 0 : len;
	d->pos = 0;
}

/* Positions are byte offsets into the cached listing. */
long
telldir(DIR *d)
{
	int slot = dir_slot_index(d);

	if (slot < 0 || !dir_used[slot]) {
		errno = EBADF;
		return -1;
	}
	return (long)d->pos;
}

void
seekdir(DIR *d, long loc)
{
	int slot = dir_slot_index(d);

	if (slot < 0 || !dir_used[slot] || loc < 0) {
		return;
	}
	d->pos = (unsigned long)loc > d->len ? d->len : (unsigned long)loc;
}

int
dirfd(DIR *d)
{
	int slot = dir_slot_index(d);
	int fd;

	if (slot < 0 || !dir_used[slot]) {
		errno = EINVAL;
		return -1;
	}
	if (dir_fd[slot] == 0) {
		fd = open(d->path[0] ? d->path : ".", O_RDONLY);
		if (fd < 0) {
			return -1;
		}
		dir_fd[slot] = fd + 1;
	}
	return dir_fd[slot] - 1;
}

int
alphasort(const struct dirent **a, const struct dirent **b)
{
	return strcoll((*a)->d_name, (*b)->d_name);
}

int
scandir(const char *path, struct dirent ***namelist,
    int (*select)(const struct dirent *),
    int (*compar)(const struct dirent **, const struct dirent **))
{
	DIR *d = opendir(path);
	struct dirent **list = NULL;
	struct dirent *e;
	size_t n = 0;
	size_t cap = 0;

	if (d == NULL) {
		return -1;
	}
	while ((e = readdir(d)) != NULL) {
		struct dirent *copy;
		if (select != NULL && !select(e)) {
			continue;
		}
		if (n == cap) {
			size_t ncap = cap ? cap * 2 : 16;
			struct dirent **grown = realloc(list, ncap * sizeof(*list));
			if (grown == NULL) {
				goto fail;
			}
			list = grown;
			cap = ncap;
		}
		copy = malloc(sizeof(*copy));
		if (copy == NULL) {
			goto fail;
		}
		memcpy(copy, e, sizeof(*copy));
		list[n++] = copy;
	}
	closedir(d);
	if (compar != NULL && n > 1) {
		qsort(list, n, sizeof(*list),
		    (int (*)(const void *, const void *))compar);
	}
	*namelist = list;
	return (int)n;
fail:
	while (n > 0) {
		free(list[--n]);
	}
	free(list);
	closedir(d);
	errno = ENOMEM;
	return -1;
}
