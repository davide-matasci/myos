/* get-myos --upgrade and --install: the boot disk's slots, see boot.c. */
#ifndef MYOS_GET_MYOS_BOOT_H
#define MYOS_GET_MYOS_BOOT_H

#include <stddef.h>

/* `list_path` is the mirror's <arch>-boot.txt, downloaded; `url_of` names
 * a file of the mirror. 0 on success. */
int boot_upgrade(const char *list_path, void (*url_of)(char *url, size_t cap, const char *file), int force);
int boot_install(const char *disk, const char *list_path,
                 void (*url_of)(char *url, size_t cap, const char *file));

#endif
