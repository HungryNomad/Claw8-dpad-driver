# Claw8-dpad-driver

Lets the MSI Claw 8 AI+'s built-in controller drive UEFI boot menus like a
keyboard. One small UEFI driver, `claw-dpad.efi`, with no firmware or boot
manager patching, so firmware updates leave it alone.

> **Untested on hardware.** Everything here builds clean and passes end-to-end
> tests in QEMU with OVMF firmware using a stand-in USB device (including the
> real Ventoy 1.1.18 boot chain), but it has not yet run on a real Claw.
> Secure Boot must be off (the files are unsigned).

| Button     | Key                       | In a boot menu          |
| ---------- | ------------------------- | ----------------------- |
| D-pad      | Arrows (repeat when held) | Move                    |
| A, Start   | Enter                     | Boot the selected entry |
| B          | Esc                       | Back / cancel           |
| Y          | F2                        | Entry options (rEFInd)  |
| LB / RB    | Home / End                | First / last entry      |

## Where to use it

- [`refind/`](refind/README.md): drop the driver into rEFInd's `drivers_x64`.
- [`ventoy/`](ventoy/README.md): a chainloader loads the driver, then Ventoy.
  Easiest: download a patched Ventoy package from this repository's
  [Releases](../../releases) and use it exactly like Ventoy. Or add the d-pad
  to an existing stick with the Deno installer.

## Layout

- [`driver/`](driver/README.md): the Rust sources, shared by every boot target.
  - `common/`: platform-independent parsing and key mapping (host tests).
  - `dpad/` → `claw-dpad.efi`: the boot-service driver.
  - `probe/` → `claw-probe.efi`: diagnostic app that logs how the controller
    looks to firmware.
- `ventoy/chainloader/` → `claw-chainload.efi`: loads the driver, then the
  boot loader it replaced.
- `ventoy/installer/`: Deno installer for Ventoy sticks, the Ventoy package
  patcher and its QEMU smoke test.
- `.github/workflows/ventoy-release.yml`: weekly check for a new Ventoy
  release; builds, boot-tests and publishes the patched packages.

## Build

```sh
rustup target add x86_64-unknown-uefi
cargo test -p claw-common   # host tests
cargo efi                   # release build of all .efi files
```

Output: `target/x86_64-unknown-uefi/release/*.efi`. Every workflow run also
uploads them as the `claw-efi` artifact, and each [release](../../releases)
attaches them next to the patched Ventoy packages.
