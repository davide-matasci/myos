/* config.h — myos GNU make port (hand-tuned; no configure run).
 * Mirrors what make-4.4.1's configure would detect against
 * newlib + libgloss/myos. See ports/make/README.md. */
#ifndef MYOS_MAKE_CONFIG_H
#define MYOS_MAKE_CONFIG_H

#define PACKAGE "make"
#define PACKAGE_NAME "GNU Make"
#define PACKAGE_TARNAME "make"
#define PACKAGE_VERSION "4.4.1"
#define PACKAGE_STRING "GNU Make 4.4.1"
#define PACKAGE_BUGREPORT "bug-make@gnu.org"
#define PACKAGE_URL "https://www.gnu.org/software/make/"
#define VERSION "4.4.1"
#define MAKE_HOST "x86_64-unknown-myos"
#define PATH_SEPARATOR_CHAR ':'

/* Headers newlib provides. */
#define HAVE_DIRENT_H 1
#define HAVE_FCNTL_H 1
#define HAVE_LIMITS_H 1
#define HAVE_STDBOOL_H 1
#define HAVE_C_BOOL 1
#define HAVE_STDINT_H 1
#define HAVE_STDIO_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
#define HAVE_STRINGS_H 1
#define HAVE_SYS_PARAM_H 1
#define HAVE_SYS_STAT_H 1
#define HAVE_SYS_TIME_H 1
#define HAVE_SYS_WAIT_H 1
#define HAVE_UNISTD_H 1
#define HAVE_WCHAR_H 1
#define STDC_HEADERS 1
#define FILE_TIMESTAMP_HI_RES 0

/* Functions libgloss/myos implements. */
#define HAVE_ALLOCA 1
#define HAVE_ATEXIT 1
#define HAVE_DUP 1
#define HAVE_DUP2 1
#define HAVE_FDOPEN 1
#define HAVE_FORK 1
#define HAVE_WORKING_FORK 1
#define HAVE_GETCWD 1
#define HAVE_GETHOSTNAME 1
#define HAVE_GETTIMEOFDAY 1
#define HAVE_ISATTY 1
#define HAVE_LSTAT 1
#define HAVE_MEMPCPY 1
#define HAVE_MEMRCHR 1
#define HAVE_PIPE 1
#define HAVE_READLINK 1
#define HAVE_SETVBUF 1
#define HAVE_SIGACTION 1
#define HAVE_SIG_ATOMIC_T 1
#define HAVE_STRCASECMP 1
#define HAVE_STRDUP 1
#define HAVE_STRERROR 1
#define HAVE_STRNCASECMP 1
#define HAVE_STRNDUP 1
#define HAVE_STRTOLL 1
#define HAVE_STRUCT_DIRENT_D_TYPE 1
#define HAVE_TTYNAME 1
#define HAVE_UMASK 1
#define HAVE_UINTMAX_T 1
#define HAVE_INTMAX_T 1
#define HAVE_LONG_LONG_INT 1
#define HAVE_UNSIGNED_LONG_LONG_INT 1

#/* HAVE_UNION_WAIT/HAVE_VFORK/MAKE_CXX stay undefined (make tests #ifdef/#ifndef). */
/* Explicitly off: vfork (plain fork), posix_spawn, loadable objects,
 * jobserver (pipe+flock-based), NLS, getloadavg, sys_siglist, symlinks. */
/* HAVE_VFORK / HAVE_POSIX_SPAWN / MAKE_LOAD / MAKE_JOBSERVER / ENABLE_NLS stay undefined */
#define HAVE_DECL_DLOPEN 0
#define HAVE_DECL_DLSYM 0
#define HAVE_DECL_DLERROR 0
#define HAVE_DECL_SYS_SIGLIST 0
#define HAVE_DECL__SYS_SIGLIST 0
#define HAVE_DECL___SYS_SIGLIST 0
#define HAVE_DECL_GETLOADAVG 0
#define HAVE_GETGROUPS 0
#define HAVE_GETRLIMIT 0
#define HAVE_SETRLIMIT 0
#define HAVE_SETEGID 0
#define HAVE_SETEUID 0
#define HAVE_SETREGID 0
#define HAVE_SETREUID 0
#define HAVE_SETLINEBUF 0
#define HAVE_MKFIFO 0
#define HAVE_MKSTEMP 0
#define HAVE_MKTEMP 0
#define HAVE_REALPATH 0
#define HAVE_STRSIGNAL 0

/* newlib struct stat uses st_mtim (timespec). */
#define HAVE_STRUCT_STAT_ST_MTIM_TV_NSEC 1

#endif /* MYOS_MAKE_CONFIG_H */
#define SCCS_GET "get"
#define LOCALEDIR "/lib/locale"
#define LIBDIR "/lib"
