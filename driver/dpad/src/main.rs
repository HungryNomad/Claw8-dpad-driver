//! claw-dpad: a UEFI boot-service driver that makes the MSI Claw's built-in
//! gamepad act as a keyboard in boot menus (rEFInd, Ventoy, GRUB, the shell).
//!
//! D-pad → arrows (with repeat), A/Start → Enter, B → Esc, Y → F2,
//! LB → Home, RB → End. See `claw_common::keys::KEYMAP`.
//!
//! It binds to the Claw's USB gamepad interface (XInput or DInput mode) with
//! the UEFI Driver Binding model, installs `EFI_SIMPLE_TEXT_INPUT(_EX)` on
//! that handle so the console splitter merges it into `ConIn`, and falls back
//! to wrapping `ConIn.ReadKeyStroke` on firmware that ignores late devices.
#![no_std]
#![no_main]

mod console;
mod device;
mod state;

use core::ffi::c_void;
use core::ptr;

use uefi::entry;
use uefi_raw::protocol::device_path::DevicePathProtocol;
use uefi_raw::protocol::driver::DriverBindingProtocol;
use uefi_raw::protocol::usb::io::UsbIoProtocol;
use uefi_raw::table::boot::{EventType, InterfaceType, TimerDelay, Tpl};
use uefi_raw::{Boolean, Handle, Status};

use crate::state::{Global, bs, log, with};

const BY_DRIVER: u32 = 0x10;
const LOCATE_BY_PROTOCOL: i32 = 2;
/// 100 ns units per millisecond, for `SetTimer`.
const MS: u64 = 10_000;

static BINDING: Global<DriverBindingProtocol> = Global::new(DriverBindingProtocol {
    supported,
    start,
    stop,
    version: 0x10,
    image_handle: ptr::null_mut(),
    driver_binding_handle: ptr::null_mut(),
});

/// Opens UsbIo on `controller` by driver.
fn open_usb(controller: Handle, image: Handle) -> Result<*mut UsbIoProtocol, Status> {
    let mut usb = ptr::null_mut();
    // SAFETY: standard driver-model open; paired with `close_usb`.
    let st = unsafe { (bs().open_protocol)(controller, &UsbIoProtocol::GUID, &mut usb, image, controller, BY_DRIVER) };
    if st.is_success() { Ok(usb.cast()) } else { Err(st) }
}

/// Closes the by-driver UsbIo open.
fn close_usb(controller: Handle, image: Handle) {
    // SAFETY: matches a successful `open_usb`.
    let _ = unsafe { (bs().close_protocol)(controller, &UsbIoProtocol::GUID, image, controller) };
}

/// The handle of this driver image.
fn image() -> Handle {
    with(|d| d.image)
}

/// Driver Binding `Supported`: a free Claw gamepad interface we can read.
unsafe extern "efiapi" fn supported(_: *const DriverBindingProtocol, controller: Handle, _: *const DevicePathProtocol) -> Status {
    if with(|d| d.dev.is_some()) {
        return Status::UNSUPPORTED;
    }
    let image = image();
    let usb = match open_usb(controller, image) {
        Ok(u) => u,
        Err(st) => return st,
    };
    let ok = device::identify(usb).is_some();
    close_usb(controller, image);
    if ok { Status::SUCCESS } else { Status::UNSUPPORTED }
}

/// Driver Binding `Start`: open the interface, publish the keyboard
/// protocols and begin polling.
unsafe extern "efiapi" fn start(_: *const DriverBindingProtocol, controller: Handle, _: *const DevicePathProtocol) -> Status {
    if with(|d| d.dev.is_some()) {
        return Status::ALREADY_STARTED;
    }
    let image = image();
    let usb = match open_usb(controller, image) {
        Ok(u) => u,
        Err(st) => return st,
    };
    let dev = match device::open(controller, usb) {
        Ok(d) => d,
        Err(st) => {
            log("interface not usable, status", Some(st.0));
            close_usb(controller, image);
            return st;
        }
    };
    let (ep, interval, mps) = (dev.ep_in, dev.interval_ms, dev.max_packet);
    let st = console::install(controller);
    if st.is_error() {
        close_usb(controller, image);
        return st;
    }
    let (tick, verify) = with(|d| {
        d.dev = Some(dev);
        d.keys.clear();
        d.mapper.reset();
        (d.tick_event, d.verify_event)
    });
    let st = device::set_polling(usb, ep, true, interval, mps);
    log("bound gamepad; polling endpoint", Some(ep.into()));
    if st.is_error() {
        log("async transfer failed, status", Some(st.0));
        with(|d| d.dev = None);
        console::uninstall(controller);
        close_usb(controller, image);
        return st;
    }
    // SAFETY: our own timer events, created in `init`.
    unsafe {
        let _ = (bs().set_timer)(tick, TimerDelay::PERIODIC, u64::from(device::TICK_MS) * MS);
        let _ = (bs().set_timer)(verify, TimerDelay::RELATIVE, 300 * MS);
    }
    Status::SUCCESS
}

