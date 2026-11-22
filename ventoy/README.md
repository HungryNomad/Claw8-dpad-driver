# Ventoy

Ventoy has no driver folder, so the d-pad gets in through a tiny chainloader
that starts the driver, then Ventoy. Everything lives on the stick, so it works
on any Claw you plug it into. There are two ways to get it there:

- **Patched Ventoy package** (easiest): download `ventoy-<version>-claw-windows.zip`
  or `-linux.tar.gz` from this repository's [Releases](https://github.com/HungryNomad/Claw8-dpad-driver/releases) and use
  it exactly like Ventoy, for Install and Update. The d-pad survives Ventoy's
  updates as long as you update from a patched package.
- **Stick installer**: adds the d-pad to a stick you already have. Re-run it
  after each Ventoy update.

**Secure Boot must be off** either way, because the chainloader and driver are
unsigned.

## Patched packages

A [GitHub Action](../.github/workflows/ventoy-release.yml) checks for a new
[Ventoy release](https://github.com/ventoy/Ventoy/releases) every Monday. For
each new version it builds the .efi files from source, patches the Windows and
Linux packages, boots each one in QEMU with OVMF in both of Ventoy's Secure
Boot support modes, and publishes a release tagged `ventoy-<version>`. A
version that already has a release is skipped. It can also be run by hand
(Actions → Patched Ventoy release → Run workflow), with an optional version.
Pull requests run everything except the release and keep the result as an
artifact.

Only the boot image `ventoy/ventoy.disk.img.xz` changes. It is the VTOYEFI
partition that Ventoy writes to the stick on Install and Update:

```
/EFI/BOOT/grubx64_real.efi   claw-chainload (was Ventoy's GRUB)
/EFI/claw/grubx64.efi        Ventoy's GRUB, moved
/EFI/claw/claw-dpad.efi      the driver
```

Replacing GRUB rather than `BOOTX64.EFI` keeps the d-pad in both of Ventoy's
modes:

- Secure Boot support on: firmware → shim (`BOOTX64.EFI`) → `fbx64.efi` →
  claw-chainload → GRUB.
- Secure Boot support off: Ventoy2Disk copies `grubx64_real.efi` (now
  claw-chainload) to `BOOTX64.EFI` itself, so firmware → claw-chainload → GRUB.

Each package also gains `CLAW-DPAD-README.txt` (what changed, links to both
sources) and `LICENSE-Ventoy.txt` (Ventoy is GPLv3).

To patch packages yourself (Linux or WSL; needs Deno 2, `xz`, mtools,
`zip`/`unzip` and GNU `tar`), build the .efi files with `cargo efi`, then:

```sh
cd ventoy/installer
deno task package --license COPYING ventoy-1.1.18-windows.zip ventoy-1.1.18-linux.tar.gz
deno task smoke out/ventoy-1.1.18-claw-linux.tar.gz   # needs qemu-system-x86_64 and OVMF
```

The patched packages land in `out/`. Set `SOURCE_DATE_EPOCH` for
byte-identical output. The smoke test lays the image out the way Ventoy2Disk
does, boots it, and passes when Ventoy's menu is on screen with no error on the
serial console; screenshots go to `shots/`.

## Stick installer

The installer puts the chainloader at the stick's `\EFI\BOOT\BOOTX64.EFI`:

```
firmware → BOOTX64.EFI (claw-chainload) → \EFI\claw\claw-dpad.efi  (stays resident)
                                       → BOOTX64_ORIG.EFI (Ventoy's own loader) → Ventoy menu
```

- If the driver is missing or fails, the chainloader shows the error for five
  seconds and boots Ventoy anyway.
- Ventoy's loader is started from its real path, so it behaves exactly as if
  the firmware had started it. Its name must start with `BOOT`: Ventoy's shim
  only takes its route to GRUB (`fbx64.efi` → `grubx64_real.efi`) then.
- Uninstall puts Ventoy's signed loader back.
- **Re-run the installer after updating Ventoy** from an unpatched package.
  Ventoy's update rewrites the whole VTOYEFI partition, which removes the
  chainloader (the stick still boots, just without the d-pad).
- A stick made from a patched package already has the d-pad; the installer
  says so and leaves it alone.

### Install

Needs [Deno](https://deno.com) 2 and the two .efi files: build them with
`cargo efi` from the repository root (the installer finds them in `target/`),
or put `claw-chainload.efi` and `claw-dpad.efi` in `ventoy/installer/dist/`.

```sh
cd ventoy/installer
deno task install      # or: deno task status, deno task uninstall
```

On Windows run it from an admin terminal. The installer finds the stick's
VTOYEFI partition by itself:

- **Windows:** uses its drive letter, or assigns one for the duration and
  removes it afterwards. If Windows won't mount the partition and
  [mtools](https://www.gnu.org/software/mtools/) is on the `PATH`, it edits the
  raw disk with mtools instead.
- **Linux:** uses the mount point, or mtools on the partition device (needs
  root, e.g. `sudo`).
- **macOS:** expects it mounted at `/Volumes/VTOYEFI`.

Options:

| Option             | Meaning                                                   |
| ------------------ | --------------------------------------------------------- |
| `--efi <dir>`      | VTOYEFI is mounted at `<dir>`                             |
| `--image <path>`   | disk, partition or image file to edit with mtools         |
| `--offset <bytes>` | VTOYEFI's offset in `--image` (default: partition table)  |
| `--loader <file>`  | path to `claw-chainload.efi`                              |
| `--driver <file>`  | path to `claw-dpad.efi`                                   |
| `--backup <dir>`   | where the stick's original loader is saved (`./backup`)   |
| `--force`          | install even if the partition doesn't look like Ventoy    |

What install does, in an order that leaves a bootable stick at every step:

1. Copies `claw-dpad.efi` to `\EFI\claw\`.
2. Saves Ventoy's `BOOTX64.EFI` to the backup folder on your computer, copies
   it to `\EFI\BOOT\BOOTX64_ORIG.EFI` on the stick and reads it back.
3. Overwrites `\EFI\BOOT\BOOTX64.EFI` with the chainloader and reads it back.

Running it again updates the files and keeps the original. Uninstall copies
the original back and deletes everything it added. If the stick ever ends up
without `BOOTX64_ORIG.EFI`, copy the saved file from the backup folder to
`\EFI\BOOT\BOOTX64.EFI`, or let Ventoy's own Update rewrite the partition.

## Develop

```sh
cd ventoy/installer
deno task test         # unit tests; mtools tests run when mtools, mkfs.fat, xz and zip exist
```
