# Hardware inventory — Pop!_OS T540p baseline

Captured: 2026-06-30T09:22:19-04:00
Host: pop-os

## Summary commands

## uname -a

```text
$ uname -a
Linux pop-os 6.18.7-76061807-generic #202601231045~1778249322~24.04~b44a3c3 SMP PREEMPT_DYNAMIC Fri M x86_64 x86_64 x86_64 GNU/Linux
```

## os-release

```text
$ cat /etc/os-release
NAME="Pop!_OS"
VERSION="24.04 LTS"
ID=pop
ID_LIKE="ubuntu debian"
PRETTY_NAME="Pop!_OS 24.04 LTS"
VERSION_ID="24.04"
HOME_URL="https://pop.system76.com"
SUPPORT_URL="https://support.system76.com"
BUG_REPORT_URL="https://github.com/pop-os/pop/issues"
PRIVACY_POLICY_URL="https://system76.com/privacy"
VERSION_CODENAME=noble
UBUNTU_CODENAME=noble
LOGO=distributor-logo-pop-os
```

## lsblk -f

```text
$ lsblk -f
NAME   FSTYPE      FSVER LABEL UUID                                 FSAVAIL FSUSE% MOUNTPOINTS
sda                                                                                
├─sda1 vfat        FAT32       D9A4-783C                             721.2M    29% /boot/efi
├─sda2 vfat        FAT32       D9A4-7865                             896.8M    78% /recovery
├─sda3 crypto_LUKS 2           dded89bf-9e94-49be-a81f-8d5564272208                
└─sda4 swap        1           cdbc82f8-0d88-4253-a50e-5d6c91c07f9c                
zram0                                                                              [SWAP]
```

## findmnt

