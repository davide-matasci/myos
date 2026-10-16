/* ngx_auto_config.h: hand-written for myos, in place of what configure
 * writes (configure runs its test programs, which a cross build cannot).
 * The NGX_HAVE_* below are what newlib + libgloss provide; what they lack
 * is left out and nginx takes its fallback (fcntl for FIONBIO, usleep for
 * sched_yield, a spinning accept mutex without POSIX semaphores, its own
 * crypt schemes without libc's crypt). The events module is poll. */

#define NGX_CONFIGURE " (myos: packages/nginx/build.sh)"
#define NGX_COMPILER "clang (myos)"

/* The compiler. */
#define NGX_HAVE_GCC_ATOMIC  1
#define NGX_HAVE_C99_VARIADIC_MACROS  1
#define NGX_HAVE_GCC_VARIADIC_MACROS  1
#define NGX_HAVE_GCC_BSWAP64  1

/* The machine: what auto/os/conf sets per arch. */
#if defined(__x86_64__)
#define NGX_HAVE_NONALIGNED  1
#else
#define NGX_ALIGNMENT  16
#endif
#define NGX_CPU_CACHE_LINE  64

/* Types: every myos arch is LP64 and little endian, off_t and time_t
 * 64-bit. */
#define NGX_PTR_SIZE  8
#define NGX_SIG_ATOMIC_T_SIZE  4
#define NGX_HAVE_LITTLE_ENDIAN  1
#define NGX_MAX_SIZE_T_VALUE  9223372036854775807LL
#define NGX_SIZE_T_LEN  (sizeof("-9223372036854775808") - 1)
#define NGX_MAX_OFF_T_VALUE  9223372036854775807LL
#define NGX_OFF_T_LEN  (sizeof("-9223372036854775808") - 1)
#define NGX_TIME_T_SIZE  8
#define NGX_TIME_T_LEN  (sizeof("-9223372036854775808") - 1)
#define NGX_MAX_TIME_T_VALUE  9223372036854775807LL

/* newlib's errno values end at 143 (ESTRPIPE). */
#define NGX_SYS_NERR  144

/* The system. */
#define NGX_HAVE_POLL  1
#define NGX_HAVE_STATVFS  1
#define NGX_HAVE_INET6  1
#define NGX_HAVE_UNIX_DOMAIN  1
#define NGX_HAVE_PREAD  1
#define NGX_HAVE_PWRITE  1
#define NGX_HAVE_LOCALTIME_R  1
#define NGX_HAVE_CLOCK_MONOTONIC  1
#define NGX_HAVE_POSIX_MEMALIGN  1
#define NGX_HAVE_MEMALIGN  1
#define NGX_HAVE_MAP_ANON  1
#define NGX_HAVE_MSGHDR_MSG_CONTROL  1
#define NGX_HAVE_D_TYPE  1
#define NGX_HAVE_SC_NPROCESSORS_ONLN  1
#define NGX_HAVE_OPENAT  1
#define NGX_HAVE_GETADDRINFO  1

/* The modules (auto/modules, auto/options: the default set without epoll
 * and select, with PCRE2, zlib and the SSL module over LibreSSL,
 * ports/libressl). */
#define NGX_HTTP_CACHE  1
#define NGX_HTTP_GZIP  1
#define NGX_HTTP_SSI  1
#define NGX_CRYPT  1
#define NGX_HTTP_X_FORWARDED_FOR  1
#define NGX_HTTP_UPSTREAM_ZONE  1
#define NGX_HTTP_UPSTREAM_STICKY  1
#define NGX_HTTP_UPSTREAM_SID  1
#define NGX_HTTP_SSL  1
#define NGX_PCRE2  1
#define NGX_PCRE  1
#define NGX_OPENSSL  1
#define NGX_SSL  1
#define NGX_ZLIB  1

/* Paths: the configuration, its types and pages in the app (read only),
 * what nginx writes under /tmp/nginx, which it creates at startup
 * (nginx.c.myos.patch: myos has no /var). */
#define NGX_PREFIX  "/lib/nginx/"
#define NGX_CONF_PREFIX  "/lib/nginx/"
#define NGX_SBIN_PATH  "/bin/custom/nginx"
#define NGX_CONF_PATH  "/lib/nginx/nginx.conf"
#define NGX_MYOS_RUN_PATH  "/tmp/nginx"
#define NGX_PID_PATH  NGX_MYOS_RUN_PATH "/nginx.pid"
#define NGX_LOCK_PATH  NGX_MYOS_RUN_PATH "/nginx.lock"
#define NGX_ERROR_LOG_PATH  NGX_MYOS_RUN_PATH "/error.log"
#define NGX_HTTP_LOG_PATH  NGX_MYOS_RUN_PATH "/access.log"
#define NGX_HTTP_CLIENT_TEMP_PATH  NGX_MYOS_RUN_PATH "/client_body_temp"
#define NGX_HTTP_PROXY_TEMP_PATH  NGX_MYOS_RUN_PATH "/proxy_temp"
#define NGX_HTTP_FASTCGI_TEMP_PATH  NGX_MYOS_RUN_PATH "/fastcgi_temp"
#define NGX_HTTP_UWSGI_TEMP_PATH  NGX_MYOS_RUN_PATH "/uwsgi_temp"
#define NGX_HTTP_SCGI_TEMP_PATH  NGX_MYOS_RUN_PATH "/scgi_temp"

#define NGX_SUPPRESS_WARN  1
#define NGX_SMP  1

/* myos has no superuser and no nobody: root (uid 0) is the user a master
 * started by root gives its workers (setuid changes nothing on myos). */
#define NGX_USER  "root"
#define NGX_GROUP  "root"
