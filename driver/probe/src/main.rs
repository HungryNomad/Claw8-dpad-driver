//! claw-probe: a one-shot UEFI app that shows how the MSI Claw gamepad looks
//! to firmware, so the driver can be checked against real data.
//!
//! It lists every USB interface, dumps the Claw's descriptors, asks the MCU
//! which gamepad mode it is in (read-only command), then walks you through
//! pressing each button while it records the raw reports. Everything is shown
//! on screen and written to `\claw-probe-log.txt` on the volume it ran from.
//! It needs no keyboard and returns to the boot menu by itself.
#![no_std]
#![no_main]

extern crate alloc;

use alloc::{format, string::String, vec, vec::Vec};
use core::cell::UnsafeCell;
use core::ffi::c_void;
use core::fmt::Write as _;
use core::time::Duration;

use claw_common::{hid, usb};
use uefi::boot::{self, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol};
use uefi::fs::FileSystem;
use uefi::proto::console::text::Key;
use uefi::proto::usb::io::{ControlTransfer, UsbIo};
use uefi::{Handle, Status, cstr16, entry, println, system};
use uefi_raw::Boolean;
use uefi_raw::protocol::usb::io::UsbIoProtocol;
use uefi_raw::protocol::usb::{EndpointDescriptor, UsbTransferStatus};
use uefi_raw::table::boot::Tpl;

/// Buttons to press, in order, with how long to hold each (ms).
const STEPS: &[(&str, u32)] = &[
    ("D-PAD UP", 2500),
    ("D-PAD DOWN", 2500),
    ("D-PAD LEFT", 2500),
    ("D-PAD RIGHT", 2500),
    ("A", 2000),
    ("B", 2000),
    ("X", 2000),
    ("Y", 2000),
    ("LB (left bumper)", 2000),
    ("RB (right bumper)", 2000),
    ("LT (left trigger)", 2000),
    ("RT (right trigger)", 2000),
    ("VIEW / SELECT (left small button)", 2000),
    ("MENU / START (right small button)", 2000),
    ("M1 (back right paddle)", 2000),
    ("M2 (back left paddle)", 2000),
    ("MSI CENTER button", 2000),
    ("QUICK SETTINGS (...) button", 2000),
    ("LEFT STICK: push up", 2000),
];

/// One USB interface found on the system.
struct Iface {
    handle: Handle,
    vid: u16,
    pid: u16,
    number: u8,
    class: (u8, u8, u8),
    endpoints: Vec<EndpointDescriptor>,
}

/// Screen + file logger.
struct Log(String);

impl Log {
    /// Prints a line and keeps it for the log file.
    fn line(&mut self, s: &str) {
        println!("{s}");
        self.0.push_str(s);
        self.0.push_str("\r\n");
    }
}

/// Formats bytes as space-separated hex, 32 per line.
fn hex(data: &[u8]) -> String {
    let mut s = String::new();
    for (i, b) in data.iter().enumerate() {
        let sep = match i {
            0 => "",
            _ if i % 32 == 0 => "\r\n    ",
            _ => " ",
        };
        let _ = write!(s, "{sep}{b:02x}");
    }
    s
}

/// Opens UsbIo on `handle` without taking ownership from any driver.
fn open_usb(handle: Handle) -> uefi::Result<ScopedProtocol<UsbIo>> {
    let params = OpenProtocolParams { handle, agent: boot::image_handle(), controller: None };
    // SAFETY: GetProtocol does not disturb drivers that own the interface; we
    // only use it while this app runs.
    unsafe { boot::open_protocol::<UsbIo>(params, OpenProtocolAttributes::GetProtocol) }
}

