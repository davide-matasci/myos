#ifndef _SYS_DIRENT_H_
#define _SYS_DIRENT_H_

#include <sys/cdefs.h>
#include <sys/_types.h>

#ifndef _INO_T_DECLARED
typedef __ino_t ino_t;
#define _INO_T_DECLARED
#endif

#ifndef _OFF_T_DECLARED
typedef __off_t off_t;
#define _OFF_T_DECLARED
#endif

#define MYOS_DIRBUF 4096

#define DT_UNKNOWN 0
#define DT_FIFO 1
#define DT_CHR 2
#define DT_DIR 4
#define DT_BLK 6
#define DT_REG 8
#define DT_LNK 10

struct dirent {
	ino_t d_ino;
	off_t d_off;
	unsigned short d_reclen;
	unsigned char d_type;
	char d_name[256];
};

/* An open directory (dirent.c): its fd, or -1 for one a namespace makes up
 * (it cannot be opened, only listed by its path), and its names, one per
 * line, read at opendir and rewinddir (the buffer grows from MYOS_DIRBUF to
 * hold them). */
typedef struct {
	int fd;
	char *buf;
	unsigned long cap;
	char path[256];
	unsigned long len;
	unsigned long pos;
	struct dirent ent;
} DIR;

#endif /* _SYS_DIRENT_H_ */
