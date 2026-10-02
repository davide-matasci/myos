/* mkfifo() creates a node that stat() reports as a FIFO; unlink() removes it. */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* path = join(dir, "fifo");
	if ( mkfifo(path, 0600) < 0 )
		err(1, "mkfifo");
	struct stat st;
	if ( stat(path, &st) < 0 )
		err(1, "stat");
	if ( !S_ISFIFO(st.st_mode) )
		errx(1, "stat mode %#o is not S_IFIFO", (unsigned) st.st_mode);
	if ( unlink(path) < 0 )
		err(1, "unlink");
	if ( stat(path, &st) == 0 )
		errx(1, "fifo still exists after unlink");
	rmdir(dir);
	return 0;
}
