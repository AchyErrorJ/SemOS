# T540p Pop!_OS + SemOS dual-boot resize runbook

Date: 2026-06-30
Host: ThinkPad T540p Pop!_OS 24.04 LTS

## Status before this runbook

SemOS already boots from the internal Pop!_OS ESP without repartitioning:

- Pop!_OS remains installed on the internal SSD.
- SemOS boot files were copied to the existing ESP.
- A firmware boot entry named `SemOS` was added.
- SemOS was boot-tested from internal SSD and reached the shell.

This runbook is **only** for creating raw disk space for later SemOS storage work:

- `SEMOS_SYSROOT`: raw, unformatted partition used by the current sysroot blob code.
- Future semantic-object storage: currently no stable on-disk format; leave space unallocated unless/until an installer/formatter exists.

## Important warnings

Do **not** run this from the live Pop!_OS install.

The current Pop!_OS root is mounted from the encrypted/LVM stack. ext4 cannot be shrunk while mounted, and SemOS cannot resize it either. Run these steps only from a Pop!_OS live USB / installer demo environment.

Back up anything important before resizing. If a command errors or output looks wrong, stop and inspect before continuing.

Never write a whole-disk SemOS image to `/dev/sda`; that would destroy Pop!_OS.

## Current disk stack observed on 2026-06-30

Internal disk:

```text
/dev/sda 238.5G SATA MTFDDAK256TBN-1AR1ZABHA
├─sda1 1022M vfat        /boot/efi
├─sda2 4G    vfat        /recovery
├─sda3 229.5G crypto_LUKS
└─sda4 4G    swap
```

Root storage stack:

```text
/dev/sda3
  → LUKS2 mapper: cryptdata
    → LVM PV
      → VG data
        → LV root
          → /dev/mapper/data-root ext4 /
```

Sysfs geometry from the installed system:

```text
sector size:             512 logical / 4096 physical
sda total sectors:       500118192
sda3 start sector:       10485760
sda3 size sectors:       481239727
cryptdata payload:       481206959 sectors
LVM LV data-root:        481198080 sectors
LUKS header offset:      32768 sectors = 16 MiB
sda4 start sector:       491725488
```

Because this is **LUKS + LVM**, GParted alone is not enough. GParted does not shrink LVM physical volumes. Use the CLI stack-shrink sequence below.

## Target layout

This runbook shrinks Pop!_OS root to approximately 150 GiB and creates a 4 GiB raw sysroot partition:

```text
sda1  ESP                         existing, unchanged
sda2  Pop recovery                existing, unchanged
sda3  Pop!_OS LUKS/LVM root       shrunk
sda5  SEMOS_SYSROOT               4 GiB raw, unformatted
...   future SemOS object space   leave unallocated for now
sda4  swap                        existing, unchanged at disk end
```

The `SEMOS_SYSROOT` partition must be named exactly:

```text
SEMOS_SYSROOT
```

Do **not** format it. The current SemOS kernel reads/writes a raw `SEMSYSR1` blob there.

## Live USB preparation

Boot the Pop!_OS ISO from Ventoy or a dedicated Pop!_OS USB stick.

Choose the live/demo environment, not Install.

Open a terminal.

Check required tools:

```sh
command -v cryptsetup lvs vgs pvs lvreduce pvresize e2fsck resize2fs parted lsblk
```

If any LVM tool is missing, install it in the live environment:

```sh
sudo apt update
sudo apt install -y lvm2 cryptsetup parted e2fsprogs
```

## Step 0 — identify the disk

```sh
lsblk -o NAME,SIZE,MODEL,TYPE,FSTYPE
```

Find the ~238G internal disk. On 2026-06-30 it was `/dev/sda`.

Set variables, adjusting `DISK` if necessary:

```sh
DISK=/dev/sda
P3=${DISK}3
echo "DISK=$DISK  P3=$P3"
lsblk -o NAME,SIZE,FSTYPE "$DISK"
```

Before continuing, confirm:

- `$DISK` is the internal 238G SSD, not the Ventoy/USB stick.
- `$P3` is the ~229G `crypto_LUKS` partition.

## Step 1 — back up the LUKS header

Write the header backup somewhere off the internal SSD. This example writes to the live user's home first; copy it to another USB/cloud if possible.

```sh
sudo cryptsetup luksHeaderBackup "$P3" --header-backup-file ~/cryptdata-header.img
ls -lh ~/cryptdata-header.img
```

## Step 2 — unlock LUKS and activate LVM

```sh
sudo cryptsetup open "$P3" cryptdata
sudo vgchange -ay data
sudo lvs
```

Expected: VG `data`, LV `root`.

