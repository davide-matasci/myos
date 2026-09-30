/* Blocking opens rendezvous: a writer and a reader meet on the FIFO, data
 * flows, and the reader sees EOF once the writer closes. */

#include "../myos.h"

int main(void)
{
	char* dir = make_scratch();
	char* path = join(dir, "fifo");
	if ( mkfifo(path, 0600) < 0 )
		err(1, "mkfifo");
	pid_t pid = fork();
	if ( pid < 0 )
		err(1, "fork");
	if ( !pid )
	{
		int fd = open(path, O_WRONLY);
		if ( fd < 0 )
			err(1, "child: open O_WRONLY");
		if ( write(fd, "hello", 5) != 5 )
			err(1, "child: write");
		close(fd);
		_exit(0);
	}
	int fd = open(path, O_RDONLY);
	if ( fd < 0 )
		err(1, "open O_RDONLY");
	char buf[16];
	size_t got = 0;
	while ( got < 5 )
	{
		ssize_t n = read(fd, buf + got, sizeof(buf) - got);
		if ( n < 0 )
			err(1, "read");
		if ( n == 0 )
			break;
		got += n;
	}
	if ( got != 5 || memcmp(buf, "hello", 5) != 0 )
		errx(1, "read %zu bytes, want \"hello\"", got);
	wait_ok(pid, "writer");
	ssize_t n = read(fd, buf, sizeof(buf));
	if ( n != 0 )
		errx(1, "read after writer closed returned %zd, want 0 (EOF)", n);
	close(fd);
	unlink(path);
	rmdir(dir);
	return 0;
}
