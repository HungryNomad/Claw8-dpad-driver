//! claw-chainload: takes the place of `\EFI\BOOT\BOOTX64.EFI` on a boot
//! partition (Ventoy's VTOYEFI), starts the d-pad driver, then hands off to
//! the original loader, which the installer renamed next to it.
//!
//! The original loader is started from its real device path, so it sees its
//! own location and finds its neighbours (shim → `grubx64.efi`, GRUB → its
//! config) as if the firmware had started it. A missing or broken driver
//! never blocks booting: it is reported and skipped.
#![no_std]
#![no_main]

use core::mem::MaybeUninit;
use core::time::Duration;

use uefi::boot::{self, LoadImageSource, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol};
use uefi::proto::BootPolicy;
use uefi::proto::device_path::build::{self, DevicePathBuilder};
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::{CStr16, Handle, Status, cstr16, entry, println};

/// The d-pad driver, loaded first.
const DRIVER: &CStr16 = cstr16!("\\EFI\\claw\\claw-dpad.efi");
/// The loader this image replaced, renamed in the same folder by the installer.
/// Ventoy's loader is shim, which only runs Ventoy's `fbx64.efi` (and from it
/// GRUB) when its own path starts with `\EFI\BOOT\BOOT`, so the name must too.
const ORIGINAL: &CStr16 = cstr16!("\\EFI\\BOOT\\BOOTX64_ORIG.EFI");
/// How long an error stays on screen.
const ERROR_PAUSE: Duration = Duration::from_secs(5);

/// Opens protocol `P` on `handle` without taking ownership of it.
fn get<P: uefi::proto::ProtocolPointer + ?Sized>(handle: Handle) -> uefi::Result<ScopedProtocol<P>> {
    let params = OpenProtocolParams { handle, agent: boot::image_handle(), controller: None };
    // SAFETY: GetProtocol is a non-exclusive read; we only read the protocol
    // while the handle is alive (boot services, no disconnects in between).
    unsafe { boot::open_protocol::<P>(params, OpenProtocolAttributes::GetProtocol) }
}

/// Loads the image at `path` on the partition `device` came from.
fn load(device: Handle, path: &CStr16) -> uefi::Result<Handle> {
    let mut buf = [MaybeUninit::uninit(); 512];
    let partition = get::<DevicePath>(device)?;
    let mut builder = DevicePathBuilder::with_buf(&mut buf);
    for node in partition.node_iter() {
        builder = builder.push(&node).map_err(|_| Status::BUFFER_TOO_SMALL)?;
    }
    let full = builder
        .push(&build::media::FilePath { path_name: path })
        .and_then(DevicePathBuilder::finalize)
        .map_err(|_| Status::BUFFER_TOO_SMALL)?;
    let source = LoadImageSource::FromDevicePath { device_path: full, boot_policy: BootPolicy::ExactMatch };
    boot::load_image(boot::image_handle(), source)
}

/// Loads and starts the driver. A boot-service driver returns from its entry
/// point and stays resident, already bound to the controller if present.
fn start_driver(device: Handle) -> uefi::Result {
    let driver = load(device, DRIVER)?;
    boot::start_image(driver)
}

/// Prints `what` failed with `status` and leaves it on screen briefly.
fn report(what: &CStr16, status: Status) {
    println!("claw-chainload: {what}: {status:?}");
    boot::stall(ERROR_PAUSE);
}

/// Entry point: driver first (best effort), then the original loader.
#[entry]
fn main() -> Status {
    let device = match get::<LoadedImage>(boot::image_handle()).map(|me| me.device()) {
        Ok(Some(device)) => device,
        Ok(None) => return Status::NOT_FOUND,
        Err(e) => return e.status(),
    };
    if let Err(e) = start_driver(device) {
        report(DRIVER, e.status());
    }
    match load(device, ORIGINAL).and_then(boot::start_image) {
        Ok(()) => Status::SUCCESS,
        Err(e) => {
            report(ORIGINAL, e.status());
            e.status()
        }
    }
}
