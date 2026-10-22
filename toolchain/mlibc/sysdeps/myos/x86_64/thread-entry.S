// A new thread starts here with the stack prepared by PrepareStack
// (thread.cpp): the entry, the TCB and the argument on top, 16-aligned
// above them. The kernel starts a thread as if called: its stack pointer
// is one word below the top it was given, where a call's return address
// would be.
    .text
    .globl __mlibc_thread_entry
__mlibc_thread_entry:
    add $8, %rsp
    pop %rdi
    pop %rsi
    pop %rdx
    pop %rcx
    call __mlibc_thread_trampoline
    ud2

    .section .note.GNU-stack,"",%progbits
