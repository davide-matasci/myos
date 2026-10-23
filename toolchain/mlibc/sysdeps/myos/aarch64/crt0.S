// A program's entry: main and the entry stack to mlibc's startup (the
// kernel's argc and argv in x0 and x1 are on the stack too).
    .text
    .globl _start
_start:
    adrp x0, main
    add x0, x0, :lo12:main
    mov x1, sp
    bl __mlibc_entry
    brk #0

    .section .note.GNU-stack,"",%progbits
