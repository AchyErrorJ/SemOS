# Print info every time the page_fault_handler fires, then continue.
set osabi none
set architecture i386:x86-64
set print asm-demangle on
set disassembly-flavor intel
set pagination off
set confirm off
set logging file gdb-trace.log
set logging overwrite on
set logging on

file kernel-x86_64/target/x86_64-unknown-none/release/semantic-os-x86_64
target remote :1240

# Iret frame layout at page_fault_handler entry:
#   [rsp+0]  err     (8 bytes)
#   [rsp+8]  RIP     (8 bytes)
#   [rsp+16] CS      (low byte = selector)
#   [rsp+24] RFLAGS  (8 bytes)
#   [rsp+32] RSP     (8 bytes)
#   [rsp+40] SS      (8 bytes)
break *0x1000000d737
commands
  silent
  printf "PF: "
  x/6gx $rsp
  continue
end

continue