```text
$ findmnt
TARGET                                        SOURCE                                                   FSTYPE          OPTIONS
/                                             /dev/mapper/data-root                                    ext4            ro,nosuid,nodev,noatime,errors=remount-ro
├─/dev                                        udev                                                     devtmpfs        ro,nosuid,nodev,relatime,size=3929324k,nr_inodes=982331,mode=755,inode64
│ ├─/dev/pts                                  devpts                                                   devpts          ro,nosuid,nodev,noexec,relatime,gid=5,mode=620,ptmxmode=000
│ ├─/dev/shm                                  tmpfs                                                    tmpfs           ro,nosuid,nodev,inode64
│ ├─/dev/hugepages                            hugetlbfs                                                hugetlbfs       ro,nosuid,nodev,relatime,pagesize=2M
│ ├─/dev/mqueue                               mqueue                                                   mqueue          ro,nosuid,nodev,noexec,relatime
│ └─/dev                                      tmpfs                                                    tmpfs           rw,nosuid,nodev,relatime,mode=755,uid=1000,gid=1000,inode64
│   ├─/dev/null                               udev[/null]                                              devtmpfs        rw,nosuid,relatime,size=3929324k,nr_inodes=982331,mode=755,inode64
│   ├─/dev/zero                               udev[/zero]                                              devtmpfs        rw,nosuid,relatime,size=3929324k,nr_inodes=982331,mode=755,inode64
│   ├─/dev/full                               udev[/full]                                              devtmpfs        rw,nosuid,relatime,size=3929324k,nr_inodes=982331,mode=755,inode64
│   ├─/dev/random                             udev[/random]                                            devtmpfs        rw,nosuid,relatime,size=3929324k,nr_inodes=982331,mode=755,inode64
│   ├─/dev/urandom                            udev[/urandom]                                           devtmpfs        rw,nosuid,relatime,size=3929324k,nr_inodes=982331,mode=755,inode64
│   ├─/dev/tty                                udev[/tty]                                               devtmpfs        rw,nosuid,relatime,size=3929324k,nr_inodes=982331,mode=755,inode64
│   └─/dev/pts                                devpts                                                   devpts          rw,nosuid,noexec,relatime,mode=620,ptmxmode=666
├─/run                                        tmpfs                                                    tmpfs           ro,nosuid,nodev,noexec,relatime,size=800228k,mode=755,inode64
│ ├─/run/lock                                 tmpfs                                                    tmpfs           ro,nosuid,nodev,noexec,relatime,size=5120k,inode64
│ └─/run/user/1000                            tmpfs                                                    tmpfs           ro,nosuid,nodev,relatime,size=800224k,nr_inodes=200056,mode=700,uid=1000,gid=1000,inode64
│   ├─/run/user/1000/gvfs                     gvfsd-fuse                                               fuse.gvfsd-fuse ro,nosuid,nodev,relatime,user_id=1000,group_id=1000
│   └─/run/user/1000/doc                      portal                                                   fuse.portal     ro,nosuid,nodev,relatime,user_id=1000,group_id=1000
├─/sys                                        sysfs                                                    sysfs           ro,nosuid,nodev,noexec,relatime
│ ├─/sys/firmware/efi/efivars                 efivarfs                                                 efivarfs        ro,nosuid,nodev,noexec,relatime
│ ├─/sys/kernel/security                      securityfs                                               securityfs      ro,nosuid,nodev,noexec,relatime
│ ├─/sys/fs/cgroup                            cgroup2                                                  cgroup2         ro,nosuid,nodev,noexec,relatime,nsdelegate,memory_recursiveprot
│ ├─/sys/fs/pstore                            none                                                     pstore          ro,nosuid,nodev,noexec,relatime
│ ├─/sys/fs/bpf                               bpf                                                      bpf             ro,nosuid,nodev,noexec,relatime,mode=700
│ ├─/sys/kernel/debug                         debugfs                                                  debugfs         ro,nosuid,nodev,noexec,relatime
│ ├─/sys/kernel/tracing                       tracefs                                                  tracefs         ro,nosuid,nodev,noexec,relatime
│ ├─/sys/fs/fuse/connections                  fusectl                                                  fusectl         ro,nosuid,nodev,noexec,relatime
│ └─/sys/kernel/config                        configfs                                                 configfs        ro,nosuid,nodev,noexec,relatime
├─/proc                                       proc                                                     proc            ro,nosuid,nodev,noexec,relatime
│ ├─/proc/sys/fs/binfmt_misc                  systemd-1                                                autofs          rw,relatime,fd=32,pgrp=0,timeout=0,minproto=5,maxproto=5,direct,pipe_ino=10288
│ │ └─/proc/sys/fs/binfmt_misc                binfmt_misc                                              binfmt_misc     ro,nosuid,nodev,noexec,relatime
│ └─/proc                                     proc                                                     proc            rw,nosuid,nodev,noexec,relatime
├─/boot/efi                                   /dev/sda1                                                vfat            ro,nosuid,nodev,relatime,fmask=0077,dmask=0077,codepage=437,iocharset=iso8859-1,shortname=mixed,errors=remount-ro
├─/recovery                                   /dev/sda2                                                vfat            ro,nosuid,nodev,relatime,fmask=0077,dmask=0077,codepage=437,iocharset=iso8859-1,shortname=mixed,errors=remount-ro
├─/tmp                                        /dev/mapper/data-root[/tmp]                              ext4            rw,nosuid,nodev,noatime,errors=remount-ro
│ ├─/tmp/.git                                 tmpfs                                                    tmpfs           ro,nosuid,nodev,relatime,mode=555,uid=1000,gid=1000,inode64
│ ├─/tmp/.agents                              tmpfs                                                    tmpfs           ro,nosuid,nodev,relatime,mode=555,uid=1000,gid=1000,inode64
│ └─/tmp/.codex                               tmpfs                                                    tmpfs           ro,nosuid,nodev,relatime,mode=555,uid=1000,gid=1000,inode64
└─/home/jeremieroy/Desktop/Software           /dev/mapper/data-root[/home/jeremieroy/Desktop/Software] ext4            rw,nosuid,nodev,noatime,errors=remount-ro
  ├─/home/jeremieroy/Desktop/Software/.git    tmpfs                                                    tmpfs           ro,nosuid,nodev,relatime,mode=555,uid=1000,gid=1000,inode64
  ├─/home/jeremieroy/Desktop/Software/.agents tmpfs                                                    tmpfs           ro,nosuid,nodev,relatime,mode=555,uid=1000,gid=1000,inode64
  └─/home/jeremieroy/Desktop/Software/.codex  tmpfs                                                    tmpfs           ro,nosuid,nodev,relatime,mode=555,uid=1000,gid=1000,inode64
```

