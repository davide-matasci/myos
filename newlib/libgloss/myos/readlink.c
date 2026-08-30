/* myos libgloss: no symlinks on flat bootfs yet. */

#include <errno.h>
#include <unistd.h>

ssize_t
readlink(const char *path, char *buf, size_t bufsiz)
{
    (void)path;
    (void)buf;
    (void)bufsiz;
    errno = ENOSYS;
    return -1;
}
