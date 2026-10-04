/*
 * readv / writev for myos, in libc over read and write: no syscall of their
 * own.
 *
 * writev gathers the buffers into one and makes one write, so what a socket
 * or a pipe gets is one write, as with a real writev (a request header and
 * its data arrive together). Past GATHER_MAX it writes the buffers one by
 * one and stops at the first short write, which is still a valid writev
 * result: the count of bytes written, in order. readv makes one read into a
 * bounce buffer and scatters it.
 */
#include <errno.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>
#include <sys/uio.h>
#include <unistd.h>

#ifndef SSIZE_MAX
#define SSIZE_MAX ((ssize_t)(~(size_t)0 >> 1))
#endif

/* Largest gather/scatter done through one heap buffer. */
#define GATHER_MAX (64 * 1024)

/* The total length of `iov`, or -1 (EINVAL) for a bad count or a total
 * past SSIZE_MAX. */
static ssize_t iov_total(const struct iovec *iov, int iovcnt) {
    size_t total = 0;
    if (iovcnt < 0 || iovcnt > IOV_MAX) {
        errno = EINVAL;
        return -1;
    }
    for (int i = 0; i < iovcnt; i++) {
        if (iov[i].iov_len > (size_t)SSIZE_MAX - total) {
            errno = EINVAL;
            return -1;
        }
        total += iov[i].iov_len;
    }
    return (ssize_t)total;
}

ssize_t writev(int fd, const struct iovec *iov, int iovcnt) {
    ssize_t total = iov_total(iov, iovcnt);
    if (total < 0) {
        return -1;
    }
    if (iovcnt == 1) {
        return write(fd, iov[0].iov_base, iov[0].iov_len);
    }
    char *buf = total <= GATHER_MAX ? malloc(total ? (size_t)total : 1) : NULL;
    if (buf) {
        size_t off = 0;
        for (int i = 0; i < iovcnt; i++) {
            memcpy(buf + off, iov[i].iov_base, iov[i].iov_len);
            off += iov[i].iov_len;
        }
        ssize_t n = write(fd, buf, (size_t)total);
        free(buf);
        return n;
    }
    ssize_t done = 0;
    for (int i = 0; i < iovcnt; i++) {
        if (iov[i].iov_len == 0) {
            continue;
        }
        ssize_t n = write(fd, iov[i].iov_base, iov[i].iov_len);
        if (n < 0) {
            return done ? done : -1;
        }
        done += n;
        if ((size_t)n < iov[i].iov_len) {
            break;
        }
    }
    return done;
}

ssize_t readv(int fd, const struct iovec *iov, int iovcnt) {
    ssize_t total = iov_total(iov, iovcnt);
    if (total < 0) {
        return -1;
    }
    if (iovcnt == 1) {
        return read(fd, iov[0].iov_base, iov[0].iov_len);
    }
    /* One read: a second could block after the first returned data. */
    size_t want = total < GATHER_MAX ? (size_t)total : GATHER_MAX;
    char *buf = malloc(want ? want : 1);
    if (!buf) {
        errno = ENOMEM;
        return -1;
    }
    ssize_t n = read(fd, buf, want);
    size_t off = 0;
    for (int i = 0; i < iovcnt && n > 0 && off < (size_t)n; i++) {
        size_t take = iov[i].iov_len;
        if (take > (size_t)n - off) {
            take = (size_t)n - off;
        }
        memcpy(iov[i].iov_base, buf + off, take);
        off += take;
    }
    free(buf);
    return n;
}
