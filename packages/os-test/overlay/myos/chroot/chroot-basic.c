/* chroot(dir) makes dir the process's "/": files inside are reachable by
 * absolute path, files outside are not, and getcwd() reports "/". */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* jail = join(dir, "jail");
	char* outside = join(dir, "outside");
	if ( mkdir(jail, 0755) < 0 )
		err(1, "mkdir: %s", jail);
	char* inside = join(jail, "inside");
	touch(inside);
	touch(outside);
	pid_t pid = fork();
	if ( pid < 0 )
		err(1, "fork");
	if ( !pid )
	{
		if ( chroot(jail) < 0 )
			err(1, "chroot");
		if ( chdir("/") < 0 )
			err(1, "chdir /");
		if ( access("/inside", F_OK) < 0 )
			err(1, "access /inside");
		if ( access(outside, F_OK) == 0 )
			errx(1, "%s is visible from inside the jail", outside);
		char cwd[256];
		if ( !getcwd(cwd, sizeof(cwd)) )
			err(1, "getcwd");
		if ( strcmp(cwd, "/") != 0 )
			errx(1, "getcwd() = \"%s\", want \"/\"", cwd);
		_exit(0);
	}
	wait_ok(pid, "chroot-basic");
	unlink(inside);
	unlink(outside);
	rmdir(jail);
	rmdir(dir);
	return 0;
}
