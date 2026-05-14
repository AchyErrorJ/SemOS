set pagination off
set confirm off
set arch i386:x86-64
set osabi none
file F:/Software/ArmKernel3/kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64
set language c
target remote :1240

# Plain hardware watchpoint (no condition — keeps it HW-fast).
# In commands block: if value is now 0, dump and quit; else continue.
watch *(long*)0x1000015dfe8

commands
  silent
  set $val = *(long*)0x1000015dfe8
  if $val == 0
    printf "\n=========================================================\n"
    printf "  ZERO WRITE TO 0x1000015dfe8 CAUGHT\n"
    printf "=========================================================\n"
    printf "RIP = 0x%lx\n", $rip
    printf "RSP = 0x%lx\n", $rsp
    printf "RBP = 0x%lx\n", $rbp
    printf "RAX = 0x%lx  RBX = 0x%lx\n", $rax, $rbx
    printf "RCX = 0x%lx  RDX = 0x%lx\n", $rcx, $rdx
    printf "RSI = 0x%lx  RDI = 0x%lx\n", $rsi, $rdi
    printf "R8  = 0x%lx  R9  = 0x%lx\n", $r8,  $r9
    printf "R10 = 0x%lx  R11 = 0x%lx\n", $r10, $r11
    printf "R12 = 0x%lx  R13 = 0x%lx\n", $r12, $r13
    printf "R14 = 0x%lx  R15 = 0x%lx\n", $r14, $r15
    printf "\n--- 12 instructions BEFORE rip ---\n"
    x/12i $rip-24
    printf "\n--- next 6 instructions FROM rip ---\n"
    x/6i $rip
    printf "\n--- info symbol $rip ---\n"
    info symbol $rip
    printf "\n--- 16 quadwords ABOVE rsp (recent stack) ---\n"
    x/16gx $rsp
    printf "\n--- backtrace ---\n"
    bt 30
    printf "=========================================================\n"
    quit
  end
  cont
end

continue
