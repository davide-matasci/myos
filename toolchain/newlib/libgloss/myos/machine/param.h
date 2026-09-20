/* myos machine/param.h — page size and related constants. */
#ifndef _MYOS_MACHINE_PARAM_H_
#define _MYOS_MACHINE_PARAM_H_

#ifndef PAGESIZE
#define PAGESIZE 4096
#endif
#ifndef PAGE_SIZE
#define PAGE_SIZE PAGESIZE
#endif
#ifndef NBPG
#define NBPG PAGESIZE
#endif
#ifndef CLBYTES
#define CLBYTES PAGESIZE
#endif

#endif /* _MYOS_MACHINE_PARAM_H_ */
