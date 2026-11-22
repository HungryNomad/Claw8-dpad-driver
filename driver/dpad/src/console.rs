//! Keyboard-side protocols: `EFI_SIMPLE_TEXT_INPUT(_EX)_PROTOCOL` backed by
//! the key queue, plus the `ConIn` hook fallback.

use core::ffi::c_void;
use core::ptr;

use claw_common::keys::Key;
use uefi_raw::protocol::console::{
    InputKey, KeyData, KeyNotifyFn, KeyShiftState, KeyState, KeyToggleState,
    SimpleTextInputExProtocol, SimpleTextInputProtocol,
};
use uefi_raw::Event;
use uefi_raw::table::boot::{InterfaceType, OpenProtocolInformationEntry};
use uefi_raw::{Boolean, Guid, Handle, Status, guid};

use crate::state::{Global, Hook, bs, log, st, with};

/// `EFI_CONSOLE_IN_DEVICE_GUID`: tag that tells the console splitter a
/// handle is a console input device.
pub const CONSOLE_IN_DEVICE: Guid = guid!("d3b36f2b-d551-11d4-9a46-0090273fc14d");

const GET_PROTOCOL: u32 = 0x02;
const BY_CHILD_CONTROLLER: u32 = 0x08;
const BY_DRIVER: u32 = 0x10;
const EXCLUSIVE: u32 = 0x20;

/// The protocol instance installed on the gamepad's handle.
pub static TEXT_IN: Global<SimpleTextInputProtocol> = Global::new(SimpleTextInputProtocol {
    reset,
    read_key_stroke,
    wait_for_key: ptr::null_mut(),
});

/// The extended protocol instance installed alongside `TEXT_IN`.
pub static TEXT_IN_EX: Global<SimpleTextInputExProtocol> = Global::new(SimpleTextInputExProtocol {
    reset: reset_ex,
    read_key_stroke_ex,
    wait_for_key_ex: ptr::null_mut(),
    set_state,
    register_key_notify,
    unregister_key_notify,
});

/// Dummy target for key-notify handles (notifications are not delivered).
static NOTIFY_HANDLE: Global<u8> = Global::new(0);

/// Converts our key type to the firmware's.
const fn to_input(k: Key) -> InputKey {
    InputKey { scan_code: k.scan, unicode_char: k.unicode }
}

/// Drops pending keys.
unsafe extern "efiapi" fn reset(_: *mut SimpleTextInputProtocol, _: Boolean) -> Status {
    with(|d| d.keys.clear());
    Status::SUCCESS
}

/// Returns the next pending key, or `NOT_READY`.
unsafe extern "efiapi" fn read_key_stroke(_: *mut SimpleTextInputProtocol, key: *mut InputKey) -> Status {
    if key.is_null() {
        return Status::INVALID_PARAMETER;
    }
    match with(|d| d.keys.pop()) {
        // SAFETY: caller provides a writable key.
        Some(k) => unsafe {
            key.write(to_input(k));
            Status::SUCCESS
        },
        None => Status::NOT_READY,
    }
}

/// Extended reset; same as `reset`.
unsafe extern "efiapi" fn reset_ex(_: *mut SimpleTextInputExProtocol, _: Boolean) -> Status {
    with(|d| d.keys.clear());
    Status::SUCCESS
}

/// Extended read: the key plus a valid, empty shift/toggle state.
unsafe extern "efiapi" fn read_key_stroke_ex(_: *mut SimpleTextInputExProtocol, out: *mut KeyData) -> Status {
    if out.is_null() {
        return Status::INVALID_PARAMETER;
    }
    match with(|d| d.keys.pop()) {
        // SAFETY: caller provides a writable KeyData.
        Some(k) => unsafe {
            out.write(KeyData {
                key: to_input(k),
                key_state: KeyState {
                    key_shift_state: KeyShiftState::VALID,
                    key_toggle_state: KeyToggleState::VALID,
                },
            });
            Status::SUCCESS
        },
        None => Status::NOT_READY,
    }
}

