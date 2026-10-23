// Directories: mlibc reads a directory fd's entries as Linux dirent64
// records (ReadEntries, a buffer at a time, until none is left); the
// kernel lists a directory's names at once, one per line (myos_listdirat,
// libmyos/at.c). A listing is read at the first call on an fd and handed
// out record by record; it goes with its end, a rewind (mlibc seeks the fd
// to 0 and reads again) or the fd's close. "." and ".." are left out, as
// newlib's readdir leaves them out; the type and inode of each entry come
// from its stat, as there.
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#include <bits/ensure.h>
#include <mlibc/all-sysdeps.hpp>

#include "libmyos.h"

namespace {

constexpr size_t DIRBUF = 4096;
// The largest listing read (the kernel's LISTDIR_MAX).
constexpr size_t DIRBUF_MAX = 256 * 1024;

struct Listing {
	char *buf;
	size_t len;
	size_t pos;
	long index;
};

Listing listings[MYOS_MAX_FDS];

void drop(int fd) {
	free(listings[fd].buf);
	listings[fd] = {};
}

// The names of the directory `fd`, into its listing: 0, or an errno.
int list(int fd) {
	Listing &l = listings[fd];
	size_t cap = DIRBUF;
	for (;;) {
		char *buf = (char *)malloc(cap);
		if (buf == nullptr)
			return ENOMEM;
		long n = myos_listdirat(fd, "", buf, cap, MYOS_AT_EMPTY_PATH);
		if (n < 0) {
			free(buf);
			return errno;
		}
		if ((size_t)n < cap || cap >= DIRBUF_MAX) {
			l = {buf, (size_t)n, 0, 0};
			return 0;
		}
		// Full: there may be more.
		free(buf);
		cap *= 2;
	}
}

unsigned char type_of(mode_t mode) {
	if (S_ISDIR(mode))
		return DT_DIR;
	if (S_ISREG(mode))
		return DT_REG;
	if (S_ISFIFO(mode))
		return DT_FIFO;
	if (S_ISCHR(mode))
		return DT_CHR;
	if (S_ISLNK(mode))
		return DT_LNK;
	return DT_UNKNOWN;
}

} // namespace

// libmyos/fdio.c's close drops the listing of the fd that goes.
extern "C" void __myos_dir_closed(int fd) {
	if (fd >= 0 && fd < MYOS_MAX_FDS && listings[fd].buf != nullptr)
		drop(fd);
}

namespace mlibc {

int Sysdeps<OpenDir>::operator()(const char *path, int *handle) {
	int fd = __myos_open(path, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
	if (fd < 0)
		return errno;
	*handle = fd;
	return 0;
}

int Sysdeps<ReadEntries>::operator()(int handle, void *buffer, size_t max_size, size_t *bytes_read) {
	if (handle < 0 || handle >= MYOS_MAX_FDS)
		return EBADF;
	Listing &l = listings[handle];
	if (l.buf == nullptr) {
		if (int e = list(handle); e)
			return e;
	}
	size_t out = 0;
	while (l.pos < l.len) {
		size_t start = l.pos;
		while (l.pos < l.len && l.buf[l.pos] != '\n')
			l.pos++;
		size_t n = l.pos - start;
		size_t next = l.pos < l.len ? l.pos + 1 : l.pos;
		if (n == 0 || (n <= 2 && l.buf[start] == '.' && (n == 1 || l.buf[start + 1] == '.'))) {
			l.pos = next;
			continue;
		}
		if (n >= sizeof(((struct dirent *)nullptr)->d_name))
			n = sizeof(((struct dirent *)nullptr)->d_name) - 1;
		size_t reclen = (offsetof(struct dirent, d_name) + n + 1 + 7) & ~(size_t)7;
		if (out + reclen > max_size) {
			if (out == 0)
				return EINVAL; // a buffer too small for one entry
			l.pos = start;
			break;
		}
		auto ent = (struct dirent *)((char *)buffer + out);
		memset(ent, 0, reclen);
		memcpy(ent->d_name, l.buf + start, n);
		ent->d_name[n] = '\0';
		ent->d_reclen = (unsigned short)reclen;
		ent->d_off = ++l.index;
		struct stat st;
		if (__myos_fstatat(handle, ent->d_name, &st, 0) == 0) {
			ent->d_ino = st.st_ino;
			ent->d_type = type_of(st.st_mode);
		} else {
			ent->d_type = DT_UNKNOWN;
		}
		out += reclen;
		l.pos = next;
	}
	if (out == 0)
		drop(handle);
	*bytes_read = out;
	return 0;
}

} // namespace mlibc
