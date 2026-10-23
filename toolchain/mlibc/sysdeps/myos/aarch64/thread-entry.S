// A new thread starts here with the stack prepared by PrepareStack
// (thread.cpp): the entry, the TCB and the argument on top.
    .text
    .globl __mlibc_thread_entry
__mlibc_thread_entry:
    ldp x0, x1, [sp]
    ldr x2, [sp, #16]
    add sp, sp, #32
    bl __mlibc_thread_trampoline
    brk #0

    .section .note.GNU-stack,"",%progbits