/// Collects descriptors for every USB interface handle.
fn list_interfaces() -> Vec<Iface> {
    let handles = boot::find_handles::<UsbIo>().unwrap_or_default();
    handles
        .into_iter()
        .filter_map(|handle| {
            let mut io = open_usb(handle).ok()?;
            let dev = io.device_descriptor().ok()?;
            let ifd = io.interface_descriptor().ok()?;
            let endpoints = (0..ifd.num_endpoints)
                .filter_map(|i| io.endpoint_descriptor(i).ok())
                .collect();
            Some(Iface {
                handle,
                vid: dev.id_vendor,
                pid: dev.id_product,
                number: ifd.interface_number,
                class: (ifd.interface_class, ifd.interface_subclass, ifd.interface_protocol),
                endpoints,
            })
        })
        .collect()
}

/// Issues a standard GET_DESCRIPTOR into a buffer of `len` bytes.
fn get_descriptor(io: &mut UsbIo, recipient: u8, value: u16, index: u16, len: usize) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; len];
    io.control_transfer(recipient, 0x06, value, index, ControlTransfer::DataIn(&mut buf), 1000)
        .ok()?;
    Some(buf)
}

/// Reads a string descriptor in US English, or "?".
fn string(io: &mut UsbIo, index: u8) -> String {
    if index == 0 {
        return String::from("-");
    }
    io.string_descriptor(0x0409, index)
        .map(|s| format!("{}", &*s))
        .unwrap_or_else(|_| String::from("?"))
}

/// Dumps device strings, the full config descriptor and HID report
/// descriptors for one Claw interface.
fn dump_claw_iface(log: &mut Log, it: &Iface, dumped_config: &mut bool) {
    let Ok(mut io) = open_usb(it.handle) else { return };
    if !*dumped_config {
        *dumped_config = true;
        if let Ok(dev) = io.device_descriptor() {
            let (m, p) = (string(&mut io, dev.str_manufacturer), string(&mut io, dev.str_product));
            log.line(&format!("  Manufacturer: {m}  Product: {p}  bcdDevice: {:04x}", { dev.bcd_device }));
        }
        if let Ok(cfg) = io.config_descriptor() {
            let total = usize::from(cfg.total_length);
            if let Some(full) = get_descriptor(&mut io, 0x80, 0x0200, 0, total) {
                log.line(&format!("  Config descriptor ({total} bytes):\r\n    {}", hex(&full)));
            }
        }
    }
    if it.class.0 == 0x03 {
        let len = io
            .config_descriptor()
            .ok()
            .and_then(|c| get_descriptor(&mut io, 0x80, 0x0200, 0, usize::from(c.total_length)))
            .and_then(|full| usb::hid_report_len(&full, it.number));
        match len.and_then(|n| get_descriptor(&mut io, 0x81, 0x2200, it.number.into(), n.into())) {
            Some(rd) => {
                log.line(&format!("  iface {} HID report descriptor ({} bytes):\r\n    {}", it.number, rd.len(), hex(&rd)));
                match hid::parse(&rd) {
                    Some(l) => log.line(&format!("    -> gamepad layout: {l:?}")),
                    None => log.line("    -> no gamepad/joystick collection"),
                }
            }
            None => log.line(&format!("  iface {}: could not read HID report descriptor", it.number)),
        }
    }
}

