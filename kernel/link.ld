/* Higher-half kernel for the Limine protocol (x86_64 and aarch64).
   Limine maps PT_LOAD segments at these VAs (>= 0xffffffff80000000). */
ENTRY(_start)

PHDRS
{
    text   PT_LOAD FLAGS(5); /* RX */
    rodata PT_LOAD FLAGS(4); /* R  */
    data   PT_LOAD FLAGS(6); /* RW */
}

SECTIONS
{
    . = 0xffffffff80000000;

    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    } :rodata

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
        KEEP(*(.limine_requests_start))
        KEEP(*(.limine_requests))
        KEEP(*(.limine_requests_end))
    } :data

    .bss : {
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
    } :data

    . = ALIGN(CONSTANT(MAXPAGESIZE));
    _end = .;

    /DISCARD/ : {
        *(.eh_frame*)
        *(.comment)
        *(.note*)
        *(.interp)
    }
}
