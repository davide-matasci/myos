#ifndef _SYS_SYSLOG_H
#define _SYS_SYSLOG_H

/* syslog for myos (libgloss syslog.c): there is no log daemon, a message is
 * one line on the console, "ident[pid]: message". Priorities and facilities
 * have the usual values; the facility is not shown. */
#include <stdarg.h>

#ifdef __cplusplus
extern "C" {
#endif

#define LOG_EMERG   0
#define LOG_ALERT   1
#define LOG_CRIT    2
#define LOG_ERR     3
#define LOG_WARNING 4
#define LOG_NOTICE  5
#define LOG_INFO    6
#define LOG_DEBUG   7

#define LOG_PRIMASK 0x07
#define LOG_PRI(p) ((p) & LOG_PRIMASK)
#define LOG_MAKEPRI(fac, pri) ((fac) | (pri))

#define LOG_KERN     (0 << 3)
#define LOG_USER     (1 << 3)
#define LOG_MAIL     (2 << 3)
#define LOG_DAEMON   (3 << 3)
#define LOG_AUTH     (4 << 3)
#define LOG_SYSLOG   (5 << 3)
#define LOG_LPR      (6 << 3)
#define LOG_NEWS     (7 << 3)
#define LOG_UUCP     (8 << 3)
#define LOG_CRON     (9 << 3)
#define LOG_AUTHPRIV (10 << 3)
#define LOG_FTP      (11 << 3)
#define LOG_LOCAL0   (16 << 3)
#define LOG_LOCAL1   (17 << 3)
#define LOG_LOCAL2   (18 << 3)
#define LOG_LOCAL3   (19 << 3)
#define LOG_LOCAL4   (20 << 3)
#define LOG_LOCAL5   (21 << 3)
#define LOG_LOCAL6   (22 << 3)
#define LOG_LOCAL7   (23 << 3)

#define LOG_NFACILITIES 24
#define LOG_FACMASK 0x03f8
#define LOG_FAC(p) (((p) & LOG_FACMASK) >> 3)

#define LOG_MASK(pri) (1 << (pri))
#define LOG_UPTO(pri) ((1 << ((pri) + 1)) - 1)

/* openlog options */
#define LOG_PID    0x01 /* "ident[pid]: " */
#define LOG_CONS   0x02 /* every message goes to the console anyway */
#define LOG_ODELAY 0x04
#define LOG_NDELAY 0x08
#define LOG_NOWAIT 0x10
#define LOG_PERROR 0x20 /* also to stderr */

#ifdef SYSLOG_NAMES
/* The names of the priorities and facilities (logger -p facility.level). */
#define INTERNAL_NOPRI 0x10
#define INTERNAL_MARK  LOG_MAKEPRI(LOG_NFACILITIES << 3, 0)

typedef struct _code {
    const char *c_name;
    int c_val;
} CODE;

static CODE prioritynames[] = {
    {"alert", LOG_ALERT},     {"crit", LOG_CRIT},       {"debug", LOG_DEBUG},
    {"emerg", LOG_EMERG},     {"err", LOG_ERR},         {"error", LOG_ERR},
    {"info", LOG_INFO},       {"none", INTERNAL_NOPRI}, {"notice", LOG_NOTICE},
    {"panic", LOG_EMERG},     {"warn", LOG_WARNING},    {"warning", LOG_WARNING},
    {0, -1},
};

static CODE facilitynames[] = {
    {"auth", LOG_AUTH},     {"authpriv", LOG_AUTHPRIV}, {"cron", LOG_CRON},
    {"daemon", LOG_DAEMON}, {"ftp", LOG_FTP},           {"kern", LOG_KERN},
    {"lpr", LOG_LPR},       {"mail", LOG_MAIL},         {"mark", INTERNAL_MARK},
    {"news", LOG_NEWS},     {"security", LOG_AUTH},     {"syslog", LOG_SYSLOG},
    {"user", LOG_USER},     {"uucp", LOG_UUCP},         {"local0", LOG_LOCAL0},
    {"local1", LOG_LOCAL1}, {"local2", LOG_LOCAL2},     {"local3", LOG_LOCAL3},
    {"local4", LOG_LOCAL4}, {"local5", LOG_LOCAL5},     {"local6", LOG_LOCAL6},
    {"local7", LOG_LOCAL7}, {0, -1},
};
#endif

void openlog(const char *ident, int option, int facility);
void closelog(void);
int setlogmask(int mask);
void syslog(int priority, const char *format, ...);
void vsyslog(int priority, const char *format, va_list ap);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_SYSLOG_H */