## bootctl status

```text
$ bootctl status
Failed to read "/boot/efi/EFI/systemd": Permission denied
Failed to open '/boot/efi//loader/loader.conf': Permission denied
[0mSystem:
      Firmware: UEFI 2.31 (Lenovo 0.8448)
 Firmware Arch: x64
   Secure Boot: disabled
  TPM2 Support: firmware only, driver unavailable
  Measured UKI: no
  Boot into FW: supported

[0mCurrent Boot Loader:
      Product: systemd-boot 255.4-1ubuntu8.15pop0~1778766128~24.04~85b5073
     Features: ✓ Boot counting
               ✓ Menu timeout control
               ✓ One-shot menu timeout control
               ✓ Default entry control
               ✓ One-shot entry control
               ✓ Support for XBOOTLDR partition
               ✓ Support for passing random seed to OS
               ✓ Load drop-in drivers
               ✓ Support Type #1 sort-key field
               ✓ Support @saved pseudo-entry
               ✓ Support Type #1 devicetree field
               ✓ Enroll SecureBoot keys
               ✓ Retain SHIM protocols
               ✓ Menu can be disabled
               ✓ Boot loader sets ESP information
          ESP: /dev/disk/by-partuuid/59c85b6f-250e-4007-b1e4-28e579329bd4
         File: └─/EFI/systemd/systemd-bootx64.efi

[0mRandom Seed:
 System Token: set
       Exists: no

[0mAvailable Boot Loaders on ESP:
          ESP: /boot/efi (/dev/disk/by-partuuid/59c85b6f-250e-4007-b1e4-28e579329bd4)
         File: (can't access /boot/efi: Permission denied)

[0mBoot Loaders Listed in EFI Variables:
        Title: Pop!_OS 24.04 LTS
           ID: 0x0016
       Status: active, boot-order
    Partition: /dev/disk/by-partuuid/59c85b6f-250e-4007-b1e4-28e579329bd4
         File: └─/EFI/systemd/systemd-bootx64.efi

[exit status: 1]
```

## sudo -n bootctl status

```text
$ sudo -n bootctl status
sudo: /etc/sudo.conf is owned by uid 65534, should be 0
sudo: The "no new privileges" flag is set, which prevents sudo from running as root.
sudo: If sudo is running in a container, you may need to adjust the container configuration to disable the flag.
[exit status: 1]
```

## parted -l

```text
$ parted -l
```

## sudo -n parted -l

```text
$ sudo -n parted -l
sudo: /etc/sudo.conf is owned by uid 65534, should be 0
sudo: The "no new privileges" flag is set, which prevents sudo from running as root.
sudo: If sudo is running in a container, you may need to adjust the container configuration to disable the flag.
[exit status: 1]
```

## lspci -nnk

