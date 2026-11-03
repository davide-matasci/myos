/*
 * One read-only, executable page: the ELF header, the dynamic symbol
 * table musl's `__vdsosym` reads (DT_HASH, DT_SYMTAB, DT_STRTAB, the
 * versions), the code. File offsets are the addresses, so the file's
 * bytes copied to a page are the mapped image.
 */
SECTIONS
{
	. = SIZEOF_HEADERS;
	.hash : { *(.hash) } :text
	.gnu.hash : { *(.gnu.hash) }
	.dynsym : { *(.dynsym) }
	.dynstr : { *(.dynstr) }
	.gnu.version : { *(.gnu.version) }
	.gnu.version_d : { *(.gnu.version_d) }
	.gnu.version_r : { *(.gnu.version_r) }
	.dynamic : { *(.dynamic) } :text :dynamic
	.rodata : { *(.rodata .rodata.*) } :text
	.text : { *(.text .text.*) }
	/DISCARD/ : { *(.data .data.* .bss .bss.* .comment .note.* .eh_frame* .got .got.plt .rela.* .riscv.attributes) }
}

PHDRS
{
	text PT_LOAD FLAGS(5) FILEHDR PHDRS;
	dynamic PT_DYNAMIC FLAGS(4);
}
