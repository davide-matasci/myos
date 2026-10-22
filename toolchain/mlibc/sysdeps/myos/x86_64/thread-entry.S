// A new thread starts here with the stack prepared by PrepareStack
// (thread.cpp): the entry, the TCB and the argument on top, 16-aligned
// above them.
    .text
    .globl __mlibc_thread_entry
__mlibc_thread_entry:
    pop %rdi
    pop %rsi
    pop %rdx
    pop %rcx
    call __mlibc_thread_trampoline
    ud2

    .section .note.GNU-stack,"",%progbits
