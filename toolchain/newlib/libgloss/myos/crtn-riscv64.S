    .section .init,"ax"
    ld ra, 8(sp)
    ld s0, 0(sp)
    addi sp, sp, 16
    ret

    .section .fini,"ax"
    ld ra, 8(sp)
    ld s0, 0(sp)
    addi sp, sp, 16
    ret
