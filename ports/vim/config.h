/* auto/config.h — myos FEAT_TINY freestanding port.
 *
 * Choice: hand-written minimal config (not host `./configure`). Host configure
 * probes Linux/glibc/ncurses and enables TERMINFO/TGETENT/select/sysinfo/etc.
 * that myos newlib+libgloss do not provide. We keep FEAT_TINY + UNIX and only
 * the HAVE_* flags that match available newlib/libgloss + ncurses symbols.
 * HAVE_TGETENT uses ports/ncurses (static lib); no TERMINFO database.
 */
#ifndef MYOS_VIM_CONFIG_H_
#define MYOS_VIM_CONFIG_H_

/* #undef EBCDIC */
/* #undef HAVE_X11 */
/* #undef HAVE_WAYLAND */
/* #undef FEAT_WAYLAND_CLIPBOARD_FS */

/* ncurses termcap API (ports/ncurses); no full TERMINFO DB. */
/* #undef TERMINFO */
#define HAVE_OSPEED 1
#define OSPEED_EXTERN 1
#define HAVE_UP_BC_PC 1
#define UP_BC_PC_EXTERN 1
/* #undef HAVE_OUTFUNTYPE */
/* #undef HAVE_DEL_CURTERM */
#define HAVE_TGETENT 1
#define HAVE_TERMCAP_H 1

#define HAVE_DATE_TIME 1
#define HAVE_ATTRIBUTE_UNUSED 1
#define UNIX 1

#define VIM_SIZEOF_INT 4
#define VIM_SIZEOF_LONG 8
#define SIZEOF_OFF_T 8
#define SIZEOF_TIME_T 8

/* #undef SMALL_WCHAR_T */

#define USEMEMMOVE 1

/* #undef USEMAN_S */

/* PTY / signals — keep conservative. */
/* #undef HAVE_SVR4_PTYS */
/* #undef HAVE_DEV_PTC */
/* #undef HAVE_SIGCONTEXT */
/* #undef HAVE_SIGSETJMP */

#define HAVE_GETCWD 1
/* #undef HAVE_FCHDIR */
/* #undef HAVE_FCHOWN */
/* #undef HAVE_FCHMOD */
/* #undef HAVE_FSEEKO */
/* #undef HAVE_FSYNC */
/* #undef HAVE_FTRUNCATE */
/* #undef HAVE_GETPGID */
/* #undef HAVE_GETPSEUDOTTY */
/* #undef HAVE_GETPWENT */
/* #undef HAVE_GETPWNAM */
/* #undef HAVE_GETPWUID */
/* #undef HAVE_GETRLIMIT */
#define HAVE_GETTIMEOFDAY 1
/* #undef HAVE_GETWD */
/* #undef HAVE_ICONV */
/* #undef HAVE_INET_NTOP */
/* #undef HAVE_LOCALTIME_R */
/* #undef HAVE_LSTAT */
#define HAVE_MEMSET 1
/* #undef HAVE_MKDTEMP */
/* #undef HAVE_NANOSLEEP */
/* #undef HAVE_NL_LANGINFO_CODESET */
#define HAVE_OPENDIR 1
/* #undef HAVE_POSIX_OPENPT */
#define HAVE_PUTENV 1
#define HAVE_QSORT 1
/* #undef HAVE_READLINK */
#define HAVE_RENAME 1

