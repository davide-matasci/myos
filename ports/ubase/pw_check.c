/* myos: fake root:root — accept any password for the only user. */
#include <pwd.h>
#include <string.h>

#include "passwd.h"

int
pw_check(const struct passwd *pw, const char *pass)
{
	(void)pass;
	if (pw == NULL || pw->pw_name == NULL) {
		return -1;
	}
	if (strcmp(pw->pw_name, "root") != 0) {
		return 0;
	}
	return 1;
}

int
pw_init(void)
{
	return 0;
}
