/* myos ubase config: fake single-user paths, no real utmp/shadow. */
#define ENV_SUPATH	"/s:/c:/"
#define ENV_PATH	"/s:/c:/"
#define PW_CIPHER	"$6$"
#undef UTMP_PATH
#define UTMP_PATH	"/var/run/utmp"
#undef BTMP_PATH
#define BTMP_PATH	"/var/log/btmp"
#undef WTMP_PATH
#define WTMP_PATH	"/var/log/wtmp"
