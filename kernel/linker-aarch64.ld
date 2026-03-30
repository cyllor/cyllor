OUTPUT_FORMAT(elf64-littleaarch64)
ENTRY(_start)

PHDRS {
    text    PT_LOAD FLAGS(5); /* r-x */
    rodata  PT_LOAD FLAGS(4); /* r-- */
    data    PT_LOAD FLAGS(6); /* rw- */
}

SECTIONS {
    . = 0xFFFFFFFF80000000;

    .text : {
        *(.text .text.*)
    } :text

    . = ALIGN(4096);
    .rodata : {
        *(.rodata .rodata.*)
    } :rodata

    . = ALIGN(4096);
    .requests_start_marker : {
        KEEP(*(.requests_start_marker))
    } :rodata

    .requests : {
        KEEP(*(.requests))
    } :rodata

    .requests_end_marker : {
        KEEP(*(.requests_end_marker))
    } :rodata

    . = ALIGN(4096);
    .data : {
        *(.data .data.*)
    } :data

    .bss : {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        __bss_end = .;
    } :data

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment*)
    }
}
