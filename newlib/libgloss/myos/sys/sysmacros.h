#ifndef _SYS_SYSMACROS_H_
#define _SYS_SYSMACROS_H_

#define major(dev) ((unsigned int)(((unsigned long long)(dev) >> 8) & 0xfff))
#define minor(dev) ((unsigned int)((unsigned long long)(dev) & 0xff))
#define makedev(maj, min) \
    ((dev_t)((((maj) & 0xfff) << 8) | ((min) & 0xff)))

#endif /* _SYS_SYSMACROS_H_ */