```text
$ lspci -nnk
00:00.0 Host bridge [0600]: Intel Corporation Xeon E3-1200 v3/4th Gen Core Processor DRAM Controller [8086:0c04] (rev 06)
	Subsystem: Lenovo Xeon E3-1200 v3/4th Gen Core Processor DRAM Controller [17aa:2210]
	Kernel modules: ie31200_edac
00:01.0 PCI bridge [0604]: Intel Corporation Xeon E3-1200 v3/4th Gen Core Processor PCI Express x16 Controller [8086:0c01] (rev 06)
	Subsystem: Lenovo Xeon E3-1200 v3/4th Gen Core Processor PCI Express x16 Controller [17aa:2210]
	Kernel driver in use: pcieport
	Kernel modules: shpchp
00:02.0 VGA compatible controller [0300]: Intel Corporation 4th Gen Core Processor Integrated Graphics Controller [8086:0416] (rev 06)
	Subsystem: Lenovo 4th Gen Core Processor Integrated Graphics Controller [17aa:221e]
	Kernel driver in use: i915
	Kernel modules: i915
00:03.0 Audio device [0403]: Intel Corporation Xeon E3-1200 v3/4th Gen Core Processor HD Audio Controller [8086:0c0c] (rev 06)
	Subsystem: Lenovo Xeon E3-1200 v3/4th Gen Core Processor HD Audio Controller [17aa:2210]
	Kernel driver in use: snd_hda_intel
	Kernel modules: snd_hda_intel
00:14.0 USB controller [0c03]: Intel Corporation 8 Series/C220 Series Chipset Family USB xHCI [8086:8c31] (rev 04)
	Subsystem: Lenovo 8 Series/C220 Series Chipset Family USB xHCI [17aa:2210]
	Kernel driver in use: xhci_hcd
	Kernel modules: xhci_pci
00:16.0 Communication controller [0780]: Intel Corporation 8 Series/C220 Series Chipset Family MEI Controller #1 [8086:8c3a] (rev 04)
	Subsystem: Lenovo 8 Series/C220 Series Chipset Family MEI Controller [17aa:2210]
	Kernel driver in use: mei_me
	Kernel modules: mei_me
00:19.0 Ethernet controller [0200]: Intel Corporation Ethernet Connection I217-LM [8086:153a] (rev 04)
	Subsystem: Lenovo Ethernet Connection I217-LM [17aa:2210]
	Kernel driver in use: e1000e
	Kernel modules: e1000e
00:1a.0 USB controller [0c03]: Intel Corporation 8 Series/C220 Series Chipset Family USB EHCI #2 [8086:8c2d] (rev 04)
	Subsystem: Lenovo 8 Series/C220 Series Chipset Family USB EHCI [17aa:2210]
	Kernel driver in use: ehci-pci
	Kernel modules: ehci_pci
00:1b.0 Audio device [0403]: Intel Corporation 8 Series/C220 Series Chipset High Definition Audio Controller [8086:8c20] (rev 04)
	Subsystem: Lenovo 8 Series/C220 Series Chipset High Definition Audio Controller [17aa:2210]
	Kernel driver in use: snd_hda_intel
	Kernel modules: snd_hda_intel
00:1c.0 PCI bridge [0604]: Intel Corporation 8 Series/C220 Series Chipset Family PCI Express Root Port #1 [8086:8c10] (rev d4)
	Subsystem: Lenovo 8 Series/C220 Series Chipset Family PCI Express Root Port [17aa:2210]
	Kernel driver in use: pcieport
	Kernel modules: shpchp
00:1c.1 PCI bridge [0604]: Intel Corporation 8 Series/C220 Series Chipset Family PCI Express Root Port #2 [8086:8c12] (rev d4)
	Subsystem: Lenovo 8 Series/C220 Series Chipset Family PCI Express Root Port [17aa:2210]
	Kernel driver in use: pcieport
	Kernel modules: shpchp
00:1c.2 PCI bridge [0604]: Intel Corporation 8 Series/C220 Series Chipset Family PCI Express Root Port #3 [8086:8c14] (rev d4)
	Subsystem: Lenovo 8 Series/C220 Series Chipset Family PCI Express Root Port [17aa:2210]
	Kernel driver in use: pcieport
	Kernel modules: shpchp
00:1d.0 USB controller [0c03]: Intel Corporation 8 Series/C220 Series Chipset Family USB EHCI #1 [8086:8c26] (rev 04)
	Subsystem: Lenovo ThinkPad T540p [17aa:2210]
	Kernel driver in use: ehci-pci
	Kernel modules: ehci_pci
00:1f.0 ISA bridge [0601]: Intel Corporation QM87 Express LPC Controller [8086:8c4f] (rev 04)
	Subsystem: Lenovo QM87 Express LPC Controller [17aa:2210]
	Kernel driver in use: lpc_ich
	Kernel modules: lpc_ich
00:1f.2 SATA controller [0106]: Intel Corporation 8 Series/C220 Series Chipset Family 6-port SATA Controller 1 [AHCI mode] [8086:8c03] (rev 04)
	Subsystem: Lenovo 8 Series/C220 Series Chipset Family 6-port SATA Controller 1 [AHCI mode] [17aa:2210]
	Kernel driver in use: ahci
	Kernel modules: ahci
00:1f.3 SMBus [0c05]: Intel Corporation 8 Series/C220 Series Chipset Family SMBus Controller [8086:8c22] (rev 04)
	Subsystem: Lenovo 8 Series/C220 Series Chipset Family SMBus Controller [17aa:2210]
	Kernel driver in use: i801_smbus
	Kernel modules: i2c_i801
01:00.0 VGA compatible controller [0300]: NVIDIA Corporation GK208M [GeForce GT 730M] [10de:1290] (rev a1)
	Subsystem: Lenovo GK208M [GeForce GT 730M] [17aa:221e]
	Kernel driver in use: nouveau
	Kernel modules: nvidiafb, nouveau
03:00.0 Unassigned class [ff00]: Realtek Semiconductor Co., Ltd. RTS5227 PCI Express Card Reader [10ec:5227] (rev 01)
	Subsystem: Lenovo RTS5227 PCI Express Card Reader [17aa:2210]
	Kernel driver in use: rtsx_pci
	Kernel modules: rtsx_pci
04:00.0 Network controller [0280]: Intel Corporation Wireless 7260 [8086:08b2] (rev 83)
	Subsystem: Intel Corporation Dual Band Wireless-AC 7260 [Wilkins Peak 2] [8086:c270]
	Kernel driver in use: iwlwifi
	Kernel modules: iwlwifi
```

