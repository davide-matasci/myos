/* mount(2) on myos (libmyos/mount.c, docs/ports.md): a filesystem type on
 * a device, at a mount point; the flags and data of other systems are not
 * taken. Unmounting is the umount program's (user/umount). */
#ifndef _SYS_MOUNT_H
#define _SYS_MOUNT_H

#ifdef __cplusplus
extern "C" {
#endif

int _mount(const char *source, const char *target, const char *fstype);
int mount(const char *source, const char *target, const char *fstype, ...);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_MOUNT_H */
