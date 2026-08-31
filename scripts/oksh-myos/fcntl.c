/* Override newlib's ENOSYS fcntl() (must be a .o on the link line, not an archive). */
#include <fcntl.h>
#include <stdarg.h>

int _fcntl(int fd, int cmd, int arg);

int
fcntl(int fd, int cmd, ...)
{
    va_list ap;
    int arg = 0;

    switch (cmd) {
    case F_GETFL:
    case F_GETFD:
        break;
    default:
        va_start(ap, cmd);
        arg = va_arg(ap, int);
        va_end(ap);
        break;
    }
    return _fcntl(fd, cmd, arg);
}
