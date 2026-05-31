/* Linker script for Semantic OS user programs.
 * Layout matches kernel-x86_64::paging::user_layout::USER_CODE_BASE = 0x400000
 * and kernel-core's ELF loader, which expects ET_EXEC with PT_LOAD segments.
 *
 * Identical to semos-cc/link.ld; semos-rustc has the same Ring-3 layout
 * constraints (non-PIE ET_EXEC; entry at _start; kernel maps PT_LOADs
 * with R/W/X from p_flags).
 */

ENTRY(_start)

SECTIONS
{
    . = 0x400000;

    .text : {
        *(.text._start)
        *(.text .text.*)
    }

    .rodata : ALIGN(0x1000) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(0x1000) {
        *(.data .data.*)
    }

    .bss : ALIGN(0x1000) {
        *(.bss .bss.*)
        *(COMMON)
    }

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note*)
        *(.comment)
        *(.gnu.build-id)
    }
}
