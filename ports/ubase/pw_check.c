/* myos: the kernel checks the password (docs/security.md): setuser runs
 * this process as the user, in their login domain, when it is right (or
 * the user has none and login may enter it). */
#include <pwd.h>
#include <string.h>
#include <sys/myos_extra.h>

#include "passwd.h"

int
pw_check(const struct passwd *pw, const char *pass)
{
	if (pw == NULL || pw->pw_name == NULL) {
		return -1;
	}
	return myos_setuser(pw->pw_name, pass != NULL ? pass : "") == 0 ? 1 : 0;
}

int
pw_init(void)
{
	return 0;
}
