/* myos libgloss: SYS_READLINK over tmpfs (and future mounts). */

#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>

#include "myos_syscalls.h"

ssize_t
readlink(const char *path, char *buf, size_t bufsiz)
{
    size_t plen;
    long packed;
    long ret;

    if (path == NULL || buf == NULL || bufsiz == 0) {
        errno = EINVAL;
        return -1;
    }
    plen = strlen(path);
    if (plen == 0 || plen > 0xffff || bufsiz > 0xffff) {
        errno = ENAMETOOLONG;
        return -1;
    }
    packed = (long)((plen << 16) | bufsiz);
    ret = myos_syscall3(
        MYOS_SYS_READLINK,
        (long)(uintptr_t)path,
        (long)(uintptr_t)buf,
        packed);
    if (ret == (long)MYOS_SYSERR) {
        errno = ENOENT;
        return -1;
    }
    return (ssize_t)ret;
}
