// A new thread starts here with the stack prepared by PrepareStack
// (thread.cpp): the entry, the TCB and the argument on top.
    .text
    .globl __mlibc_thread_entry
__mlibc_thread_entry:
    .option push
    .option norelax
    lla gp, __global_pointer$
    .option pop
    ld a0, 0(sp)
    ld a1, 8(sp)
    ld a2, 16(sp)
    addi sp, sp, 32
    call __mlibc_thread_trampoline
    unimp

    .section .note.GNU-stack,"",%progbits
