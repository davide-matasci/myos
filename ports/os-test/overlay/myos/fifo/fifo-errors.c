/* mkfifo() fails with EEXIST on an existing path and ENOENT when the parent
 * directory does not exist. */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* path = join(dir, "fifo");
	if ( mkfifo(path, 0600) < 0 )
		err(1, "mkfifo");
	errno = 0;
	if ( mkfifo(path, 0600) == 0 )
		errx(1, "second mkfifo succeeded");
	if ( errno != EEXIST )
		err(1, "second mkfifo: want EEXIST");
	errno = 0;
	if ( mkfifo(join(dir, "missing/fifo"), 0600) == 0 )
		errx(1, "mkfifo in a missing directory succeeded");
	if ( errno != ENOENT )
		err(1, "mkfifo in a missing directory: want ENOENT");
	unlink(path);
	rmdir(dir);
	return 0;
}
