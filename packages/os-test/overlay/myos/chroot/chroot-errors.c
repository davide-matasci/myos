/* chroot() rejects paths that are not existing directories. */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* file = join(dir, "file");
	touch(file);
	errno = 0;
	if ( chroot(join(dir, "missing")) == 0 )
		errx(1, "chroot(missing) succeeded");
	if ( errno != ENOENT )
		err(1, "chroot(missing): want ENOENT");
	errno = 0;
	if ( chroot(file) == 0 )
		errx(1, "chroot(regular file) succeeded");
	if ( errno != ENOTDIR )
		err(1, "chroot(regular file): want ENOTDIR");
	unlink(file);
	rmdir(dir);
	return 0;
}
