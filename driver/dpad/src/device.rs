//! USB side: identify the Claw gamepad interface, read its reports with an
//! async interrupt transfer and feed them to the key mapper.

use core::ffi::c_void;
use core::mem::MaybeUninit;
use core::ptr;

use claw_common::usb::{self, IfaceKind};
use claw_common::{hid, pad};
use uefi_raw::protocol::usb::io::UsbIoProtocol;
use uefi_raw::protocol::usb::{
    ConfigDescriptor, DataDirection, DeviceDescriptor, DeviceRequest, EndpointDescriptor,
    InterfaceDescriptor, UsbTransferStatus,
};
use uefi_raw::Event;
use uefi_raw::{Boolean, Handle, Status};

use crate::state::{Decoder, Device, with};

/// Timer period driving key repeat and error recovery.
pub const TICK_MS: u32 = 20;
/// Delay before re-arming the transfer after a USB error.
const RESTART_DELAY_MS: u32 = 500;

/// Reads the device and interface descriptors and classifies the interface.
pub fn identify(usb: *mut UsbIoProtocol) -> Option<(IfaceKind, InterfaceDescriptor)> {
    let mut dev = MaybeUninit::<DeviceDescriptor>::uninit();
    let mut ifd = MaybeUninit::<InterfaceDescriptor>::uninit();
    // SAFETY: valid UsbIo pointer; firmware fills the descriptors on success.
    unsafe {
        ((*usb).get_device_descriptor)(usb, dev.as_mut_ptr()).is_success().then_some(())?;
        ((*usb).get_interface_descriptor)(usb, ifd.as_mut_ptr()).is_success().then_some(())?;
        let (dev, ifd) = (dev.assume_init(), ifd.assume_init());
        let kind = usb::classify(
            dev.id_vendor,
            dev.id_product,
            ifd.interface_class,
            ifd.interface_subclass,
            ifd.interface_protocol,
        )?;
        Some((kind, ifd))
    }
}

/// Fetches endpoint descriptor `index` of the interface.
fn endpoint(usb: *mut UsbIoProtocol, index: u8) -> Option<EndpointDescriptor> {
    let mut ep = MaybeUninit::<EndpointDescriptor>::uninit();
    // SAFETY: valid UsbIo pointer; filled on success.
    unsafe {
        ((*usb).get_endpoint_descriptor)(usb, index, ep.as_mut_ptr())
            .is_success()
            .then(|| ep.assume_init())
    }
}

/// Standard control IN transfer into `buf`.
fn control_in(usb: *mut UsbIoProtocol, request_type: u8, request: u8, value: u16, index: u16, buf: &mut [u8]) -> bool {
    let req = DeviceRequest { request_type, request, value, index, length: buf.len() as u16 };
    let mut status = UsbTransferStatus::default();
    // SAFETY: `buf` is valid for `buf.len()` bytes for the whole call.
    unsafe {
        ((*usb).control_transfer)(usb, &req, DataDirection::DATA_IN, 1000, buf.as_mut_ptr().cast(), buf.len(), &mut status)
            .is_success()
    }
}

/// Reads and parses the HID report descriptor of interface `iface`.
fn hid_layout(usb: *mut UsbIoProtocol, iface: u8) -> Option<hid::Layout> {
    let mut cfg = MaybeUninit::<ConfigDescriptor>::uninit();
    // SAFETY: valid UsbIo pointer; filled on success.
    let total = unsafe {
        ((*usb).get_config_descriptor)(usb, cfg.as_mut_ptr()).is_success().then_some(())?;
        usize::from(cfg.assume_init().total_length)
    };
    let mut config = [0u8; 512];
    let config = &mut config[..total.min(512)];
    control_in(usb, 0x80, 0x06, 0x0200, 0, config).then_some(())?;
    let len = usize::from(usb::hid_report_len(config, iface)?).min(1024);
    let mut report = [0u8; 1024];
    control_in(usb, 0x81, 0x06, 0x2200, iface.into(), &mut report[..len]).then_some(())?;
    hid::parse(&report[..len])
}

/// Best-effort "player 1 LED on" message; some Xbox 360-compatible pads only
/// start streaming after the host talks to them.
fn xinput_hello(usb: *mut UsbIoProtocol, ep_out: u8) {
    let mut msg = [0x01u8, 0x03, 0x06];
    let mut len = msg.len();
    let mut status = UsbTransferStatus::default();
    // SAFETY: `msg` outlives the synchronous call.
    unsafe {
        let _ = ((*usb).sync_interrupt_transfer)(usb, ep_out, msg.as_mut_ptr().cast(), &mut len, 50, &mut status);
    }
}

