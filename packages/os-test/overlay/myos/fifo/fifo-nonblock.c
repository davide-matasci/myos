/* O_NONBLOCK opens: a reader opens at once without a writer, and a writer
 * fails with ENXIO without a reader but succeeds once one is open. */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* path = join(dir, "fifo");
	if ( mkfifo(path, 0600) < 0 )
		err(1, "mkfifo");
	errno = 0;
	int wfd = open(path, O_WRONLY | O_NONBLOCK);
	if ( wfd >= 0 )
		errx(1, "O_WRONLY|O_NONBLOCK open without a reader succeeded");
	if ( errno != ENXIO )
		err(1, "O_WRONLY|O_NONBLOCK without a reader: want ENXIO");
	int rfd = open(path, O_RDONLY | O_NONBLOCK);
	if ( rfd < 0 )
		err(1, "O_RDONLY|O_NONBLOCK open");
	wfd = open(path, O_WRONLY | O_NONBLOCK);
	if ( wfd < 0 )
		err(1, "O_WRONLY|O_NONBLOCK open with a reader");
	if ( write(wfd, "x", 1) != 1 )
		err(1, "write");
	char c;
	if ( read(rfd, &c, 1) != 1 || c != 'x' )
		errx(1, "read did not return the written byte");
	close(wfd);
	close(rfd);
	unlink(path);
	rmdir(dir);
	return 0;
}
