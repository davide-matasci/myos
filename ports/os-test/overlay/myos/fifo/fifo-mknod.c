/* mknod(S_IFIFO) and mkfifoat(dirfd, ...) both create FIFOs. */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* a = join(dir, "via-mknod");
	if ( mknod(a, S_IFIFO | 0600, 0) < 0 )
		err(1, "mknod S_IFIFO");
	struct stat st;
	if ( stat(a, &st) < 0 || !S_ISFIFO(st.st_mode) )
		errx(1, "mknod did not create a FIFO");
	int dfd = open(dir, O_RDONLY);
	if ( dfd < 0 )
		err(1, "open dir");
	if ( mkfifoat(dfd, "via-mkfifoat", 0600) < 0 )
		err(1, "mkfifoat");
	close(dfd);
	char* b = join(dir, "via-mkfifoat");
	if ( stat(b, &st) < 0 || !S_ISFIFO(st.st_mode) )
		errx(1, "mkfifoat did not create a FIFO at %s", b);
	unlink(a);
	unlink(b);
	rmdir(dir);
	return 0;
}
