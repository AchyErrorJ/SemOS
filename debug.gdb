# GDB script — break at iretq inside timer_interrupt_handler and at page_fault_handler.
set osabi none
set architecture i386:x86-64
set print asm-demangle on
set disassembly-flavor intel
set pagination off
set confirm off
set print pretty on

file kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64
target remote :1237

# iretq inside timer_interrupt_handler at runtime address (kernel_base + 0xebb1)
break *0x1000000ebb1

# page_fault_handler entry — we want to catch any fault before iretq
break *0x1000000e790

echo \n=== Continuing ===\n
continue

echo \n=== STOPPED #1 ===\n
echo \n--- Where ---\n
where 5
echo \n--- Registers ---\n
info registers
echo \n--- Stack contents (RSP..RSP+96) ---\n
x/12gx $rsp
echo \n--- 8 instructions at RIP ---\n
x/8i $rip

echo \n=== Continuing ===\n
continue

echo \n=== STOPPED #2 ===\n
echo \n--- Where ---\n
where 5
echo \n--- Registers ---\n
info registers
echo \n--- Stack contents (RSP..RSP+96) ---\n
x/12gx $rsp
echo \n--- 8 instructions at RIP ---\n
x/8i $rip