/// No lock LEDs to set; accept silently.
unsafe extern "efiapi" fn set_state(_: *mut SimpleTextInputExProtocol, _: *const KeyToggleState) -> Status {
    Status::SUCCESS
}

/// Accepts registrations so console splitters don't fail as a whole, but
/// never calls them (rEFInd and Ventoy don't use key notifications).
unsafe extern "efiapi" fn register_key_notify(
    _: *mut SimpleTextInputExProtocol,
    key: *const KeyData,
    _: KeyNotifyFn,
    handle: *mut *mut c_void,
) -> Status {
    if key.is_null() || handle.is_null() {
        return Status::INVALID_PARAMETER;
    }
    // SAFETY: caller provides a writable handle slot.
    unsafe { handle.write(NOTIFY_HANDLE.get().cast()) };
    Status::SUCCESS
}

/// Counterpart of `register_key_notify`.
unsafe extern "efiapi" fn unregister_key_notify(_: *mut SimpleTextInputExProtocol, handle: *mut c_void) -> Status {
    if handle == NOTIFY_HANDLE.get().cast() {
        Status::SUCCESS
    } else {
        Status::INVALID_PARAMETER
    }
}

/// `WaitForKey` notify: signals the event while keys are pending.
pub unsafe extern "efiapi" fn wait_notify(event: Event, _: *mut c_void) {
    if !with(|d| d.keys.is_empty()) {
        // SAFETY: `event` is our live wait event.
        let _ = unsafe { (bs().signal_event)(event) };
    }
}

/// Installs both text-input protocols on `controller`.
pub fn install(controller: Handle) -> Status {
    let bs = bs();
    let mut h = controller;
    // SAFETY: the interfaces are statics that live as long as the driver.
    unsafe {
        let st = (bs.install_protocol_interface)(&mut h, &SimpleTextInputProtocol::GUID, InterfaceType::NATIVE_INTERFACE, TEXT_IN.get().cast());
        if st.is_error() {
            return st;
        }
        let st = (bs.install_protocol_interface)(&mut h, &SimpleTextInputExProtocol::GUID, InterfaceType::NATIVE_INTERFACE, TEXT_IN_EX.get().cast());
        if st.is_error() {
            let _ = (bs.uninstall_protocol_interface)(h, &SimpleTextInputProtocol::GUID, TEXT_IN.get().cast());
        }
        st
    }
}

/// Removes everything `install` and `ensure_consumed` put on `controller`.
pub fn uninstall(controller: Handle) {
    let bs = bs();
    // SAFETY: removing our own interfaces; failures leave nothing to undo.
    unsafe {
        let _ = (bs.uninstall_protocol_interface)(controller, &SimpleTextInputExProtocol::GUID, TEXT_IN_EX.get().cast());
        let _ = (bs.uninstall_protocol_interface)(controller, &SimpleTextInputProtocol::GUID, TEXT_IN.get().cast());
        let _ = (bs.uninstall_protocol_interface)(controller, &CONSOLE_IN_DEVICE, ptr::null());
    }
}

/// Returns `true` if a console splitter has taken `controller` as an input
/// device. EDK2's ConSplitter opens the console-device tag by driver and our
/// text input by get-protocol on behalf of its virtual handle; other
/// firmware may open the text input by driver or child controller.
fn consumed(controller: Handle, image: Handle) -> bool {
    [SimpleTextInputProtocol::GUID, CONSOLE_IN_DEVICE].iter().any(|guid| {
        opened_by_other(controller, guid, image, |e| {
            e.attributes & (BY_CHILD_CONTROLLER | BY_DRIVER | EXCLUSIVE) != 0
                || (e.attributes & GET_PROTOCOL != 0 && !e.controller_handle.is_null() && e.controller_handle != controller)
        })
    })
}

