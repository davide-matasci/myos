/* What ubase's login expects from <stdlib.h>: newlib declares no clearenv
 * (libgloss implements it). Compile-only for ubase; not copied into the
 * newlib sysroot. */
#ifndef _MYOS_UBASE_COMPAT_H_
#define _MYOS_UBASE_COMPAT_H_

int clearenv(void);

#endif /* _MYOS_UBASE_COMPAT_H_ */
