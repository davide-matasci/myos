// A program's entry: main and the entry stack to mlibc's startup (the
// kernel's argc and argv in a0 and a1 are on the stack too). gp is the
// global pointer the linker relaxes accesses against.
    .text
    .globl _start
_start:
    .option push
    .option norelax
    lla gp, __global_pointer$
    .option pop
    lla a0, main
    mv a1, sp
    call __mlibc_entry
    unimp

    .section .note.GNU-stack,"",%progbits
