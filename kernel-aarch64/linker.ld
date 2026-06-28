/* Bare-metal aarch64 link script for QEMU `-M virt`.
 * QEMU loads an ELF passed via `-kernel` at its link address; for arm64 virt
 * the convention is RAM base 0x4000_0000, kernel at 0x4008_0000. */
ENTRY(_start)

SECTIONS
{
    . = 0x40080000;

    .text : {
        KEEP(*(.text._start))
        *(.text*)
    }

    /* Exception vector table — VBAR_EL1 requires 2 KiB (0x800) alignment. */
    .vectors ALIGN(0x800) : {
        KEEP(*(.vectors))
    }

    .rodata : { *(.rodata*) }
    .data   : { *(.data*) }

    .bss : {
        . = ALIGN(8);
        _bss_start = .;
        *(.bss*)
        *(COMMON)
        . = ALIGN(8);
        _bss_end = .;
    }

    /* 64 KiB boot stack, grows down from _stack_top. */
    . = ALIGN(16);
    . = . + 0x10000;
    _stack_top = .;

    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) }
}
