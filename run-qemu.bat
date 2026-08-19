@echo off
REM QEMU wrapper for the SemOS M14 kernel image.
REM Usage: run-qemu.bat [bios|uefi]
REM Defaults to BIOS boot and captures serial output to serial.log.

set MODE=%1
if "%MODE%"=="" set MODE=bios

set QEMU="C:\Program Files\qemu\qemu-system-x86_64.exe"

if "%MODE%"=="uefi" (
    set IMG=F:\Software\Semos\kernel-x86_64\target\x86_64-unknown-none\release\semantic-os-x86_64.img
) else (
    set IMG=F:\Software\Semos\kernel-x86_64\target\x86_64-unknown-none\release\semantic-os-x86_64-bios.img
)

if not exist %IMG% (
    echo Image not found: %IMG%
    echo Build it first with: cd x86_64-runner ^&^& cargo run --release
    exit /b 1
)

echo Booting SemOS (%MODE%) from %IMG%
%QEMU% -cpu max ^
  -drive format=raw,file=%IMG% ^
  -m 1024M ^
  -serial file:serial.log ^
  -display none ^
  -no-reboot
