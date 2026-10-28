/* myos: JavaScriptCore's ARM64 assembler (built as for Linux) includes
 * Linux's for the HWCAP_ bits of AT_HWCAP. It defines the bits it tests
 * itself, and myos's auxiliary vector has no AT_HWCAP: getauxval answers
 * 0, so the JIT takes no optional instructions (LSE atomics, JSCVT, ...). */
#ifndef MYOS_COMPAT_ASM_HWCAP_H
#define MYOS_COMPAT_ASM_HWCAP_H
#endif
