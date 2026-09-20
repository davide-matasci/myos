/* myos localoptions.h — dropbear 2026.94 feature selection for myos.
 * Kept minimal: sshd + dbclient, pubkey KEX/auth only (no passwd db on myos),
 * no utmp/wtmp/lastlog, no syslog (log to stderr via -F/-E), no X11/agent fwd. */

/* --- auth: pubkey only, root passwordless via getpwnam shim --- */
#define DROPBEAR_PASSWORD_AUTH 0
#define DROPBEAR_PAM_AUTH 0
#define ENABLE_SVR_PASSWORD_AUTH 0
#define ENABLE_SVR_PAM_AUTH 0
#define ENABLE_CLI_PASSWORD_AUTH 0
#define ENABLE_CLI_INTERACT_AUTH 0
#define DROPBEAR_SVR_PASSWORD_AUTH 0
#define DROPBEAR_CLI_PASSWORD_AUTH 0
#define DROPBEAR_CLI_INTERACT_AUTH 0

/* TEMP bisect: myos re-exec (fexecve/dup2) path unproven — disable for now. */
#define DROPBEAR_REEXEC 0

/* --- platform services myos lacks --- */
#define DROPBEAR_UTMP 0
#define DROPBEAR_WTMP 0
#define DROPBEAR_LASTLOG 0
#define DROPBEAR_SYSLOG 0
#define DROPBEAR_X11FWD 0
#define DROPBEAR_SVR_AGENTFWD 0
#define DROPBEAR_CLI_AGENTFWD 0
/* NOTE: DROPBEAR_PIDFILE is a PATH (default /var/run/dropbear.pid), not a
 * 0/1 flag — setting it to 0 crashes svr_getopts (expand_homedir_path(NULL)). */
#define DROPBEAR_PUTENV 1

/* --- channels: tcp fwd left at defaults (listener over netfs) --- */

/* --- compression: off (keeps the port free of a zlib dependency) --- */
#define DISABLE_ZLIB 1

/* --- build both sshd and dbclient (configure normally sets these) --- */
#define DROPBEAR_SERVER 1
#define DROPBEAR_CLIENT 1

/* runtime -v/-vvv trace (needed to debug boot issues; kept for CI triage) */
#define DEBUG_TRACE 1

/* --- algorithms: keep defaults (x25519, ecdsa, ed25519, chachapoly, aes) ---
 * defaults in default_options.h are fine; nothing overridden here yet. */
