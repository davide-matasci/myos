/* pathdef.c — myos port (generated at prepare time from this template). */
#include "vim.h"
char_u *default_vim_dir = (char_u *)"/usr/share/vim";
char_u *default_vimruntime_dir = (char_u *)"";
char_u *all_cflags = (char_u *)"x86_64-unknown-myos-cc -c -I. -Iproto -DHAVE_CONFIG_H -DFEAT_TINY -ffreestanding";
char_u *all_lflags = (char_u *)"x86_64-unknown-myos-ld -pie -lc -lgloss";
char_u *compiled_user = (char_u *)"myos";
char_u *compiled_sys = (char_u *)"myos";