/// Returns `true` if an agent other than `image` holds an open of `guid` on
/// `controller` that matches `pred`.
fn opened_by_other(controller: Handle, guid: &Guid, image: Handle, pred: impl Fn(&OpenProtocolInformationEntry) -> bool) -> bool {
    let bs = bs();
    let mut entries = ptr::null_mut();
    let mut count = 0usize;
    // SAFETY: firmware allocates the buffer; we free it below.
    unsafe {
        if (bs.open_protocol_information)(controller, guid, &mut entries, &mut count).is_error() {
            return false;
        }
        let found = core::slice::from_raw_parts(entries, count)
            .iter()
            .any(|e| e.agent_handle != image && pred(e));
        let _ = (bs.free_pool)(entries.cast());
        found
    }
}

/// Makes sure our keys reach `ST->ConIn`. Normally the platform's console
/// splitter has already picked up our protocol. If not, tag the handle as a
/// console device and reconnect; if that still fails, hook `ConIn`.
/// Safe to call repeatedly. Must run at TPL_CALLBACK or below.
pub fn ensure_consumed() {
    let Some((ctrl, image, hooked)) = with(|d| d.dev.as_ref().map(|v| (v.controller, d.image, d.hook.is_some()))) else {
        return;
    };
    if hooked {
        return;
    }
    if consumed(ctrl, image) {
        log("keys reach ConIn through the console splitter", None);
        return;
    }
    log("console splitter has not picked us up; tagging and reconnecting", None);
    let bs = bs();
    let mut h = ctrl;
    // SAFETY: a NULL-interface tag protocol on our controller; reconnecting
    // lets ConPlatform/ConSplitter bind to it.
    unsafe {
        let mut probe = ptr::null_mut();
        let tagged = (bs.open_protocol)(ctrl, &CONSOLE_IN_DEVICE, &mut probe, image, ctrl, 0x04).is_success();
        if !tagged {
            let _ = (bs.install_protocol_interface)(&mut h, &CONSOLE_IN_DEVICE, InterfaceType::NATIVE_INTERFACE, ptr::null());
        }
        let _ = (bs.connect_controller)(ctrl, ptr::null(), ptr::null(), Boolean::FALSE);
    }
    if consumed(ctrl, image) {
        log("console splitter picked us up after reconnect", None);
    } else {
        log("falling back to the ConIn hook", None);
        hook_con_in();
    }
}

/// Replacement `ConIn.ReadKeyStroke`: our keys first, then the original.
unsafe extern "efiapi" fn hooked_read(this: *mut SimpleTextInputProtocol, key: *mut InputKey) -> Status {
    let (next, hook) = with(|d| (d.keys.pop(), d.hook));
    match (next, hook) {
        (Some(k), _) if !key.is_null() => {
            // SAFETY: caller provides a writable key.
            unsafe { key.write(to_input(k)) };
            Status::SUCCESS
        }
        // SAFETY: forwarding to the firmware's own implementation.
        (_, Some(h)) => unsafe { (h.orig_read)(this, key) },
        _ => Status::NOT_READY,
    }
}

/// Wraps `ST->ConIn.ReadKeyStroke` (fallback for firmware whose console
/// splitter ignores late devices). Keys also signal `ConIn.WaitForKey`.
fn hook_con_in() {
    let con_in = st().stdin;
    if con_in.is_null() {
        return;
    }
    // SAFETY: the console protocol lives in firmware pool memory for the
    // whole boot; we only swap one function pointer and remember the old one.
    unsafe {
        let orig_read = (*con_in).read_key_stroke;
        (*con_in).read_key_stroke = hooked_read;
        with(|d| d.hook = Some(Hook { con_in, orig_read }));
    }
}

/// Restores `ConIn.ReadKeyStroke` if we hooked it.
pub fn unhook_con_in() {
    if let Some(h) = with(|d| d.hook.take()) {
        // SAFETY: undoing `hook_con_in` on the same live protocol.
        unsafe {
            let ours: unsafe extern "efiapi" fn(_, _) -> Status = hooked_read;
            if ptr::fn_addr_eq((*h.con_in).read_key_stroke, ours) {
                (*h.con_in).read_key_stroke = h.orig_read;
            }
        }
    }
}
