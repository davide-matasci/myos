#ifndef _MYOS_UTMP_H_
#define _MYOS_UTMP_H_

#include <sys/types.h>

#ifndef UTMP_PATH
#define UTMP_PATH "/var/run/utmp"
#endif

#define EMPTY 0
#define USER_PROCESS 7
#define LOGIN_PROCESS 6

struct utmp {
    short ut_type;
    pid_t ut_pid;
    char ut_line[32];
    char ut_id[4];
    char ut_user[32];
    char ut_host[256];
    struct {
        int tv_sec;
        int tv_usec;
    } ut_tv;
};

#endif /* _MYOS_UTMP_H_ */
