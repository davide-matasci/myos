/* A FIFO is reusable: after one writer session ends (reader sees EOF), a new
 * reader/writer pair can open it again and exchange data. */

#include "../myos.h"

static void session(const char* path, const char* msg)
{
	pid_t pid = fork();
	if ( pid < 0 )
		err(1, "fork");
	if ( !pid )
	{
		int fd = open(path, O_WRONLY);
		if ( fd < 0 )
			err(1, "child: open O_WRONLY");
		size_t len = strlen(msg);
		if ( write(fd, msg, len) != (ssize_t) len )
			err(1, "child: write");
		close(fd);
		_exit(0);
	}
	int fd = open(path, O_RDONLY);
	if ( fd < 0 )
		err(1, "open O_RDONLY");
	char buf[32];
	size_t got = 0;
	for ( ;; )
	{
		ssize_t n = read(fd, buf + got, sizeof(buf) - 1 - got);
		if ( n < 0 )
			err(1, "read");
		if ( n == 0 )
			break;
		got += n;
	}
	buf[got] = '\0';
	close(fd);
	wait_ok(pid, "writer");
	if ( strcmp(buf, msg) != 0 )
		errx(1, "read \"%s\", want \"%s\"", buf, msg);
}

int main(void)
{
	char* dir = make_scratch();
	char* path = join(dir, "fifo");
	if ( mkfifo(path, 0600) < 0 )
		err(1, "mkfifo");
	session(path, "first");
	session(path, "second");
	unlink(path);
	rmdir(dir);
	return 0;
}
