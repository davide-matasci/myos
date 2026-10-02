/* A second chroot() inside a jail is relative to the current root. */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* jail = join(dir, "jail");
	char* inner = join(jail, "inner");
	if ( mkdir(jail, 0755) < 0 || mkdir(inner, 0755) < 0 )
		err(1, "mkdir");
	touch(join(jail, "outer-file"));
	touch(join(inner, "inner-file"));
	pid_t pid = fork();
	if ( pid < 0 )
		err(1, "fork");
	if ( !pid )
	{
		if ( chroot(jail) < 0 )
			err(1, "chroot jail");
		if ( chroot("/inner") < 0 )
			err(1, "chroot /inner");
		if ( chdir("/") < 0 )
			err(1, "chdir /");
		if ( access("/inner-file", F_OK) < 0 )
			err(1, "access /inner-file");
		if ( access("/outer-file", F_OK) == 0 )
			errx(1, "/outer-file visible after nested chroot");
		_exit(0);
	}
	wait_ok(pid, "chroot-nested");
	unlink(join(inner, "inner-file"));
	unlink(join(jail, "outer-file"));
	rmdir(inner);
	rmdir(jail);
	rmdir(dir);
	return 0;
}
