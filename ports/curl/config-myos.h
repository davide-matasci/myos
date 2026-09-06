/* Hand-crafted curl_config.h for myos (userspace BSD sockets + mbedtls). */
#ifndef HEADER_CURL_CONFIG_MYOS_H
#define HEADER_CURL_CONFIG_MYOS_H

/* BUILDING_LIBCURL set via -D when compiling libcurl */
#define CURL_STATICLIB 1
#define CURL_OS "myos"
#define OS "myos"
#define PACKAGE "curl"
#define PACKAGE_NAME "curl"
#define PACKAGE_STRING "curl 8.11.1"
#define PACKAGE_TARNAME "curl"
#define PACKAGE_VERSION "8.11.1"
#define VERSION "8.11.1"

#define STDC_HEADERS 1
#define HAVE_BOOL_T 1
#define HAVE_STDBOOL_H 1
#include <stdbool.h>
#define HAVE_LONGLONG 1
#define HAVE_STRUCT_TIMEVAL 1
#define HAVE_SOCKLEN_T 1

#define SIZEOF_INT 4
#define SIZEOF_LONG 8
#define SIZEOF_LONG_LONG 8
#define SIZEOF_OFF_T 8
#define SIZEOF_CURL_OFF_T 8
#define SIZEOF_SIZE_T 8
#define SIZEOF_TIME_T 8
#define SIZEOF_SHORT 2

#define HAVE_FCNTL_H 1
#define HAVE_NETDB_H 1
#define HAVE_NETINET_IN_H 1
#define HAVE_SYS_SOCKET_H 1
#define HAVE_SYS_SELECT_H 1
#define HAVE_SYS_STAT_H 1
#define HAVE_SYS_TIME_H 1
#define HAVE_SYS_TYPES_H 1
#define HAVE_UNISTD_H 1
#define HAVE_ARPA_INET_H 1
#define HAVE_POLL_H 1
#define HAVE_STDLIB_H 1
#define HAVE_STRING_H 1
#define HAVE_STRINGS_H 1
#define HAVE_TIME_H 1
#define HAVE_ERRNO_H 1
#define HAVE_SETJMP_H 1
#define HAVE_SIGNAL_H 1

#define HAVE_SOCKET 1
#define HAVE_SELECT 1
#define HAVE_POLL_FINE 1
#define HAVE_POLL 1
#define HAVE_GETADDRINFO 1
#define HAVE_FREEADDRINFO 1
#define HAVE_GETHOSTBYNAME 1
#define HAVE_GETHOSTNAME 1
#define HAVE_GETNAMEINFO 1
#define HAVE_GETTIMEOFDAY 1
#define HAVE_INET_NTOP 1
#define HAVE_INET_PTON 1
#define HAVE_STRCASECMP 1
#define HAVE_STRNCASECMP 1
#define HAVE_STRDUP 1
#define HAVE_STRSTR 1
#define HAVE_STRTOK_R 1
#define HAVE_STRTOL 1
#define HAVE_STRTOUL 1
#define HAVE_STRTOLL 1
#define HAVE_SIGNAL 1
/* HAVE_SIGACTION not available */
#define HAVE_FORK 1
#define HAVE_PIPE 1
#define HAVE_FCNTL 1
#define HAVE_FCNTL_O_NONBLOCK 1
/* HAVE_ALARM not available */
/* HAVE_UTIME not available */
/* HAVE_LIBGEN_H not available */
/* HAVE_PWD_H not available */
/* HAVE_LOCALE_H not available */
/* HAVE_SETLOCALE not available */
/* HAVE_GETEUID not available */
/* HAVE_GETPWUID not available */
#define HAVE_GMTIME_R 1
#define HAVE_LOCALTIME_R 1
#define HAVE_FTRUNCATE 1
#define HAVE_UNAME 1
#define HAVE_SYS_UTSNAME_H 1

#define HAVE_RECV 1
#define RECV_TYPE_ARG1 int
#define RECV_TYPE_ARG2 void *
#define RECV_TYPE_ARG3 size_t
#define RECV_TYPE_ARG4 int
#define RECV_TYPE_RETV ssize_t

#define HAVE_SEND 1
#define SEND_TYPE_ARG1 int
#define SEND_TYPE_ARG2 void *
#define SEND_QUAL_ARG2 const
#define SEND_TYPE_ARG3 size_t
#define SEND_TYPE_ARG4 int
#define SEND_TYPE_RETV ssize_t

#define USE_MBEDTLS 1
/* CA: rely on mbedtls bundle baked into libmbedtls; also set CURLOPT_CAINFO if needed. */
#define CURL_CA_BUNDLE "/lib/cacert.pem"

#define CURL_DISABLE_LDAP 1
#define CURL_DISABLE_LDAPS 1
#define CURL_DISABLE_RTSP 1
#define CURL_DISABLE_PROXY 1
#define CURL_DISABLE_DICT 1
#define CURL_DISABLE_TELNET 1
#define CURL_DISABLE_TFTP 1
#define CURL_DISABLE_POP3 1
#define CURL_DISABLE_IMAP 1
#define CURL_DISABLE_SMB 1
#define CURL_DISABLE_SMTP 1
#define CURL_DISABLE_GOPHER 1
#define CURL_DISABLE_MQTT 1
#define CURL_DISABLE_DOH 1
#define CURL_DISABLE_ALTSVC 1
#define CURL_DISABLE_HSTS 1
#define CURL_DISABLE_WEBSOCKETS 1
#define CURL_DISABLE_IPFS 1
#define CURL_DISABLE_NETRC 1
#define CURL_DISABLE_PROGRESS_METER 1
#define CURL_DISABLE_COOKIES 1
#define CURL_DISABLE_SHUFFLE_DNS 1
#define CURL_DISABLE_MIME 1
#define CURL_DISABLE_FORM_API 1
#define CURL_DISABLE_BINDLOCAL 1
#define CURL_DISABLE_NTLM 1
#define CURL_DISABLE_DIGEST_AUTH 1
#define CURL_DISABLE_NEGOTIATE_AUTH 1
#define CURL_DISABLE_AWS 1
#define CURL_DISABLE_BASIC_AUTH 1
#define CURL_DISABLE_BEARER_AUTH 1
#define CURL_DISABLE_FILE 1
#define CURL_DISABLE_FTP 1
#define CURL_DISABLE_FTPS 1
#define CURL_DISABLE_SOCKETPAIR 1
#define CURL_DISABLE_LIBCURL_OPTION 1
#define CURL_DISABLE_GETOPTIONS 1
#define CURL_DISABLE_HEADERS_API 1
#define CURL_DISABLE_PARSEDATE 1
#define CURL_DISABLE_VERBOSE_STRINGS 0
#define HTTP_ONLY 1

/* No IPv6 yet. */
#undef USE_IPV6

#endif
