/* The two C libraries over this directory (README.md): every public function
 * here is defined as __myos_NAME. newlib's libgloss exports it as NAME with
 * the alias below (libgloss/myos/hooks.c defines the _NAME hooks newlib's
 * own libc calls instead); mlibc's libc defines NAME itself, over the
 * sysdeps that call __myos_NAME (toolchain/mlibc/sysdeps/myos), and is built
 * with MYOS_LIBC_MLIBC, which leaves the aliases out. */
#ifndef MYOS_LIBC_H
#define MYOS_LIBC_H

/* A name neither libc defines itself (newlib and mlibc lack it): the
 * alias in both. */
#define MYOS_PUBLIC(name) \
    extern __typeof(__myos_##name) name __attribute__((alias("__myos_" #name)));

#ifdef MYOS_LIBC_MLIBC
#define MYOS_ALIAS(name)
#define MYOS_WEAK_ALIAS(name)
#else
#define MYOS_ALIAS(name) \
    extern __typeof(__myos_##name) name __attribute__((alias("__myos_" #name)));
/* A port's own definition wins (dropbear's nanosleep). */
#define MYOS_WEAK_ALIAS(name) \
    extern __typeof(__myos_##name) name __attribute__((weak, alias("__myos_" #name)));
#endif

#endif