## lsusb

```text
$ lsusb
unable to initialize libusb: -99
[exit status: 1]
```

## lsusb -t

```text
$ lsusb -t
/:  Bus 001.Port 001: Dev 001, Class=root_hub, Driver=ehci-pci/3p, 480M
    |__ Port 001: Dev 002, If 0, Class=Hub, Driver=hub/6p, 480M
/:  Bus 002.Port 001: Dev 001, Class=root_hub, Driver=xhci_hcd/15p, 480M
    |__ Port 007: Dev 002, If 0, Class=Vendor Specific Class, Driver=[none], 12M
    |__ Port 011: Dev 007, If 0, Class=Wireless, Driver=btusb, 12M
    |__ Port 011: Dev 007, If 1, Class=Wireless, Driver=btusb, 12M
    |__ Port 012: Dev 008, If 0, Class=Video, Driver=uvcvideo, 480M
    |__ Port 012: Dev 008, If 1, Class=Video, Driver=uvcvideo, 480M
/:  Bus 003.Port 001: Dev 001, Class=root_hub, Driver=ehci-pci/3p, 480M
    |__ Port 001: Dev 002, If 0, Class=Hub, Driver=hub/8p, 480M
/:  Bus 004.Port 001: Dev 001, Class=root_hub, Driver=xhci_hcd/6p, 5000M
    |__ Port 005: Dev 002, If 0, Class=Hub, Driver=hub/4p, 5000M
```

## ip link

```text
$ ip link
Cannot open netlink socket: Operation not permitted
[exit status: 1]
```

## rfkill list

```text
$ rfkill list
rfkill: cannot open /dev/rfkill: No such file or directory
[exit status: 1]
```

## dmesg -T

```text
$ dmesg -T
dmesg: read kernel buffer failed: Operation not permitted
[exit status: 1]
```

## sudo -n dmesg -T

```text
$ sudo -n dmesg -T
sudo: /etc/sudo.conf is owned by uid 65534, should be 0
sudo: The "no new privileges" flag is set, which prevents sudo from running as root.
sudo: If sudo is running in a container, you may need to adjust the container configuration to disable the flag.
[exit status: 1]
```

## nvidia-smi

```text
$ nvidia-smi
environment: line 29: nvidia-smi: command not found
[exit status: 127]
```

## glxinfo -B