/* select: stubbed in myos_stubs.c */
#define HAVE_SELECT 1
#define SYS_SELECT_WITH_SYS_TIME 1
#define SELECT_TYPE_ARG234 (fd_set *)
#define TIME_WITH_SYS_TIME 1
/* #undef HAVE_SELINUX */
#define HAVE_SETENV 1
/* #undef HAVE_SETPGID */
/* #undef HAVE_SETSID */
/* #undef HAVE_SIGACTION — provided via libgloss misc_stubs; enable */
#define HAVE_SIGACTION 1
/* #undef HAVE_SIGALTSTACK */
/* #undef HAVE_SIGSET */
/* #undef HAVE_SIGSTACK */
/* #undef HAVE_SIGPROCMASK */
/* #undef HAVE_SIGVEC */
#define HAVE_STRCASECMP 1
/* #undef HAVE_STRCOLL */
#define HAVE_STRERROR 1
/* #undef HAVE_STRFTIME */
#define HAVE_STRNCASECMP 1
#define HAVE_STRPBRK 1
/* #undef HAVE_STRPTIME */
#define HAVE_STRTOL 1
/* #undef HAVE_CANBERRA */
/* #undef HAVE_SODIUM */
/* #undef HAVE_ST_BLKSIZE */
/* #undef HAVE_SYNC */
/* #undef HAVE_SYSCONF */
/* #undef HAVE_SYSCTL */
/* #undef HAVE_SYSINFO */
/* #undef HAVE_SYSINFO_MEM_UNIT */
/* #undef HAVE_SYSINFO_UPTIME */
/* #undef HAVE_TOWLOWER */
/* #undef HAVE_TOWUPPER */
/* #undef HAVE_ISWUPPER */
/* #undef HAVE_TZSET */
#define HAVE_UNSETENV 1
/* #undef HAVE_USLEEP */
/* #undef HAVE_UTIME */
/* #undef HAVE_MBLEN */
/* #undef HAVE_TIMER_CREATE */
#define HAVE_CLOCK_GETTIME 1
/* #undef HAVE_XATTR */
/* #undef HAVE_UTIMES */

#define HAVE_DIRENT_H 1
#define HAVE_ERRNO_H 1
#define HAVE_FCNTL_H 1
/* #undef HAVE_ICONV_H */
#define HAVE_INTTYPES_H 1
/* #undef HAVE_LANGINFO_H */
/* #undef HAVE_LIBGEN_H */
/* #undef HAVE_LIBINTL_H */
#define HAVE_LOCALE_H 1
#define HAVE_MATH_H 1
/* #undef HAVE_POLL_H */
#define HAVE_PWD_H 1
#define HAVE_SETJMP_H 1
/* #undef HAVE_SGTTY_H */
#define HAVE_STDINT_H 1
#define HAVE_STRINGS_H 1
#define HAVE_SYS_IOCTL_H 1
#define HAVE_SYS_PARAM_H 1
/* #undef HAVE_SYS_POLL_H */
/* #undef HAVE_SYS_RESOURCE_H */
#define HAVE_SYS_SELECT_H 1
/* #undef HAVE_SYS_STATFS_H */
/* #undef HAVE_SYS_SYSINFO_H */
#define HAVE_SYS_TIME_H 1
#define HAVE_SYS_TYPES_H 1
/* #undef HAVE_SYS_UTSNAME_H */
#define HAVE_TERMIOS_H 1
/* #undef HAVE_TERMIO_H */
/* #undef HAVE_WCHAR_H */
/* #undef HAVE_WCTYPE_H */
#define HAVE_UNISTD_H 1
/* #undef HAVE_UTIME_H */

#define HAVE_SYS_WAIT_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1

#define FEAT_TINY 1
/* #undef FEAT_NORMAL */
/* #undef FEAT_HUGE */

/* #undef FEAT_LUA */
/* #undef FEAT_MZSCHEME */
/* #undef FEAT_PERL */
/* #undef FEAT_PYTHON */
/* #undef FEAT_PYTHON3 */
/* #undef FEAT_RUBY */
/* #undef FEAT_TCL */

/* #undef HAVE_POSIX_ACL */
/* #undef HAVE_GPM */
/* #undef HAVE_SYSMOUSE */

/* #undef ENABLE_CSCOPE */

/* #undef FEAT_AUTOSERVERNAME */
/* #undef WANT_SOCKETSERVER */
/* #undef FEAT_XFONTSET */
/* #undef FEAT_XIM */

/* #undef HAVE_DLFCN_H */
/* #undef HAVE_GETTEXT */
/* #undef HAVE_DLOPEN */
/* #undef HAVE_DLSYM */

/* #undef FEAT_IPV6 */
/* #undef FEAT_NETBEANS_INTG */
/* #undef FEAT_JOB_CHANNEL */
/* #undef FEAT_TERMINAL */

/* #undef USE_XSMP_INTERACT */
/* #undef HAVE_FD_CLOEXEC */
/* #undef PROC_EXE_LINK */

#define HAVE_ISINF 1
#define HAVE_ISNAN 1
/* #undef HAVE_DIRFD */
/* #undef HAVE_FLOCK */
/* #undef HAVE_SHM_OPEN */
/* #undef HAVE_SYSCONF_SIGSTKSZ */

#endif /* MYOS_VIM_CONFIG_H_ */
