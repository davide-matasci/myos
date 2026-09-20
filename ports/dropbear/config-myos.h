/* myos config.h for dropbear 2026.94 — replaces the configure-generated one.
 * Only HAVE_* feature probes configure would set for a Linux-ish target;
 * myos-specific tweaks live in localoptions.h / myos_compat.h. */
#ifndef DROPBEAR_MYOS_CONFIG_H_
#define DROPBEAR_MYOS_CONFIG_H_

/* Enable localoptions.h inclusion (configure normally defines this). */
#define LOCALOPTIONS_H_EXISTS 1

/* setresuid/setresgid provided by ports/dropbear/myos_shims.c. */
#define HAVE_SETRESUID 1
#define HAVE_SETRESGID 1

/* newlib's socket/netdb headers provide these structs; without these flags
 * fake-rfc2553.h redefines them. */
#define HAVE_STRUCT_SOCKADDR_STORAGE 1
#define HAVE_STRUCT_SOCKADDR_IN6 1
#define HAVE_STRUCT_IN6_ADDR 1
#define HAVE_STRUCT_ADDRINFO 1
#define HAVE_STRUCT_SOCKADDR_STORAGE_SS_FAMILY 1

#define HAVE_ARPA_INET_H 1
#define HAVE_FCNTL_H 1
#define HAVE_LIMITS_H 1
#define HAVE_NETDB_H 1
#define HAVE_NETINET_IN_H 1
#define HAVE_STDINT_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
/* NOTE: features myos lacks are OMITTED entirely (never defined to 0) —
 * dropbear tests them with #ifdef. */
#define HAVE_SYS_PARAM_H 1
#define HAVE_SYS_RESOURCE_H 1
#define HAVE_SYS_SOCKET_H 1
#define HAVE_SYS_STAT_H 1
#define HAVE_SYS_TYPES_H 1
#define HAVE_SYS_UIO_H 1
#define HAVE_SYS_UN_H 1
#define HAVE_SYS_WAIT_H 1
#define HAVE_TERMIOS_H 1
#define HAVE_UNISTD_H 1
/* NOTE: features myos lacks are OMITTED entirely (never defined to 0) —
 * dropbear tests them with #ifdef. */
#define HAVE_CRYPT 0
#define HAVE_STRTOL 1
#define HAVE_STRTOUL 1
#define HAVE_STRDUP 1
#define HAVE_DAEMON 0
#define HAVE_SETSID 1
#define HAVE_SETENV 1
#define HAVE_UNSETENV 1
#define HAVE_VASPRINTF 0
#define HAVE_SOCKETPAIR 1
#define HAVE_GETUSERSHELL 0
#define HAVE_GETADDRINFO 1
#define HAVE_GETNAMEINFO 0
#define HAVE_ENDPWENT 1
#define HAVE_FSYNC 1

#endif /* DROPBEAR_MYOS_CONFIG_H_ */
