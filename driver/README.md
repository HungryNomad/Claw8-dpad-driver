# driver

Rust sources for `claw-dpad.efi` (the driver) and `claw-probe.efi` (the
diagnostic app). Build from the repository root; see the top-level README.

- `common/`: pure `no_std` logic with host unit tests: Claw USB IDs, Xbox 360
  (XInput) report parser, a minimal HID report-descriptor parser for DInput
  mode (hat switch + buttons), and the button-to-key mapper with auto-repeat.
  The key table is `KEYMAP` in `common/src/keys.rs`.
- `dpad/` → `claw-dpad.efi`: the boot-service driver.
  `cargo build --release --target x86_64-unknown-uefi -p claw-dpad --features verbose`
  builds one that prints what it is doing on screen.
- `probe/` → `claw-probe.efi`: one-shot app that records the controller's
  descriptors, current mode and raw reports per button, and saves
  `\claw-probe-log.txt` on the partition it was started from.

## How the driver works

1. The entry point installs a UEFI Driver Binding protocol and connects it to
   any MSI (`0x0DB0`) USB interface already present. The driver stays resident,
   so a controller that re-enumerates later is also picked up.
2. `Supported` accepts the Claw's gamepad interface: vendor class `FF/5D/01`
   (XInput, PID `0x1901`) or a non-boot HID interface whose report descriptor
   has a Gamepad/Joystick collection (DInput, PID `0x1902`). Boot keyboards and
   mice are left to the firmware.
3. `Start` polls the interrupt-IN endpoint with `UsbAsyncInterruptTransfer`,
   decodes each report, and turns new presses into keys. A 20 ms timer drives
   d-pad repeat (400 ms delay, 110 ms rate) and recovers from USB errors.
4. Keys are published through `EFI_SIMPLE_TEXT_INPUT(_EX)_PROTOCOL` on the
   interface's handle. The driver then checks that the console splitter took
   it into `ConIn`. If not, it adds the console-device tag and reconnects
   (OVMF needed this). As a last resort, it wraps `ConIn.ReadKeyStroke` and
   signals `ConIn.WaitForKey`. Only one path delivers keys, so presses are
   never doubled.

## Testing in QEMU

```sh
cargo build --release --target x86_64-unknown-uefi -p claw-dpad --features qemu-test
```

The `qemu-test` feature also accepts QEMU's `usb-tablet` (`0627:0001`) as a
"gamepad", so its left button acts as A. Boot OVMF with
`-device qemu-xhci -device usb-tablet`, `load claw-dpad.efi` in the UEFI shell,
then `pause` and click: the click continues the shell as one Enter. Never ship
this build.

## Open questions (answered by one probe run)

- Which mode the controller is in at boot (it keeps the last mode the OS set:
  XInput, DInput or Desktop), and whether it enumerates in firmware at all.
- Whether the Claw's XInput mode streams without an init message (the driver
  sends the standard Xbox 360 LED message just in case).
- DInput button numbering: the driver assumes 1 = A, 2 = B, 3 = X, 4 = Y,
  5/6 = LB/RB, 7 = View, 8 = Menu (`common/src/hid.rs`, `HID_BUTTONS`).
- Whether MSI's firmware console splitter picks the device up directly or
  needs the fallback.

USB IDs and the read-mode command come from the Linux `hid-msi` driver
(Derek J. Clark, v13, July 2026). The probe only sends the read-only "read
gamepad mode" command; nothing here changes the controller's mode or settings.
