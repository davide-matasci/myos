/* myos libgloss: flat root-only cwd stubs. */

#include <errno.h>
#include <string.h>
#include <unistd.h>

int
chdir(const char *path)
{
    if (path == NULL) {
        errno = EFAULT;
        return -1;
    }
    if (path[0] == '\0') {
        errno = ENOENT;
        return -1;
    }
    return 0;
}

char *
getcwd(char *buf, size_t size)
{
    if (buf == NULL || size == 0) {
        errno = ERANGE;
        return NULL;
    }
    if (size < 2) {
        errno = ERANGE;
        return NULL;
    }
    buf[0] = '/';
    buf[1] = '\0';
    return buf;
}
