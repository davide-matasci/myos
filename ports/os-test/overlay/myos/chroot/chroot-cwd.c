/* A working directory inside the new root keeps pointing at the same
 * directory, now named relative to the jail. */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* jail = join(dir, "jail");
	char* sub = join(jail, "sub");
	if ( mkdir(jail, 0755) < 0 || mkdir(sub, 0755) < 0 )
		err(1, "mkdir");
	touch(join(sub, "here"));
	pid_t pid = fork();
	if ( pid < 0 )
		err(1, "fork");
	if ( !pid )
	{
		if ( chdir(sub) < 0 )
			err(1, "chdir: %s", sub);
		if ( chroot(jail) < 0 )
			err(1, "chroot");
		char cwd[256];
		if ( !getcwd(cwd, sizeof(cwd)) )
			err(1, "getcwd");
		if ( strcmp(cwd, "/sub") != 0 )
			errx(1, "getcwd() = \"%s\", want \"/sub\"", cwd);
		if ( access("here", F_OK) < 0 )
			err(1, "access here (relative to cwd)");
		_exit(0);
	}
	wait_ok(pid, "chroot-cwd");
	unlink(join(sub, "here"));
	rmdir(sub);
	rmdir(jail);
	rmdir(dir);
	return 0;
}
