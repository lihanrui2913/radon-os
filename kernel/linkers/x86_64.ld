OUTPUT_FORMAT(elf64-x86-64)

ENTRY(_kernel_start)

PHDRS
{
    text    PT_LOAD;
    rodata  PT_LOAD;
    data    PT_LOAD;
}

SECTIONS
{
    . = 0xffffffff80000000;

    __start = .;

    .text : {
        __text_start = .;
        *(.text .text.*)
	. = ALIGN(4096);
        __text_end = .;
    } :text

    PROVIDE (__etext = .);

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .rodata : {
        __rodata_start = .;
        *(.rodata .rodata.*)
	. = ALIGN(4096);
        __rodata_end = .;
    } :rodata

    . = ALIGN(8);
    PROVIDE(__eh_frame = .);
    .eh_frame : {
        KEEP (*(.eh_frame)) *(.eh_frame.*)
    } :rodata

    . = ALIGN(CONSTANT(MAXPAGESIZE));

    .data : {
        *(.data .data.*)

        KEEP(*(.requests_start_marker))
        KEEP(*(.requests))
        KEEP(*(.requests_end_marker))
    } :data

    .bss : {
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        *(COMMON)
    } :data

    __end = .;

    /DISCARD/ : {
        *(.note .note.*)
    }
}
