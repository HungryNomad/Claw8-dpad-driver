//! Driver-wide state, firmware table access and the TPL lock.

use core::cell::UnsafeCell;
use core::ptr;

use claw_common::hid::Layout;
use claw_common::keys::{Key, Mapper};
use uefi_raw::protocol::console::SimpleTextInputProtocol;
use uefi_raw::protocol::usb::io::UsbIoProtocol;
use uefi_raw::table::boot::{BootServices, Tpl};
use uefi_raw::table::system::SystemTable;
use uefi_raw::{Event, Handle, Status};

/// Interior-mutable static for data shared with firmware callbacks.
pub struct Global<T>(UnsafeCell<T>);

// SAFETY: UEFI runs one CPU thread; concurrent access only comes from event
// callbacks, which `with` excludes by raising the TPL.
unsafe impl<T> Sync for Global<T> {}

impl<T> Global<T> {
    /// Wraps a value.
    pub const fn new(v: T) -> Self {
        Self(UnsafeCell::new(v))
    }

    /// Raw pointer to the value (stable address, usable as a protocol interface).
    pub const fn get(&self) -> *mut T {
        self.0.get()
    }
}

/// How reports from the bound interface are decoded.
// Stored once in a static, so the size gap between variants costs nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Copy)]
pub enum Decoder {
    XInput,
    Hid(Layout),
}

/// The gamepad interface the driver is bound to.
pub struct Device {
    pub controller: Handle,
    pub usb: *mut UsbIoProtocol,
    pub ep_in: u8,
    pub max_packet: usize,
    pub interval_ms: usize,
    pub decoder: Decoder,
    /// Set by the USB callback on a transfer error; the tick timer recovers.
    pub restart_at_ms: Option<u32>,
}

/// `ConIn` read hook used only when no console splitter consumes our protocol.
#[derive(Clone, Copy)]
pub struct Hook {
    pub con_in: *mut SimpleTextInputProtocol,
    pub orig_read: unsafe extern "efiapi" fn(*mut SimpleTextInputProtocol, *mut uefi_raw::protocol::console::InputKey) -> Status,
}

/// Small FIFO of pending key presses.
pub struct KeyQueue {
    buf: [Key; 16],
    head: u8,
    len: u8,
}

impl KeyQueue {
    /// Creates an empty queue.
    pub const fn new() -> Self {
        Self { buf: [Key { scan: 0, unicode: 0 }; 16], head: 0, len: 0 }
    }

    /// Appends a key, dropping it if the queue is full.
    pub fn push(&mut self, k: Key) {
        if usize::from(self.len) < self.buf.len() {
            self.buf[(usize::from(self.head) + usize::from(self.len)) % self.buf.len()] = k;
            self.len += 1;
        }
    }

    /// Removes the oldest key.
    pub fn pop(&mut self) -> Option<Key> {
        (self.len > 0).then(|| {
            let k = self.buf[usize::from(self.head)];
            self.head = ((usize::from(self.head) + 1) % self.buf.len()) as u8;
            self.len -= 1;
            k
        })
    }

    /// Returns `true` if no key is pending.
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Drops all pending keys.
    pub fn clear(&mut self) {
        self.len = 0;
    }
}

/// Everything mutable the driver owns.
pub struct Driver {
    pub image: Handle,
    pub dev: Option<Device>,
    pub keys: KeyQueue,
    pub mapper: Mapper,
    pub now_ms: u32,
    pub hook: Option<Hook>,
    pub wait_event: Event,
    pub tick_event: Event,
    pub verify_event: Event,
}

impl Driver {
    /// Queues a key and, when the `ConIn` hook is active, wakes `ConIn` waiters.
    pub fn emit(&mut self, k: Key) {
        self.keys.push(k);
        if let Some(h) = self.hook {
            // SAFETY: `con_in` is the live console protocol saved by the hook.
            let _ = unsafe { (bs().signal_event)((*h.con_in).wait_for_key) };
        }
    }
}

static DRIVER: Global<Driver> = Global::new(Driver {
    image: ptr::null_mut(),
    dev: None,
    keys: KeyQueue::new(),
    mapper: Mapper::new(),
    now_ms: 0,
    hook: None,
    wait_event: ptr::null_mut(),
    tick_event: ptr::null_mut(),
    verify_event: ptr::null_mut(),
});

/// Runs `f` on the driver state with USB callbacks and timers masked.
/// Must not be nested.
pub fn with<R>(f: impl FnOnce(&mut Driver) -> R) -> R {
    let bs = bs();
    // SAFETY: raising to TPL_NOTIFY from any lower or equal level is allowed;
    // while raised no other code touches `DRIVER`.
    unsafe {
        let old = (bs.raise_tpl)(Tpl::NOTIFY);
        let r = f(&mut *DRIVER.get());
        (bs.restore_tpl)(old);
        r
    }
}

/// The firmware system table.
pub fn st() -> &'static SystemTable {
    // SAFETY: set by the `#[entry]` glue before any of our code runs, and
    // valid until ExitBootServices (after which this driver is never called).
    unsafe { uefi::table::system_table_raw().expect("system table").cast().as_ref() }
}

/// The firmware boot services.
pub fn bs() -> &'static BootServices {
    // SAFETY: see `st`.
    unsafe { &*st().boot_services }
}

/// Prints `claw-dpad: <msg> <code>` on the console when built with the
/// `verbose` feature; compiles to nothing otherwise. `code` (if any) is shown
/// in hex, e.g. an EFI status.
pub fn log(msg: &str, code: Option<usize>) {
    if !cfg!(feature = "verbose") {
        return;
    }
    let out = st().stdout;
    if out.is_null() {
        return;
    }
    let mut buf = [0u16; 128];
    let mut n = 0;
    let mut put = |c: u8| {
        if n < buf.len() - 1 {
            buf[n] = u16::from(c);
            n += 1;
        }
    };
    "claw-dpad: ".bytes().chain(msg.bytes()).for_each(&mut put);
    if let Some(v) = code {
        put(b' ');
        put(b'0');
        put(b'x');
        let digits = (usize::BITS - v.leading_zeros()).div_ceil(4).max(1);
        (0..digits).rev().for_each(|i| put(b"0123456789abcdef"[(v >> (i * 4)) & 0xF]));
    }
    "\r\n".bytes().for_each(&mut put);
    // SAFETY: `buf` is NUL-terminated (zero-initialised, last slot never written).
    let _ = unsafe { ((*out).output_string)(out, buf.as_ptr()) };
}