If the VG name is not `data`, stop and inspect `sudo vgs`/`sudo lvs` before continuing.

## Step 3 — filesystem check

```sh
sudo e2fsck -f /dev/data/root
```

This must end cleanly. If it reports unfixable errors, stop.

## Step 4 — shrink ext4 and the logical volume to 150 GiB

```sh
sudo lvreduce --resizefs -L 150G /dev/data/root
```

Confirm when prompted.

This command shrinks the ext4 filesystem first, then shrinks the LVM logical volume. Do not interrupt it.

## Step 5 — shrink the LVM physical volume to 152 GiB

```sh
sudo pvresize --setphysicalvolumesize 152G /dev/mapper/cryptdata
sudo pvs -o pv_name,pv_size,pv_free
```

If `pvresize` complains that extents cannot be relocated or the PV cannot be shrunk, stop. It may require `pvmove` before retrying.

## Step 6 — shrink the LUKS mapping

For the 152 GiB target, the LUKS payload size is:

```text
152 GiB = 318767104 sectors of 512 bytes
```

Run:

```sh
sudo cryptsetup resize cryptdata --size 318767104
```

## Step 7 — deactivate and close

```sh
sudo vgchange -an data
sudo cryptsetup close cryptdata
```

Confirm no mapper remains active for `cryptdata`:

```sh
lsblk -o NAME,SIZE,FSTYPE "$DISK"
```

## Step 8 — shrink partition 3 and create SEMOS_SYSROOT

Print the current table:

```sh
sudo parted "$DISK" unit GiB print
```

Shrink partition 3 and create a 4 GiB raw partition:

```sh
sudo parted -s "$DISK" unit GiB resizepart 3 158
sudo parted -s "$DISK" unit GiB mkpart SEMOS_SYSROOT 158 162
sudo parted "$DISK" unit GiB print
```

Check:

- partition 3 ends at about `158GiB`,
- the new partition is named `SEMOS_SYSROOT`,
- the new partition is not formatted,
- swap remains at the end of the disk,
- space from about `162GiB` to swap is unallocated.

If the new partition number is not `5`, that is fine. SemOS searches by GPT partition name, not number.

## Step 9 — refresh partition table and verify label

```sh
sudo partprobe "$DISK"
lsblk -o NAME,SIZE,PARTLABEL,FSTYPE "$DISK"
```

Expected: one partition has `PARTLABEL` exactly `SEMOS_SYSROOT` and no filesystem.

## Step 10 — reboot into Pop!_OS and verify

```sh
sudo reboot
```

Boot Pop!_OS.

Then verify:

```sh
df -h /
lsblk -o NAME,SIZE,PARTLABEL,FSTYPE /dev/sda
```

Expected:

- `/` is about 150 GiB,
- Pop!_OS boots normally,
- `SEMOS_SYSROOT` exists,
- SemOS firmware entry still boots via F12.

## Expected SemOS behavior before flashing a sysroot blob

After creating an empty `SEMOS_SYSROOT`, SemOS may log that it found the partition but did not find `SEMSYSR1` magic. That is expected until the sysroot blob is packed/flashed.

The current safety behavior in `kernel-core/src/sysroot_blob.rs`:

- no GPT: legacy raw LBA0 mode,
- GPT + `SEMOS_SYSROOT`: use that partition's first LBA,
- GPT + no `SEMOS_SYSROOT`: refuse to touch LBA0.

## Future sysroot blob flow

Later, once the sysroot files are ready:

1. Pack sysroot metadata/rlibs into a raw blob with:

   ```sh
   python tools/pack-sysroot-blob.py sysroot.img NAME1=FILE1 [NAME2=FILE2 ...]
   ```

2. Put `SYSROOT.IMG` on a FAT USB stick.
3. Boot SemOS with that USB attached.
4. Invoke the SemOS sysroot flashing path (`SYS_FLASH_SYSROOT` / shell wrapper when available).
5. Confirm logs:

   ```text
   [sysroot] using SEMOS_SYSROOT partition at LBA ...
   [sysroot] blob found: N file(s)
   ```

Do not flash sysroot unless the partition label has been verified.

## If keeping encryption feels unnecessary

If this Pop!_OS install is disposable, an alternative is to reinstall Pop!_OS with a simpler manual layout:

```text
ESP                 1 GiB
Pop!_OS root        120–160 GiB, unencrypted ext4 or simpler encryption plan
Pop swap            4–8 GiB
SEMOS_SYSROOT       4 GiB raw/unformatted
future SemOS area   unallocated
```

That is simpler than shrinking LUKS+LVM, but it loses the current Pop!_OS install unless backed up.
