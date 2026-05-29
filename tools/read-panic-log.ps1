#requires -Version 5
<#
.SYNOPSIS
  Read a SemOS panic dump from a block device or disk image.

.DESCRIPTION
  After a kernel panic, SemOS writes a 65 KiB snapshot (header + scrollback
  ring) to the last 130 sectors of the first available block device — see
  kernel-x86_64/src/panic_dump.rs.

  This script reads that area from a raw disk OR a QEMU disk image file using
  only built-in PowerShell — no third-party tool (HxD etc.) required. The
  scrollback content is plain ASCII; the script prints it directly.

.PARAMETER Path
  Path to the device or image file. Examples:
    \\.\PhysicalDrive2                  (raw physical drive — admin shell)
    F:\Software\ArmKernel3\sata.img     (QEMU disk image)

.PARAMETER SectorSize
  Sector size of the underlying device. Defaults to 512. Override if the
  device reports something else (some NVMe drives do 4096).

.EXAMPLE
  .\tools\read-panic-log.ps1 -Path F:\Software\ArmKernel3\sata.img

.EXAMPLE
  # From an elevated PowerShell — replace the drive number with whatever
  # `Get-Disk` shows for the T540's SATA SSD.
  .\tools\read-panic-log.ps1 -Path \\.\PhysicalDrive2
#>
param(
    [Parameter(Mandatory=$true)] [string] $Path,
    [int] $SectorSize = 512
)

$ErrorActionPreference = 'Stop'

# Open the device/file. Raw \\.\PhysicalDriveN requires Admin; image files
# don't. ReadWrite share so we don't fight other openers (we don't write here).
$fs = [System.IO.File]::Open($Path, 'Open', 'Read', 'ReadWrite')
try {
    # For a regular file, Length is the file size. For a raw device handle,
    # Length isn't always reliable — fall back to a Seek-to-End trick.
    $size = $fs.Length
    if ($size -le 0) {
        $size = $fs.Seek(0, 'End')
        $fs.Seek(0, 'Begin') | Out-Null
    }
    if ($size -le 0) {
        throw "Could not determine size of $Path"
    }
    # Last 130 sectors = 1 header + 128 scrollback + 1 slack.
    $totalSectors = 130
    $dumpBytes = $totalSectors * $SectorSize
    if ($size -lt $dumpBytes) {
        throw "$Path is too small (${size}B) for a 130-sector dump area."
    }
    $start = $size - $dumpBytes
    $fs.Seek($start, 'Begin') | Out-Null
    $buf = New-Object byte[] $dumpBytes
    $read = $fs.Read($buf, 0, $dumpBytes)
    if ($read -lt $dumpBytes) {
        Write-Warning "Short read: wanted $dumpBytes B, got $read B."
    }
} finally {
    $fs.Close()
}

# Verify magic.
$magic = [System.Text.Encoding]::ASCII.GetString($buf, 0, 8)
if ($magic -ne 'PANICLOG') {
    Write-Error "No PANICLOG magic at last sector (got '$magic'). Either no panic was dumped to this device, or the dump area is somewhere else."
    return
}

$version = [BitConverter]::ToUInt32($buf, 8)
$tick    = [BitConverter]::ToUInt64($buf, 12)
$sbLen   = [BitConverter]::ToUInt32($buf, 20)
$rLen    = [BitConverter]::ToUInt32($buf, 24)
$reason  = [System.Text.Encoding]::ASCII.GetString($buf, 28, $rLen)

Write-Host "SemOS panic dump (version $version)" -ForegroundColor Yellow
$secs = [math]::Round($tick / 100, 1)
Write-Host "Tick at panic: $tick (~${secs}s since boot)"
Write-Host ""
Write-Host "Panic reason:" -ForegroundColor Yellow
Write-Host "  $reason"
Write-Host ""
Write-Host "--- Scrollback ($sbLen B) ---" -ForegroundColor Yellow
if ($sbLen -gt 0) {
    $sbStart = $SectorSize
    $sbBytes = [Math]::Min([int]$sbLen, ($totalSectors - 2) * $SectorSize)
    [System.Text.Encoding]::ASCII.GetString($buf, $sbStart, $sbBytes)
}
