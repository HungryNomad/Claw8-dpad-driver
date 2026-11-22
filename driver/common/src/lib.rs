//! Platform-independent logic for the MSI Claw built-in gamepad: USB IDs,
//! report parsing (XInput and HID) and the button-to-key mapper.
//!
//! Everything here is pure, `no_std` and unit-tested on the host, so the UEFI
//! crates only glue it to firmware protocols.
#![cfg_attr(not(test), no_std)]

pub mod hid;
pub mod keys;
pub mod pad;
pub mod usb;
