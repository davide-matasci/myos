/* Oksh-only no-ops for POSIX id/passwd calls newlib declares but myos
 * libgloss does not implement. Not part of libgloss. */
#include <grp.h>
#include <pwd.h>
#include <sys/types.h>
#include <unistd.h>

int
setuid(uid_t uid)
{
	(void)uid;
	return 0;
}

int
seteuid(uid_t uid)
{
	(void)uid;
	return 0;
}

int
setgid(gid_t gid)
{
	(void)gid;
	return 0;
}

int
setegid(gid_t gid)
{
	(void)gid;
	return 0;
}

int
setgroups(int size, const gid_t *list)
{
	(void)size;
	(void)list;
	return 0;
}

gid_t
getegid(void)
{
	return 0;
}

void
setpwent(void)
{
}

struct passwd *
getpwent(void)
{
	return NULL;
}

void
endpwent(void)
{
}
