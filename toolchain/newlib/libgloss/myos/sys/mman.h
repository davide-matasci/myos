#ifndef _SYS_MMAN_H_
#define _SYS_MMAN_H_

#include <sys/cdefs.h>
#include <sys/_types.h>
#include <stddef.h>

#ifndef _OFF_T_DECLARED
typedef __off_t off_t;
#define _OFF_T_DECLARED
#endif

#define PROT_NONE  0x00
#define PROT_READ  0x01
#define PROT_WRITE 0x02
#define PROT_EXEC  0x04

#define MAP_SHARED    0x01
#define MAP_PRIVATE   0x02
#define MAP_FIXED     0x10
#define MAP_ANON      0x20
#define MAP_ANONYMOUS MAP_ANON

#define MAP_FAILED ((void *)-1)

void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset);
int munmap(void *addr, size_t length);
int mprotect(void *addr, size_t length, int prot);

void *_mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset);
int _munmap(void *addr, size_t length);
int _mprotect(void *addr, size_t length, int prot);

#endif /* _SYS_MMAN_H_ */
