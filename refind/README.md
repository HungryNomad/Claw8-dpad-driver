# rEFInd

rEFInd loads every driver in its `drivers_x64` folder before it shows the
menu, so installing is one file copy. rEFInd updates leave the folder alone.

## Install

1. Build (`cargo efi` from the repository root) or take `claw-dpad.efi` from a
   release.
2. Mount the ESP. On Windows, from an admin terminal: `mountvol S: /s`
   (unmount later with `mountvol S: /d`).
3. Copy `claw-dpad.efi` to `S:\EFI\refind\drivers_x64\`.
4. Reboot. The d-pad should move the rEFInd selection.

## If the d-pad does nothing

Run the probe and share the log:

1. Copy `claw-probe.efi` to `S:\EFI\claw-probe\claw-probe.efi` and add this to
   `S:\EFI\refind\refind.conf`:

   ```
   menuentry "Claw probe" {
       loader /EFI/claw-probe/claw-probe.efi
   }
   ```

2. Reboot, tap **Claw probe**, follow the on-screen prompts (about 80 seconds;
   no keyboard needed). It returns to rEFInd by itself.
3. The log is `S:\claw-probe-log.txt`.

The probe works with or without the driver installed. With the driver, it
leaves the gamepad interface alone and instead shows the keys the driver
delivers (`ConIn key: scan 0x0001` = Up, `0x0002` = Down, `char 0x000d` = Enter).

## Remove

Delete `EFI\refind\drivers_x64\claw-dpad.efi` (and the probe folder and menu
entry). If a broken driver ever stops rEFInd from starting, touch is
unaffected, but in the worst case use the firmware's boot menu to start
Windows Boot Manager, then delete the file from Windows as above.
