/* myos libgloss: opendir/readdir over the kernel's directory listings
 * (myos_listdirat, at.c): a directory is opened as an fd, its names read
 * at once and handed out one by one. */

#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include "myos_syscalls.h"

/* newlib declares it for _GNU_SOURCE only. */
#ifndef AT_EMPTY_PATH
#define AT_EMPTY_PATH 0x0010
#endif

#define MYOS_DIR_POOL 16
/* The largest listing read (the kernel's LISTDIR_MAX). */
#define MYOS_DIR_MAX (256 * 1024)

static DIR dir_pool[MYOS_DIR_POOL];
static unsigned char dir_used[MYOS_DIR_POOL];

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

/* Read the directory's names into d->buf, growing it until they fit. */
static int
dir_list(DIR *d)
{
	for (;;) {
		long n;
		if (d->buf == NULL) {
			d->cap = MYOS_DIRBUF;
			d->buf = malloc(d->cap);
			if (d->buf == NULL) {
				errno = ENOMEM;
				return -1;
			}
		}
		n = d->fd >= 0 ? myos_listdirat(d->fd, "", d->buf, d->cap, AT_EMPTY_PATH)
			       : myos_listdirat(AT_FDCWD, d->path, d->buf, d->cap, 0);
		if (n < 0) {
			return -1;
		}
		if ((unsigned long)n < d->cap || d->cap >= MYOS_DIR_MAX) {
			d->len = (unsigned long)n;
			d->pos = 0;
			return 0;
		}
		/* Full: there may be more. */
		free(d->buf);
		d->buf = NULL;
		d->cap *= 2;
		d->buf = malloc(d->cap);
		if (d->buf == NULL) {
			errno = ENOMEM;
			return -1;
		}
	}
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

	if (d->fd >= 0) {
		if (fstatat(d->fd, d->ent.d_name, &st, 0) != 0) {
			return;
		}
	} else {
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

/* A DIR on `fd` (owned from here on), or on `path` when there is no fd. */
static DIR *
dir_new(int fd, const char *path)
{
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
	memset(&dir_pool[i], 0, sizeof(dir_pool[i]));
	dir_pool[i].fd = fd;
	n = strlen(path);
	if (n >= sizeof(dir_pool[i].path)) {
		n = sizeof(dir_pool[i].path) - 1;
	}
	memcpy(dir_pool[i].path, path, n);
	dir_pool[i].path[n] = '\0';
	if (dir_list(&dir_pool[i]) != 0) {
		free(dir_pool[i].buf);
		return NULL;
	}
	dir_used[i] = 1;
	return &dir_pool[i];
}

DIR *
opendir(const char *name)
{
	struct stat st;
	DIR *d;
	int fd;

	if (name == NULL || name[0] == '\0') {
		errno = ENOENT;
		return NULL;
	}
	/* Checked through the fd, so it is the file listed whatever is renamed
	 * meanwhile (O_NONBLOCK: a FIFO does not wait for a writer). A
	 * directory a namespace makes up has no fd: checked and listed by its
	 * path. */
	fd = open(name, O_RDONLY | O_NONBLOCK);
	if ((fd >= 0 ? fstat(fd, &st) : stat(name, &st)) < 0) {
		if (fd >= 0) {
			close(fd);
		}
		return NULL;
	}
	if (!S_ISDIR(st.st_mode)) {
		if (fd >= 0) {
			close(fd);
		}
		errno = ENOTDIR;
		return NULL;
	}
	d = dir_new(fd, name);
	if (d == NULL && fd >= 0) {
		close(fd);
	}
	return d;
}

DIR *
fdopendir(int fd)
{
	struct stat st;

	if (fstat(fd, &st) < 0) {
		return NULL;
	}
	if (!S_ISDIR(st.st_mode)) {
		errno = ENOTDIR;
		return NULL;
	}
	return dir_new(fd, "");
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
	if (d->fd >= 0) {
		close(d->fd);
	}
	free(d->buf);
	d->buf = NULL;
	dir_used[slot] = 0;
	return 0;
}

/* Re-read the listing so entries created since opendir() show up. */
void
rewinddir(DIR *d)
{
	int slot = dir_slot_index(d);

	if (slot < 0 || !dir_used[slot]) {
		return;
	}
	if (dir_list(d) != 0) {
		d->len = 0;
		d->pos = 0;
	}
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

	if (slot < 0 || !dir_used[slot]) {
		errno = EINVAL;
		return -1;
	}
	if (d->fd < 0) {
		errno = ENOTSUP; /* a directory a namespace makes up */
	}
	return d->fd;
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