```text
$ glxinfo -B
environment: line 29: glxinfo: command not found
[exit status: 127]
```

## free -h

```text
$ free -h
               total        used        free      shared  buff/cache   available
Mem:           7.6Gi       3.6Gi       1.6Gi       376Mi       3.4Gi       4.1Gi
Swap:           11Gi          0B        11Gi
```

## lscpu

```text
$ lscpu
Architecture:                            x86_64
CPU op-mode(s):                          32-bit, 64-bit
Address sizes:                           39 bits physical, 48 bits virtual
Byte Order:                              Little Endian
CPU(s):                                  8
On-line CPU(s) list:                     0-7
Vendor ID:                               GenuineIntel
Model name:                              Intel(R) Core(TM) i7-4700MQ CPU @ 2.40GHz
CPU family:                              6
Model:                                   60
Thread(s) per core:                      2
Core(s) per socket:                      4
Socket(s):                               1
Stepping:                                3
CPU(s) scaling MHz:                      81%
CPU max MHz:                             3400.0000
CPU min MHz:                             800.0000
BogoMIPS:                                4789.00
Flags:                                   fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush dts acpi mmx fxsr sse sse2 ss ht tm pbe syscall nx pdpe1gb rdtscp lm constant_tsc arch_perfmon pebs bts rep_good nopl xtopology nonstop_tsc cpuid aperfmperf pni pclmulqdq dtes64 monitor ds_cpl vmx est tm2 ssse3 sdbg fma cx16 xtpr pdcm pcid sse4_1 sse4_2 movbe popcnt tsc_deadline_timer aes xsave avx f16c rdrand lahf_lm abm cpuid_fault epb pti ssbd ibrs ibpb stibp tpr_shadow flexpriority ept vpid ept_ad fsgsbase tsc_adjust bmi1 avx2 smep bmi2 erms invpcid xsaveopt dtherm ida arat pln pts vnmi md_clear flush_l1d
Virtualization:                          VT-x
L1d cache:                               128 KiB (4 instances)
L1i cache:                               128 KiB (4 instances)
L2 cache:                                1 MiB (4 instances)
L3 cache:                                6 MiB (1 instance)
NUMA node(s):                            1
NUMA node0 CPU(s):                       0-7
Vulnerability Gather data sampling:      Not affected
Vulnerability Ghostwrite:                Not affected
Vulnerability Indirect target selection: Not affected
Vulnerability Itlb multihit:             KVM: Mitigation: Split huge pages
Vulnerability L1tf:                      Mitigation; PTE Inversion; VMX conditional cache flushes, SMT vulnerable
Vulnerability Mds:                       Mitigation; Clear CPU buffers; SMT vulnerable
Vulnerability Meltdown:                  Mitigation; PTI
Vulnerability Mmio stale data:           Not affected
Vulnerability Old microcode:             Not affected
Vulnerability Reg file data sampling:    Not affected
Vulnerability Retbleed:                  Not affected
Vulnerability Spec rstack overflow:      Not affected
Vulnerability Spec store bypass:         Mitigation; Speculative Store Bypass disabled via prctl
Vulnerability Spectre v1:                Mitigation; usercopy/swapgs barriers and __user pointer sanitization
Vulnerability Spectre v2:                Mitigation; Retpolines; IBPB conditional; IBRS_FW; STIBP conditional; RSB filling; PBRSB-eIBRS Not affected; BHI Not affected
Vulnerability Srbds:                     Mitigation; Microcode
Vulnerability Tsa:                       Not affected
Vulnerability Tsx async abort:           Not affected
Vulnerability Vmscape:                   Mitigation; IBPB before exit to userspace
```

## ls -l /dev/disk/by-partlabel

```text
$ ls -l /dev/disk/by-partlabel
ls: cannot access '/dev/disk/by-partlabel': No such file or directory
[exit status: 2]
```

## ls -l /dev/disk/by-uuid

```text
$ ls -l /dev/disk/by-uuid
ls: cannot access '/dev/disk/by-uuid': No such file or directory
[exit status: 2]
```
