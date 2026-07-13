/* Bare-metal aarch64 link script.
 *
 * Linked as a POSITION-INDEPENDENT image at base 0, not at a fixed address.
 * m1n1 memcpy's the kernel to a 2 MiB-aligned address it chooses inside Apple
 * RAM (which starts at 0x10_0000_0000 on this machine) and jumps there — it
 * does not honour a link address. A fixed-address image would run with every
 * absolute pointer in it aimed at memory that does not exist.
 *
 * So: link at 0, keep the R_AARCH64_RELATIVE relocations, and let _start apply
 * them against wherever we actually landed. QEMU's -kernel loads us at
 * 0x4008_0000 and that keeps working, because the same relocation runs there.
 */
ENTRY(_start)

SECTIONS
{
    . = 0;

    /* Symbols the boot code takes the address of must live INSIDE a section, so
     * they are section-relative and reachable with adrp/add. A symbol defined
     * between sections is absolute, and a PIE cannot take its address without an
     * R_AARCH64_ABS64 the linker refuses to emit. */
    .text : {
        _kernel_start = .;
        _image_start = .;
        KEEP(*(.text._start))
        *(.text*)
    }

    /* Exception vector table — VBAR_EL1 requires 2 KiB (0x800) alignment. */
    .vectors ALIGN(0x800) : {
        KEEP(*(.vectors))
    }

    .rodata : { *(.rodata*) }
    .data   : { *(.data*) }

    /* Dynamic relocations. Every absolute address the compiler baked into the
     * image — literal pools, function pointers in statics, &_vectors — shows up
     * here as an R_AARCH64_RELATIVE entry for _start to fix up. */
    .rela.dyn : {
        __rela_start = .;
        *(.rela.dyn) *(.rela*)
        __rela_end = .;
    }

    .got    : { *(.got) *(.got.plt) }

    /* The 64 KiB boot stack lives inside .bss, so `_stack_top` is section-
     * relative (see above) and so the BSS-zero loop clears it for free. Safe:
     * the stack is empty until `bl kmain`, which runs after the loop. */
    .bss : {
        . = ALIGN(8);
        _bss_start = .;
        *(.bss*)
        *(COMMON)
        . = ALIGN(16);
        . = . + 0x10000;
        _stack_top = .;
        . = ALIGN(8);
        _bss_end = .;
        _image_end = .;
    }

    /DISCARD/ : {
        *(.comment) *(.note*) *(.eh_frame*)
        *(.dynsym) *(.dynstr) *(.hash) *(.gnu.hash) *(.dynamic) *(.interp)
    }
}