/// Sends the read-only "read gamepad mode" command on the config interface
/// and prints the MCU's answer. Never changes the mode.
fn query_mode(log: &mut Log, it: &Iface) {
    let Ok(mut io) = open_usb(it.handle) else { return };
    let ep_in = it.endpoints.iter().find(|e| e.endpoint_address & 0x80 != 0 && e.attributes & 3 == 3);
    let ep_out = it.endpoints.iter().find(|e| e.endpoint_address & 0x80 == 0 && e.attributes & 3 == 3);
    let Some(ep_in) = ep_in else { return };
    let frame = usb::config_frame(usb::CMD_READ_MODE, &[]);
    let sent = match ep_out {
        Some(o) => io.sync_interrupt_send(o.endpoint_address, &frame, 100).map(|_| "interrupt OUT"),
        None => {
            let value = 0x0200 | u16::from(usb::CFG_OUT_ID); // SET_REPORT(Output, id)
            io.control_transfer(0x21, 0x09, value, it.number.into(), ControlTransfer::DataOut(&frame), 100)
                .map(|()| "SET_REPORT")
        }
    };
    match sent {
        Ok(how) => log.line(&format!("  mode query sent via {how} on iface {}", it.number)),
        Err(e) => {
            log.line(&format!("  mode query send failed on iface {}: {e:?}", it.number));
            return;
        }
    }
    let mut buf = [0u8; usb::CFG_PACKET];
    for _ in 0..10 {
        let Ok(n) = io.sync_interrupt_receive(ep_in.endpoint_address & 0x7F, &mut buf, 50) else { continue };
        let r = &buf[..n];
        log.line(&format!("  config reply: {}", hex(r)));
        if let [usb::CFG_IN_ID, _, _, 0x3C, usb::CMD_MODE_ACK, mode, mkeys, ..] = *r {
            let name = match mode {
                1 => "XInput",
                2 => "DInput",
                4 => "Desktop",
                _ => "unknown",
            };
            log.line(&format!("  >>> gamepad mode = {mode} ({name}), M-keys function = {mkeys}"));
            return;
        }
    }
    log.line("  no mode reply (the MCU may not answer in firmware; that's OK)");
}

/// Ring buffer the async USB callbacks write into.
struct Ring {
    items: [(u8, u8, [u8; 64]); 128],
    head: usize,
    tail: usize,
    dropped: u32,
}

/// Interior-mutable static shared with firmware callbacks.
struct Global<T>(UnsafeCell<T>);

// SAFETY: UEFI is single-threaded; access is serialised by raising the TPL.
unsafe impl<T> Sync for Global<T> {}

static RING: Global<Ring> =
    Global(UnsafeCell::new(Ring { items: [(0, 0, [0; 64]); 128], head: 0, tail: 0, dropped: 0 }));

/// Async interrupt callback: copies the report into the ring.
/// `ctx` carries the index of the endpoint in the capture list.
unsafe extern "efiapi" fn on_report(
    data: *const c_void,
    len: usize,
    ctx: *const c_void,
    status: UsbTransferStatus,
) -> uefi_raw::Status {
    // SAFETY: called by firmware at TPL_CALLBACK or above; main loop reads
    // the ring only at TPL_NOTIFY.
    let ring = unsafe { &mut *RING.0.get() };
    let next = (ring.head + 1) % ring.items.len();
    if next == ring.tail {
        ring.dropped += 1;
        return uefi_raw::Status::SUCCESS;
    }
    let n = if status == UsbTransferStatus::SUCCESS { len.min(64) } else { 0 };
    let mut buf = [0u8; 64];
    if n > 0 && !data.is_null() {
        // SAFETY: firmware hands us `len` valid bytes.
        buf[..n].copy_from_slice(unsafe { core::slice::from_raw_parts(data.cast::<u8>(), n) });
    }
    ring.items[ring.head] = (ctx as usize as u8, n as u8, buf);
    ring.head = next;
    uefi_raw::Status::SUCCESS
}

/// Takes all queued reports out of the ring.
fn drain() -> Vec<(u8, Vec<u8>)> {
    // SAFETY: raising to NOTIFY blocks the USB callbacks while we read.
    let _tpl = unsafe { boot::raise_tpl(Tpl::NOTIFY) };
    let ring = unsafe { &mut *RING.0.get() };
    let mut out = Vec::new();
    while ring.tail != ring.head {
        let (i, n, buf) = ring.items[ring.tail];
        out.push((i, buf[..usize::from(n)].to_vec()));
        ring.tail = (ring.tail + 1) % ring.items.len();
    }
    out
}

/// An endpoint being captured.
struct Capture {
    io: ScopedProtocol<UsbIo>,
    ep: u8,
    label: String,
}