/// Inspects the interface and builds the device record (no transfer yet).
pub fn open(controller: Handle, usb: *mut UsbIoProtocol) -> Result<Device, Status> {
    let (kind, ifd) = identify(usb).ok_or(Status::UNSUPPORTED)?;
    let interrupt = |e: &EndpointDescriptor| e.attributes & 0x03 == 0x03;
    let eps = (0..ifd.num_endpoints).filter_map(|i| endpoint(usb, i));
    let ep_in = eps.clone().find(|e| interrupt(e) && e.endpoint_address & 0x80 != 0).ok_or(Status::UNSUPPORTED)?;
    let decoder = match kind {
        IfaceKind::XInput => {
            if let Some(out) = eps.clone().find(|e| interrupt(e) && e.endpoint_address & 0x80 == 0) {
                xinput_hello(usb, out.endpoint_address);
            }
            Decoder::XInput
        }
        IfaceKind::Hid => Decoder::Hid(hid_layout(usb, ifd.interface_number).ok_or(Status::UNSUPPORTED)?),
    };
    Ok(Device {
        controller,
        usb,
        ep_in: ep_in.endpoint_address,
        max_packet: usize::from(ep_in.max_packet_size & 0x7FF).clamp(8, 64),
        interval_ms: usize::from(ep_in.interval).clamp(1, 16),
        decoder,
        restart_at_ms: None,
    })
}

/// Starts or cancels the async interrupt transfer for `dev`.
pub fn set_polling(usb: *mut UsbIoProtocol, ep: u8, on: bool, interval_ms: usize, max_packet: usize) -> Status {
    // SAFETY: valid UsbIo pointer; the callback is a static function.
    unsafe {
        ((*usb).async_interrupt_transfer)(
            usb,
            ep,
            Boolean::from(on),
            if on { interval_ms } else { 0 },
            if on { max_packet } else { 0 },
            on.then_some(on_report as _),
            ptr::null(),
        )
    }
}

/// Async interrupt callback: decode the report and map it to keys.
unsafe extern "efiapi" fn on_report(
    data: *const c_void,
    len: usize,
    _: *const c_void,
    status: UsbTransferStatus,
) -> Status {
    with(|d| {
        let now = d.now_ms;
        let Some(dev) = d.dev.as_mut() else { return };
        if status != UsbTransferStatus::SUCCESS {
            dev.restart_at_ms.get_or_insert(now.wrapping_add(RESTART_DELAY_MS));
            d.mapper.reset();
            return;
        }
        if data.is_null() || len == 0 {
            return;
        }
        // SAFETY: firmware hands us `len` valid bytes for this call.
        let report = unsafe { core::slice::from_raw_parts(data.cast::<u8>(), len) };
        let pad = match &dev.decoder {
            Decoder::XInput => pad::parse_xinput(report),
            Decoder::Hid(layout) => layout.decode(report),
        };
        if let Some(pad) = pad {
            let mut mapper = core::mem::take(&mut d.mapper);
            mapper.update(pad, now, |k| d.emit(k));
            d.mapper = mapper;
        }
    });
    Status::SUCCESS
}

/// Periodic timer: advances time, emits key repeats, recovers from errors.
pub unsafe extern "efiapi" fn on_tick(_: Event, _: *mut c_void) {
    let restart = with(|d| {
        d.now_ms = d.now_ms.wrapping_add(TICK_MS);
        let now = d.now_ms;
        let mut mapper = core::mem::take(&mut d.mapper);
        mapper.tick(now, |k| d.emit(k));
        d.mapper = mapper;
        let dev = d.dev.as_mut()?;
        let due = dev.restart_at_ms.is_some_and(|t| now.wrapping_sub(t) as i32 >= 0);
        due.then(|| {
            dev.restart_at_ms = None;
            (dev.usb, dev.ep_in, dev.interval_ms, dev.max_packet)
        })
    });
    if let Some((usb, ep, interval, mps)) = restart {
        let _ = set_polling(usb, ep, false, 0, 0);
        clear_halt(usb, ep);
        if set_polling(usb, ep, true, interval, mps).is_error() {
            with(|d| {
                let now = d.now_ms;
                if let Some(dev) = d.dev.as_mut() {
                    dev.restart_at_ms = Some(now.wrapping_add(RESTART_DELAY_MS));
                }
            });
        }
    }
}

/// CLEAR_FEATURE(ENDPOINT_HALT) on `ep`, ignoring failures.
fn clear_halt(usb: *mut UsbIoProtocol, ep: u8) {
    let req = DeviceRequest { request_type: 0x02, request: 0x01, value: 0, index: ep.into(), length: 0 };
    let mut status = UsbTransferStatus::default();
    // SAFETY: no data stage.
    unsafe {
        let _ = ((*usb).control_transfer)(usb, &req, DataDirection::NO_DATA, 100, ptr::null_mut(), 0, &mut status);
    }
}
