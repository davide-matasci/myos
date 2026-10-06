#ifndef _SYS_REBOOT_H
#define _SYS_REBOOT_H

/* reboot(howto) (reboot.c, docs/power.md): stop the processes, unmount the
 * disks and reboot, halt or power off. Returns -1 (EPERM without `write`
 * on kernel.power, EINVAL for an unknown howto); otherwise not at all.
 * The BSD values, and Linux's names for them. */
#define RB_AUTOBOOT 0
#define RB_HALT 0x8
#define RB_POWEROFF 0x4000
#define RB_HALT_SYSTEM RB_HALT
#define RB_POWER_OFF RB_POWEROFF

#ifdef __cplusplus
extern "C" {
#endif

int reboot(int howto);

#ifdef __cplusplus
}
#endif

#endif /* _SYS_REBOOT_H */
