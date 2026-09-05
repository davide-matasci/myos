/* myos ubase config: fake single-user paths, no real utmp/shadow. */
#define ENV_SUPATH	"/bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc"
#define ENV_PATH	"/bin/sbase:/bin/coreutils:/bin/ubase:/bin/custom:/bin/tcc:/bin/std:/bin/etc"
#define PW_CIPHER	"$6$"
#undef UTMP_PATH
#define UTMP_PATH	"/var/run/utmp"
#undef BTMP_PATH
#define BTMP_PATH	"/var/log/btmp"
#undef WTMP_PATH
#define WTMP_PATH	"/var/log/wtmp"
