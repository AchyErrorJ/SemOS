# Catch the bug: kernel #PF with faulting RIP == 0.
# Failing iretq RSP = 0x10000178108 per qemu-int.log.

set osabi none
set architecture i386:x86-64
set print asm-demangle on
set disassembly-flavor intel
set pagination off
set confirm off

file kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64
target remote :1240
set language c

# Allow software breakpoints on addresses not yet mapped (memory only
# checked at insertion time, which we defer past kernel load).
set breakpoint pending on
set breakpoint auto-hw off

# Step 1: stop once at the kernel entry so the kernel is loaded into RAM
# and the .text/.bss are accessible. Kernel ELF entry = 0x92b2,
# load base = 0x100000000, so runtime entry = 0x1000092b2.
tbreak *0x1000092b2
echo === continuing to kernel entry ===\n
continue

echo === at kernel entry — verifying PF handler memory ===\n
x/4i 0x1000104fe

# Step 2: now the kernel is loaded — install the real probes.

# Software breakpoint at PF handler entry. RSP at entry points at the
# CPU-pushed error code; iret frame fields are above it.
break *0x1000104fe
commands
  silent
  printf "[bp] PF handler hit  err=0x%lx  faulting_RIP=0x%lx\n", \
    *(long long *)($rsp + 0), \
    *(long long *)($rsp + 8)
  if *(long long *)($rsp + 8) == 0
    printf ">>> CAUGHT RIP==0 at PF handler.\n"
    printf "    CS=0x%lx  RFLAGS=0x%lx  saved_RSP=0x%lx  saved_SS=0x%lx  CR2=0x%lx\n", \
      *(long long *)($rsp + 16), \
      *(long long *)($rsp + 24), \
      *(long long *)($rsp + 32), \
      *(long long *)($rsp + 40), \
      $cr2
    set $bad_rsp = *(long long *)($rsp + 32)
    printf "    Memory at saved_RSP-16 .. saved_RSP+48:\n"
    x/8gx ($bad_rsp - 16)
  end
  continue
end

# Watch the exact bad iret-frame RIP slot. Fires only when value
# becomes 0. Now safe — the BSS is mapped.
watch *(long long *)0x10000178108 if *(long long *)0x10000178108 == 0
commands
  silent
  printf "\n[wp] *(long *)0x10000178108 became 0\n"
  printf "  writer RIP=0x%lx  RSP=0x%lx\n", $rip, $rsp
  printf "  10 instructions before $rip:\n"
  x/10i $rip - 32
  printf "  64 bytes around 0x10000178108:\n"
  x/8gx 0x10000178100
  printf "\n"
  continue
end

continue