impl Capture {
    /// Raw protocol pointer for the async-transfer call.
    fn raw(&mut self) -> *mut UsbIoProtocol {
        let io: &mut UsbIo = &mut self.io;
        core::ptr::from_mut(io).cast()
    }

    /// Starts (`on = true`) or cancels the async interrupt transfer.
    fn set_async(&mut self, index: usize, on: bool, max_packet: usize) -> Status {
        let raw = self.raw();
        // SAFETY: valid protocol pointer; callback and context stay valid
        // until the transfer is cancelled below, before the app exits.
        unsafe {
            ((*raw).async_interrupt_transfer)(
                raw,
                self.ep,
                Boolean::from(on),
                8,
                max_packet,
                on.then_some(on_report as _),
                index as *const c_void,
            )
        }
    }
}

/// Logs any keys the firmware console delivered (shows whether firmware
/// already maps the gamepad to keys).
fn log_conin(log: &mut Log, t: u32) {
    while let Ok(Some(key)) = system::with_stdin(|i| i.read_key()) {
        let k = match key {
            Key::Printable(c) => format!("char {:#06x}", u16::from(c)),
            Key::Special(s) => format!("scan {:#06x}", s.0),
        };
        log.line(&format!("  [{t:6} ms] ConIn key: {k}"));
    }
}

/// Returns `true` if a driver (e.g. claw-dpad) has opened this interface by
/// driver; its endpoints are then left alone.
fn owned_by_driver(handle: Handle) -> bool {
    let Some(st) = uefi::table::system_table_raw() else { return false };
    let mut entries = core::ptr::null_mut();
    let mut count = 0usize;
    // SAFETY: firmware allocates the entry buffer; we free it below.
    unsafe {
        let bs = &*st.as_ref().boot_services;
        if (bs.open_protocol_information)(handle.as_ptr(), &UsbIoProtocol::GUID, &mut entries, &mut count).is_error() {
            return false;
        }
        let owned = core::slice::from_raw_parts(entries, count).iter().any(|e| e.attributes & 0x10 != 0);
        let _ = (bs.free_pool)(entries.cast());
        owned
    }
}

/// Runs the guided button capture over all Claw input endpoints.
fn capture(log: &mut Log, ifaces: &[&Iface]) {
    let mut caps: Vec<(Capture, usize)> = Vec::new();
    for it in ifaces {
        // Boot keyboards/mice belong to firmware drivers; don't steal them.
        if it.class.0 == 0x03 && it.class.2 != 0 {
            log.line(&format!("  skip iface {} (boot keyboard/mouse; watch ConIn keys instead)", it.number));
            continue;
        }
        if owned_by_driver(it.handle) {
            log.line(&format!("  skip iface {} (a driver owns it; if claw-dpad is installed, its keys show as ConIn keys)", it.number));
            continue;
        }
        for e in it.endpoints.iter().filter(|e| e.endpoint_address & 0x80 != 0 && e.attributes & 3 == 3) {
            let Ok(io) = open_usb(it.handle) else { continue };
            let label = format!("if{} ep{:02x}", it.number, e.endpoint_address);
            caps.push((Capture { io, ep: e.endpoint_address, label }, usize::from(e.max_packet_size).max(1)));
        }
    }
    for (i, (c, mps)) in caps.iter_mut().enumerate() {
        let st = c.set_async(i, true, (*mps).min(64));
        log.line(&format!("  listening on {}: {st:?}", c.label));
    }
    if caps.is_empty() {
        log.line("  nothing to listen on");
    }

    let mut t = 0u32;
    let mut last: Vec<Vec<u8>> = vec![Vec::new(); caps.len()];
    let mut record = |log: &mut Log, t: u32, labels: &[String]| {
        for (i, data) in drain() {
            let i = usize::from(i);
            if data.is_empty() {
                log.line(&format!("  [{t:6} ms] {} transfer error", labels[i]));
            } else if last[i] != data {
                log.line(&format!("  [{t:6} ms] {}: {}", labels[i], hex(&data)));
                last[i] = data;
            }
        }
    };
    let labels: Vec<String> = caps.iter().map(|(c, _)| c.label.clone()).collect();
    let mut wait = |log: &mut Log, ms: u32, t: &mut u32| {
        for _ in 0..ms / 10 {
            boot::stall(Duration::from_millis(10));
            *t += 10;
            record(log, *t, &labels);
            log_conin(log, *t);
        }
    };

    log.line("");
    log.line("Hands off the controller for 2 seconds (baseline)...");
    wait(log, 2000, &mut t);
    for &(name, ms) in STEPS {
        log.line(&format!("== PRESS AND HOLD: {name}"));
        wait(log, ms, &mut t);
        log.line("   release");
        wait(log, 1000, &mut t);
    }
    log.line("== Free play: press anything for 8 seconds");
    wait(log, 8000, &mut t);

    for (i, (c, mps)) in caps.iter_mut().enumerate() {
        let _ = c.set_async(i, false, *mps);
    }
    // SAFETY: read-only access to the drop counter after transfers stopped.
    let dropped = unsafe { (*RING.0.get()).dropped };
    if dropped > 0 {
        log.line(&format!("  ({dropped} reports dropped: ring full)"));
    }
}

