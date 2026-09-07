/* lynx_cfg.h — hand-written myos freestanding config (not host ./configure).
 *
 * Host configure probes Linux/glibc and cannot cross-link *-unknown-myos.
 * Follow ports/vim + ports/curl: only HAVE_* that match newlib/libgloss +
 * ports/ncurses + ports/mbedtls (via tidy_tls OpenSSL-compat shim).
 *
 * SSL: USE_SSL + USE_GNUTLS_INCL selects lynx's tidy_tls polyfill path.
 * ports/lynx/tidy_tls.{h,c} implement that API over mbedtls (same stack as
 * curl / user/tls) — not a second TLS library, not OpenSSL.
 */
#ifndef LYNX_CFG_H
#define LYNX_CFG_H 1

#define SYSTEM_NAME "myos"
#define UNIX 1
#define REAL_UNIX_SYSTEM 1

#define STDC_HEADERS 1
#define HAVE_STDARG_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
#define HAVE_STRINGS_H 1
#define HAVE_UNISTD_H 1
#define HAVE_FCNTL_H 1
#define HAVE_SYS_TYPES_H 1
#define HAVE_SYS_STAT_H 1
#define HAVE_SYS_TIME_H 1
#define HAVE_SYS_IOCTL_H 1
#define HAVE_TIME_H 1
#define HAVE_ERRNO_H 1
#define HAVE_LIMITS_H 1
#define HAVE_INTTYPES_H 1
#define HAVE_STDINT_H 1
#define HAVE_MEMORY_H 1
#define HAVE_DIRENT_H 1
#define HAVE_TERMIOS_H 1
#define HAVE_ARPA_INET_H 1
#define HAVE_NETINET_IN_H 1
#define HAVE_NETDB_H 1
#define HAVE_SYS_SOCKET_H 1
#define HAVE_SYS_SELECT_H 1
#define TIME_WITH_SYS_TIME 1

#define SIZEOF_INT 4
#define SIZEOF_LONG 8
#define SIZEOF_OFF_T 8
#define SIZEOF_SIZE_T 8
#define SIZEOF_TIME_T 8
#define HAVE_LONG_LONG 1

#define HAVE_GETCWD 1
#define HAVE_GETTIMEOFDAY 1
#define HAVE_MKTIME 1
#define DONT_HAVE_TM_GMTOFF 1
#define HAVE_PUTENV 1
#define HAVE_SETENV 1
#define HAVE_STRERROR 1
#define HAVE_SLEEP 1
#define HAVE_LSTAT 1
#define HAVE_READDIR 1
/* #undef HAVE_TRUNCATE */
#define HAVE_SIGACTION 1
#define HAVE_GETADDRINFO 1
#define HAVE_INET_ATON 1
#define CAN_SET_ERRNO 1

/* ncurses (ports/ncurses static lib) */
#define NCURSES 1
#define HAVE_NCURSES_H 1
#define HAVE_TERM_H 1
#define COLOR_CURSES 1
#define FANCY_CURSES 1
#define HAVE_KEYPAD 1
#define HAVE_CBREAK 1
#define HAVE_TYPE_CHTYPE 1
#define HAVE_GETBKGD 1
#define HAVE_NEWPAD 1
#define HAVE_PNOUTREFRESH 1
#define HAVE_TOUCHLINE 1
#define HAVE_USE_LEGACY_CODING 1
#define HAVE_TOUCHWIN 1
#define HAVE_WREDRAWLN 1
/* ncurses touchline is 3-arg, not BSD 4-arg */
/* #undef HAVE_BSD_TOUCHLINE */

#define USE_FCNTL 1

/* TLS via tidy_tls → mbedtls (ports/lynx/tidy_tls.c) */
#define USE_SSL 1
#define USE_GNUTLS_INCL 1
#define MYOS_MBEDTLS_TIDY_TLS 1
/* Prefer CA path already shipped for curl */
#define SSL_CERT_FILE_DEFAULT "/lib/cacert.pem"

/* Paths in the initramfs image */
#define LYNX_CFG_FILE "/etc/lynx.cfg"
#define LYNX_CFG_PATH "/etc"
#define LYNX_LSS_FILE "/etc/lynx.lss"

/* Trim protocols / features myos does not need for a first port */
#define DISABLE_NEWS 1
#define DISABLE_FINGER 1
#define DISABLE_GOPHER 1
#define DISABLE_BIBP 1
#define NO_CONFIG_INFO 1
#define NOUSERS 1
#define NO_CUSERID 1
#define NO_UTMP 1
#define NOSIGHUP 1
#define IGNORE_CTRL_C 1

/* Keep HTTP + FTP + local files; no dired / externals / NLS / zlib */
/* #undef DIRED_SUPPORT */
/* #undef USE_EXTERNALS */
/* #undef ENABLE_NLS */
/* #undef USE_ZLIB */
/* #undef USE_COLOR_STYLE */
/* #undef USE_PRETTYSRC */
#define USE_PERSISTENT_COOKIES 1
#define USE_FILE_UPLOAD 1
#define DISP_PARTIAL 1
#define USE_READPROGRESS 1
#define USE_SOURCE_CACHE 1
#define LONG_LIST 1
#define UNDERLINE_LINKS 0

#define ANSI_VARARGS 1
#define GCC_UNUSED __attribute__((unused))
#define GCC_NORETURN __attribute__((noreturn))
#define GCC_PRINTF 1

#define lynx_rand rand
#define lynx_srand srand
#define LYNX_RAND_MAX RAND_MAX

#define SOCKADDR_LEN_INET sin_len
/* socklen_t provided by newlib/libgloss */

#define HOMEPAGE_URL "https://lynx.invisible-island.net/"

#endif /* LYNX_CFG_H */
