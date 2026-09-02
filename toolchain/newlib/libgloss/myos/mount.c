/* myos libgloss: mount(2). */
#include <errno.h>
#include <stdint.h>
#include <stdarg.h>
#include <string.h>
#include <unistd.h>

#include "myos_syscalls.h"

struct myos_mount_args {
    unsigned long src;
    unsigned long src_len;
    unsigned long tgt;
    unsigned long tgt_len;
    unsigned long fstype;
    unsigned long fstype_len;
};

int _mount(const char *source, const char *target, const char *fstype) {
    struct myos_mount_args args;
    long ret;

    if (source == NULL || target == NULL || fstype == NULL) {
        errno = EINVAL;
        return -1;
    }
    args.src = (unsigned long)(uintptr_t)source;
    args.src_len = (unsigned long)strlen(source);
    args.tgt = (unsigned long)(uintptr_t)target;
    args.tgt_len = (unsigned long)strlen(target);
    args.fstype = (unsigned long)(uintptr_t)fstype;
    args.fstype_len = (unsigned long)strlen(fstype);
    ret = myos_syscall3(MYOS_SYS_MOUNT, (long)(uintptr_t)&args, 0, 0);
    if (ret == (long)MYOS_SYSERR) {
        errno = EINVAL;
        return -1;
    }
    return 0;
}

int mount(const char *source, const char *target, const char *fstype, ...) {
    {
        va_list ap;
        va_start(ap, fstype);
        va_end(ap);
    }
    return _mount(source, target, fstype);
}
