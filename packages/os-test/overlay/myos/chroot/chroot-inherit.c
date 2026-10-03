/* The root directory is inherited by children forked after chroot(). */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* jail = join(dir, "jail");
	if ( mkdir(jail, 0755) < 0 )
		err(1, "mkdir: %s", jail);
	touch(join(jail, "inside"));
	pid_t pid = fork();
	if ( pid < 0 )
		err(1, "fork");
	if ( !pid )
	{
		if ( chroot(jail) < 0 )
			err(1, "chroot");
		pid_t grandchild = fork();
		if ( grandchild < 0 )
			err(1, "fork");
		if ( !grandchild )
		{
			if ( access("/inside", F_OK) < 0 )
				err(1, "grandchild: access /inside");
			if ( access(jail, F_OK) == 0 )
				errx(1, "grandchild sees the real path %s", jail);
			_exit(0);
		}
		wait_ok(grandchild, "grandchild");
		_exit(0);
	}
	wait_ok(pid, "chroot-inherit");
	unlink(join(jail, "inside"));
	rmdir(jail);
	rmdir(dir);
	return 0;
}