/// Driver Binding `Stop`: undo `start` for `controller`.
unsafe extern "efiapi" fn stop(_: *const DriverBindingProtocol, controller: Handle, _: usize, _: *const Handle) -> Status {
    let Some((dev, tick)) = with(|d| d.dev.take_if(|v| v.controller == controller).map(|v| (v, d.tick_event))) else {
        return Status::DEVICE_ERROR;
    };
    // SAFETY: cancelling our own timer.
    let _ = unsafe { (bs().set_timer)(tick, TimerDelay::CANCEL, 0) };
    let _ = device::set_polling(dev.usb, dev.ep_in, false, 0, 0);
    console::unhook_con_in();
    console::uninstall(controller);
    close_usb(controller, image());
    Status::SUCCESS
}

/// Creates one of the driver's events.
fn event(ty: EventType, tpl: Tpl, notify: unsafe extern "efiapi" fn(*mut c_void, *mut c_void)) -> Result<*mut c_void, Status> {
    let mut ev = ptr::null_mut();
    // SAFETY: static notify functions and no context.
    let st = unsafe { (bs().create_event)(ty, tpl, Some(notify), ptr::null_mut(), &mut ev) };
    if st.is_success() { Ok(ev) } else { Err(st) }
}

/// Verify timer: make sure our keys actually reach `ConIn`.
unsafe extern "efiapi" fn on_verify(_: *mut c_void, _: *mut c_void) {
    console::ensure_consumed();
}

/// Connects drivers to every MSI USB interface already present, so the
/// gamepad starts working without waiting for the loader to reconnect.
fn connect_existing() {
    let bs = bs();
    let mut count = 0usize;
    let mut handles: *mut Handle = ptr::null_mut();
    // SAFETY: firmware allocates the handle buffer; freed below.
    unsafe {
        if (bs.locate_handle_buffer)(LOCATE_BY_PROTOCOL, &UsbIoProtocol::GUID, ptr::null(), &mut count, &mut handles).is_error() {
            return;
        }
        for &h in core::slice::from_raw_parts(handles, count) {
            let mut usb = ptr::null_mut();
            let get = (bs.open_protocol)(h, &UsbIoProtocol::GUID, &mut usb, image(), ptr::null_mut(), 0x02);
            if get.is_success() && device::identify(usb.cast()).is_some() {
                let _ = (bs.connect_controller)(h, ptr::null(), ptr::null(), Boolean::FALSE);
            }
        }
        let _ = (bs.free_pool)(handles.cast());
    }
}

/// Sets up events, registers the driver binding and binds existing devices.
fn init() -> Result<(), Status> {
    let image = uefi::boot::image_handle().as_ptr();
    let wait = event(EventType::NOTIFY_WAIT, Tpl::NOTIFY, console::wait_notify)?;
    let tick = event(EventType::TIMER | EventType::NOTIFY_SIGNAL, Tpl::CALLBACK, device::on_tick)?;
    let verify = event(EventType::TIMER | EventType::NOTIFY_SIGNAL, Tpl::CALLBACK, on_verify)?;
    with(|d| {
        d.image = image;
        d.wait_event = wait;
        d.tick_event = tick;
        d.verify_event = verify;
    });
    // SAFETY: the statics are only read by firmware after this point.
    unsafe {
        (*console::TEXT_IN.get()).wait_for_key = wait;
        (*console::TEXT_IN_EX.get()).wait_for_key_ex = wait;
        let b = BINDING.get();
        (*b).image_handle = image;
        (*b).driver_binding_handle = image;
        let mut h = image;
        let st = (bs().install_protocol_interface)(&mut h, &DriverBindingProtocol::GUID, InterfaceType::NATIVE_INTERFACE, b.cast());
        if st.is_error() {
            return Err(st);
        }
    }
    log("loaded", None);
    connect_existing();
    console::ensure_consumed();
    Ok(())
}

/// Image entry point. Stays resident (boot-service driver) on success, even
/// with no Claw present, so a re-enumerated gamepad is picked up later.
#[entry]
fn main() -> Status {
    match init() {
        Ok(()) => Status::SUCCESS,
        Err(st) => st,
    }
}
