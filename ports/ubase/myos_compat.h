/* Extra POSIX/BSD declarations ubase getty/login need on newlib/myos. */
#ifndef _MYOS_UBASE_COMPAT_H_
#define _MYOS_UBASE_COMPAT_H_

#include <stddef.h>
#include <sys/types.h>
#include <unistd.h>

#ifndef LOGIN_NAME_MAX
#define LOGIN_NAME_MAX 32
#endif
#ifndef HOST_NAME_MAX
#define HOST_NAME_MAX 64
#endif

int vhangup(void);
int fchmod(int fd, mode_t mode);
int gethostname(char *name, size_t len);
int initgroups(const char *user, gid_t group);
char *getpass(const char *prompt);
int clearenv(void);

#endif /* _MYOS_UBASE_COMPAT_H_ */
