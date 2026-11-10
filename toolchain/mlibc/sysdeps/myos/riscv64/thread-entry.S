// A new thread starts here with the stack prepared by PrepareStack
// (thread.cpp): the entry, the TCB and the argument on top.
    .text
    .globl __mlibc_thread_entry
__mlibc_thread_entry:
    // gp is the creator's: the kernel starts a thread with its creator's
    // registers (thread_spawn), and the global pointer is the program's,
    // which this code cannot name once it is in libc.so.
    ld a0, 0(sp)
    ld a1, 8(sp)
    ld a2, 16(sp)
    addi sp, sp, 32
    call __mlibc_thread_trampoline
    unimp

    .section .note.GNU-stack,"",%progbits
