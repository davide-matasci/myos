/* Minimal <sys/syslog.h> for myos: dropbear includes it unconditionally in
 * cli-main.c even with DROPBEAR_SYSLOG=0. Priorities + openlog stub macros. */
#ifndef DROPBEAR_MYOS_SYSLOG_H
#define DROPBEAR_MYOS_SYSLOG_H

#define LOG_EMERG 0
#define LOG_ALERT 1
#define LOG_CRIT 2
#define LOG_ERR 3
#define LOG_WARNING 4
#define LOG_NOTICE 5
#define LOG_INFO 6
#define LOG_DEBUG 7

#define LOG_PID 0x01
#define LOG_CONS 0x02
#define LOG_NDELAY 0x08

#define LOG_DAEMON (3 << 3)
#define LOG_USER (1 << 3)
#define LOG_AUTH (4 << 3)
#define LOG_AUTHPRIV (10 << 3)

#define LOG_PRIMASK 0x07
#define PRI(n) ((n) & LOG_PRIMASK)

/* Declarations only; myos builds call with DROPBEAR_SYSLOG=0 paths, but the
 * call sites compile unconditionally. */
void syslog(int priority, const char *format, ...);
void openlog(const char *ident, int logopt, int facility);
void closelog(void);

#endif /* DROPBEAR_MYOS_SYSLOG_H */
