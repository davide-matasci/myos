// A program's entry: main and the entry stack (argc, argv, envp, the
// auxiliary vector, as the kernel lays it out) to mlibc's startup, which
// parses the stack, sets up the TLS and runs the constructors.
    .text
    .globl _start
_start:
    lea main(%rip), %rdi
    mov %rsp, %rsi
    call __mlibc_entry
    ud2

    .section .note.GNU-stack,"",%progbits
