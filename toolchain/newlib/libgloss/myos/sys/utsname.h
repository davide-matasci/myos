#ifndef _MYOS_SYS_UTSNAME_H_
#define _MYOS_SYS_UTSNAME_H_

struct utsname {
    char sysname[32];
    char nodename[32];
    char release[32];
    char version[32];
    char machine[32];
};

#endif /* _MYOS_SYS_UTSNAME_H_ */
