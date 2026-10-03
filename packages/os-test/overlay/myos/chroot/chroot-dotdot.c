/* ".." at the new root stays at the root: a chrooted process cannot walk out
 * of its jail with relative paths. */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* jail = join(dir, "jail");
	char* marker = join(dir, "escape-marker");
	if ( mkdir(jail, 0755) < 0 )
		err(1, "mkdir: %s", jail);
	touch(marker);
	pid_t pid = fork();
	if ( pid < 0 )
		err(1, "fork");
	if ( !pid )
	{
		if ( chroot(jail) < 0 )
			err(1, "chroot");
		if ( chdir("/") < 0 )
			err(1, "chdir /");
		if ( chdir("../../..") < 0 )
			err(1, "chdir ../../..");
		char cwd[256];
		if ( !getcwd(cwd, sizeof(cwd)) )
			err(1, "getcwd");
		if ( strcmp(cwd, "/") != 0 )
			errx(1, "getcwd() after chdir(\"../../..\") = \"%s\"", cwd);
		if ( access("../escape-marker", F_OK) == 0 )
			errx(1, "../escape-marker reached from the jail root");
		if ( access("/../../escape-marker", F_OK) == 0 )
			errx(1, "/../../escape-marker reached from the jail root");
		_exit(0);
	}
	wait_ok(pid, "chroot-dotdot");
	unlink(marker);
	rmdir(jail);
	rmdir(dir);
	return 0;
}