/// Writes the log to `\claw-probe-log.txt` on the probe's own volume.
fn save(log: &Log) -> uefi::Result<()> {
    let sfs = boot::get_image_file_system(boot::image_handle())?;
    let mut fs = FileSystem::new(sfs);
    fs.write(cstr16!("\\claw-probe-log.txt"), log.0.as_bytes())
        .map_err(|_| Status::DEVICE_ERROR.into())
}

/// Entry point: list, dump, query, capture, save, return.
#[entry]
fn main() -> Status {
    let _ = system::with_stdout(|o| o.clear());
    let mut log = Log(String::new());
    log.line(&format!("claw-probe {}  (firmware: {} rev {:#x})", env!("CARGO_PKG_VERSION"), system::firmware_vendor(), system::firmware_revision()));
    log.line("");

    let all = list_interfaces();
    log.line(&format!("USB interfaces ({}):", all.len()));
    for it in &all {
        let mode = usb::claw_mode(it.vid, it.pid).map(|m| format!("  <== MSI Claw, {m} mode")).unwrap_or_default();
        log.line(&format!(
            "  {:04x}:{:04x} if{} class {:02x}/{:02x}/{:02x}{mode}",
            it.vid, it.pid, it.number, it.class.0, it.class.1, it.class.2
        ));
        for e in &it.endpoints {
            log.line(&format!(
                "      ep {:02x} attr {:02x} maxpkt {} interval {}",
                e.endpoint_address, e.attributes, { e.max_packet_size }, e.interval
            ));
        }
    }

    let claw: Vec<&Iface> = all.iter().filter(|i| i.vid == usb::MSI_VID).collect();
    log.line("");
    if claw.is_empty() {
        log.line("No MSI (0x0DB0) USB device found. The controller may be off in firmware,");
        log.line("or not enumerated before the boot menu. Nothing more to test.");
    } else {
        log.line("Claw details:");
        let mut dumped = false;
        for it in &claw {
            dump_claw_iface(&mut log, it, &mut dumped);
        }
        for it in claw.iter().filter(|i| i.endpoints.iter().any(|e| usb::CONFIG_EP_IN.contains(&e.endpoint_address))) {
            query_mode(&mut log, it);
        }
        capture(&mut log, &claw);
    }

    log.line("");
    match save(&log) {
        Ok(()) => log.line("Saved \\claw-probe-log.txt on this EFI partition."),
        Err(e) => log.line(&format!("Could not save the log: {e:?} (take a photo of the screen)")),
    }
    log.line("Returning to the boot menu in 10 seconds.");
    boot::stall(Duration::from_secs(10));
    Status::SUCCESS
}
