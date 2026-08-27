/* QEMU virt RAM starts at 0x40000000. Bare-metal boots place the DTB there,
   so load the kernel at the conventional ARM64 Image offset 0x40080000.
   QEMU -kernel of an ELF file honours PT_LOAD addresses and jumps to ENTRY. */
ENTRY(_start)

SECTIONS
{
    . = 0x40080000;

    .text : {
        KEEP(*(.text.boot))
        *(.text .text.*)
    }

    .rodata : ALIGN(8) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(8) {
        *(.data .data.*)
    }

    .bss : ALIGN(8) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(8);
        __bss_end = .;
    }

    . = ALIGN(16);
    . += 0x10000; /* 64 KiB stack */
    __stack_top = .;

    /DISCARD/ : {
        *(.eh_frame*)
        *(.comment)
        *(.note*)
    }
}
