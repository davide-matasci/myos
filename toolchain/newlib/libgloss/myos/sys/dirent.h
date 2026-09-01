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
#define DT_DIR 4
#define DT_REG 8

struct dirent {
	ino_t d_ino;
	off_t d_off;
	unsigned short d_reclen;
	unsigned char d_type;
	char d_name[256];
};

typedef struct {
	char buf[MYOS_DIRBUF];
	unsigned long len;
	unsigned long pos;
	struct dirent ent;
} DIR;

#endif /* _SYS_DIRENT_H_ */
