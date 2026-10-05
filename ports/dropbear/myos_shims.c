/* myos_shims.c — what dropbear 2026.94 needs beyond libgloss/myos.
 * Compiled into every arch's link (see build.sh). The passwd/group
 * lookups, exec*, dup, fsync, daemon, nanosleep and readv/writev are
 * libgloss's; what is left has no honest implementation on myos. */
#include "myos_compat.h"

/* Valid login shells: must match root's pw_shell (/bin/custom/sh, libgloss
 * pwdgrp.c) or dropbear's check_shell() rejects the login. Dropbear's own
 * getusershell (HAVE_GETUSERSHELL off) would read /etc/shells, which the
 * image does not carry, and fall back to /bin/sh and /bin/csh. */
static const char *const g_shells[] = { "/bin/custom/sh", "/bin/sh" };
static int g_shell_idx = 0;

char *getusershell(void) {
	if (g_shell_idx >= (int)(sizeof(g_shells) / sizeof(g_shells[0]))) {
		return NULL;
	}
	return (char *)g_shells[g_shell_idx++];
}

void endusershell(void) {
	g_shell_idx = 0;
}

void setusershell(void) {
	g_shell_idx = 0;
}

/* --- set*id with a saved id: myos has no saved set-ids (every process is
 * root); sysoptions.h wants setresgid for DROPBEAR_SVR_DROP_PRIVS --- */
int setresuid(uid_t ruid, uid_t euid, uid_t suid) {
	(void)ruid;
	(void)suid;
	return setuid(euid);
}

int setresgid(gid_t rgid, gid_t egid, gid_t sgid) {
	(void)rgid;
	(void)sgid;
	return setgid(egid);
}

/* --- rlimit: dropbear only disables core dumps with it; myos has neither
 * limits nor cores --- */
int getrlimit(int resource, struct rlimit *rlim) {
	if (!rlim) {
		errno = EFAULT;
		return -1;
	}
	rlim->rlim_cur = RLIM_INFINITY;
	rlim->rlim_max = RLIM_INFINITY;
	(void)resource;
	return 0;
}

int setrlimit(int resource, const struct rlimit *rlim) {
	(void)resource;
	(void)rlim;
	return 0;
}
