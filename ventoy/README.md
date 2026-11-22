# Ventoy

Ventoy has no driver folder, so the d-pad gets in through a tiny chainloader
that takes the place of the stick's `\EFI\BOOT\BOOTX64.EFI`. Everything lives
on the stick, so it works on any Claw you plug it into.

```
firmware → BOOTX64.EFI (claw-chainload) → \EFI\claw\claw-dpad.efi  (stays resident)
                                       → BOOTX64_ORIG.EFI (Ventoy's own loader) → Ventoy menu
```

- If the driver is missing or fails, the chainloader shows the error for five
  seconds and boots Ventoy anyway.
- Ventoy's loader is started from its real path, so it behaves exactly as if
  the firmware had started it. Its name must start with `BOOT`: Ventoy's shim
  only takes its route to GRUB (`fbx64.efi` → `grubx64_real.efi`) then.
- **Secure Boot must be off** to boot the stick in UEFI mode once this is
  installed, because the chainloader and driver are unsigned. Uninstall puts
  Ventoy's signed loader back.
- **Re-run the installer after updating Ventoy.** Ventoy's update rewrites the
  whole VTOYEFI partition, which removes the chainloader (the stick still boots,
  just without the d-pad).

## Install

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
deno task test         # unit tests; the mtools test runs when mtools and mkfs.fat exist
```
