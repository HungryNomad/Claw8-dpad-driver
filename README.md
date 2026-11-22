# Claw8-dpad-driver

Lets the MSI Claw 8 AI+'s built-in controller drive UEFI boot menus like a
keyboard. One small UEFI driver, `claw-dpad.efi`, with no firmware or boot
manager patching, so firmware updates leave it alone.

> **Untested on hardware.** Everything here builds clean and passes end-to-end
> tests in QEMU with OVMF firmware using a stand-in USB device, but it has not
> yet run on a real Claw. Secure Boot must be off (the files are unsigned).

| Button     | Key                       | In a boot menu          |
| ---------- | ------------------------- | ----------------------- |
| D-pad      | Arrows (repeat when held) | Move                    |
| A, Start   | Enter                     | Boot the selected entry |
| B          | Esc                       | Back / cancel           |
| Y          | F2                        | Entry options (rEFInd)  |
| LB / RB    | Home / End                | First / last entry      |

## Where to use it

- [`refind/`](refind/README.md): drop the driver into rEFInd's `drivers_x64`.

## Layout

- [`driver/`](driver/README.md): the Rust sources, shared by every boot target.
  - `common/`: platform-independent parsing and key mapping (host tests).
  - `dpad/` → `claw-dpad.efi`: the boot-service driver.
  - `probe/` → `claw-probe.efi`: diagnostic app that logs how the controller
    looks to firmware.

## Build

```sh
rustup target add x86_64-unknown-uefi
cargo test -p claw-common   # host tests
cargo efi                   # release build of claw-dpad.efi and claw-probe.efi
```

Output: `target/x86_64-unknown-uefi/release/*.efi`.
