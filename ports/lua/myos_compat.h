/* myos compat shims for the Lua 5.4 port. */
#ifndef MYOS_LUA_COMPAT_H
#define MYOS_LUA_COMPAT_H

/* No readline on myos; the interpreter reads plain lines from the console. */
/* luai_readline/ngetc defaults are fine without LUA_USE_READLINE. */

#endif /* MYOS_LUA_COMPAT_H */
