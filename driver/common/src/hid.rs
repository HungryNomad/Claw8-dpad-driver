//! Minimal HID report-descriptor parser.
//!
//! It only finds what a menu needs: the hat switch and buttons 1..=16 inside a
//! Gamepad or Joystick application collection, all from one input report.
//! That is enough for the Claw's DInput mode without a general HID stack.

use crate::pad::{Pad, hat_to_pad};

/// Highest button number (1-based) the layout records.
pub const MAX_BUTTONS: usize = 16;

const USAGE_JOYSTICK: u32 = 0x0001_0004;
const USAGE_GAMEPAD: u32 = 0x0001_0005;
const USAGE_HAT: u32 = 0x0001_0039;
/// Generic Desktop Mouse, accepted only by the `qemu-test` feature.
const USAGE_MOUSE: u32 = 0x0001_0002;
const PAGE_BUTTON: u32 = 0x0009;

/// Where one value sits in an input report (bit offset after the report ID).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub bit: u16,
    pub size: u8,
    pub logical_min: i32,
}

/// The parts of a gamepad report the driver decodes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// Report ID prefixing the report, if the descriptor uses IDs.
    pub report_id: Option<u8>,
    pub hat: Option<Field>,
    /// `buttons[n]` is HID Button `n + 1`.
    pub buttons: [Option<Field>; MAX_BUTTONS],
}

/// Global item state (pushed/popped as a unit).
#[derive(Clone, Copy, Default)]
struct Globals {
    page: u32,
    logical_min: i32,
    size: u32,
    count: u32,
    report_id: Option<u8>,
}

/// Local item state, cleared after every main item.
#[derive(Default)]
struct Locals {
    usages: [u32; 16],
    n: usize,
    min: Option<u32>,
    max: Option<u32>,
}

impl Locals {
    /// Resolves the usage of the `k`-th value of the next main item.
    fn usage(&self, k: u32) -> Option<u32> {
        match (self.min, self.max) {
            (Some(lo), Some(hi)) => Some((lo + k).min(hi)),
            (Some(lo), None) => Some(lo + k),
            _ if self.n > 0 => Some(self.usages[(k as usize).min(self.n - 1)]),
            _ => None,
        }
    }
}

/// Bit cursors per report ID (index 0 is "no report ID").
struct Cursors([(u8, u32); 8]);

impl Cursors {
    /// Returns the bit cursor for `id`, creating it if needed.
    fn get(&mut self, id: Option<u8>) -> &mut u32 {
        let key = id.unwrap_or(0);
        let slot = self
            .0
            .iter()
            .position(|&(k, c)| k == key && (c != u32::MAX))
            .or_else(|| self.0.iter().position(|&(_, c)| c == u32::MAX))
            .unwrap_or(7);
        if self.0[slot].1 == u32::MAX {
            self.0[slot] = (key, 0);
        }
        &mut self.0[slot].1
    }
}

/// Reads an unsigned little-endian item payload.
fn payload(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .rev()
        .fold(0u32, |acc, &b| (acc << 8) | u32::from(b))
}

/// Sign-extends an item payload of `len` bytes.
fn signed(v: u32, len: usize) -> i32 {
    match len {
        1 => v as u8 as i8 as i32,
        2 => v as u16 as i16 as i32,
        _ => v as i32,
    }
}

/// Parses a report descriptor. Returns `None` if it has no gamepad/joystick
/// collection with a hat switch or at least one button.
pub fn parse(desc: &[u8]) -> Option<Layout> {
    let mut g = Globals::default();
    let mut stack = [Globals::default(); 4];
    let mut sp = 0usize;
    let mut l = Locals::default();
    let mut cursors = Cursors([(0, u32::MAX); 8]);
    let mut depth = 0u32;
    let mut app_depth: Option<u32> = None;
    let mut out = Layout::default();
    let mut found = false;
    let mut i = 0usize;

    while i < desc.len() {
        let prefix = desc[i];
        if prefix == 0xFE {
            // Long item: 0xFE, size, tag, data...
            i += 3 + usize::from(*desc.get(i + 1)?);
            continue;
        }
        let len = [0usize, 1, 2, 4][usize::from(prefix & 3)];
        let data = desc.get(i + 1..i + 1 + len)?;
        let v = payload(data);
        i += 1 + len;

        match prefix & 0xFC {
            0x80 => {
                // Input
                let bits = g.size * g.count;
                let cursor = cursors.get(g.report_id);
                let base = *cursor;
                *cursor += bits;
                let constant = v & 1 != 0;
                let id_ok = !found || out.report_id == g.report_id;
                if app_depth.is_some() && !constant && id_ok && g.size > 0 && g.size <= 32 {
                    for k in 0..g.count {
                        let Some(usage) = l.usage(k) else { break };
                        let field = Field {
                            bit: (base + k * g.size) as u16,
                            size: g.size as u8,
                            logical_min: g.logical_min,
                        };
                        let slot = if usage == USAGE_HAT {
                            &mut out.hat
                        } else if usage >> 16 == PAGE_BUTTON {
                            match (usage & 0xFFFF) as usize {
                                n @ 1..=MAX_BUTTONS => &mut out.buttons[n - 1],
                                _ => continue,
                            }
                        } else {
                            continue;
                        };
                        if slot.is_none() {
                            *slot = Some(field);
                            out.report_id = g.report_id;
                            found = true;
                        }
                    }
                }
                l = Locals::default();
            }
            0xA0 => {
                // Collection
                depth += 1;
                let usage = l.usage(0);
                let wanted = matches!(usage, Some(USAGE_GAMEPAD | USAGE_JOYSTICK))
                    || (cfg!(feature = "qemu-test") && usage == Some(USAGE_MOUSE));
                if v == 1 && app_depth.is_none() && wanted {
                    app_depth = Some(depth);
                }
                l = Locals::default();
            }
            0xC0 => {
                // End Collection
                if app_depth == Some(depth) {
                    app_depth = None;
                    if found {
                        break;
                    }
                }
                depth = depth.saturating_sub(1);
                l = Locals::default();
            }
            0x90 | 0xB0 => l = Locals::default(), // Output, Feature
            0x04 => g.page = v,
            0x14 => g.logical_min = signed(v, len),
            0x74 => g.size = v,
            0x84 => g.report_id = Some(v as u8),
            0x94 => g.count = v,
            0xA4 => {
                // Push
                if sp < stack.len() {
                    stack[sp] = g;
                    sp += 1;
                }
            }
            0xB4 => {
                // Pop
                if sp > 0 {
                    sp -= 1;
                    g = stack[sp];
                }
            }
            0x08 => {
                // Usage (4-byte form carries its own page)
                if l.n < l.usages.len() {
                    l.usages[l.n] = if len == 4 { v } else { (g.page << 16) | v };
                    l.n += 1;
                }
            }
            0x18 => l.min = Some(if len == 4 { v } else { (g.page << 16) | v }),
            0x28 => l.max = Some(if len == 4 { v } else { (g.page << 16) | v }),
            _ => {}
        }
    }
    found.then_some(out)
}

/// Reads `size` bits at `bit` (LSB first) from `data`.
fn read_bits(data: &[u8], bit: u16, size: u8) -> Option<u32> {
    (0..u16::from(size)).try_fold(0u32, |acc, k| {
        let b = bit + k;
        let byte = *data.get(usize::from(b / 8))?;
        Some(acc | (u32::from((byte >> (b % 8)) & 1) << k))
    })
}

/// HID button number (1-based) to pad button. Guessed from the common
/// Xbox-style DInput order; confirm against a probe capture.
pub const HID_BUTTONS: [Pad; MAX_BUTTONS] = [
    Pad::A,
    Pad::B,
    Pad::X,
    Pad::Y,
    Pad::LB,
    Pad::RB,
    Pad::BACK,
    Pad::START,
    Pad::NONE,
    Pad::NONE,
    Pad::NONE,
    Pad::NONE,
    Pad::NONE,
    Pad::NONE,
    Pad::NONE,
    Pad::NONE,
];

impl Layout {
    /// Decodes an input report. Returns `None` for reports with another ID or
    /// too short to hold the fields.
    pub fn decode(&self, report: &[u8]) -> Option<Pad> {
        let body = match self.report_id {
            Some(id) => report.strip_prefix(&[id])?,
            None => report,
        };
        let mut pad = Pad::NONE;
        if let Some(h) = self.hat {
            pad |= hat_to_pad(read_bits(body, h.bit, h.size)? as i32 - h.logical_min);
        }
        for (field, &button) in self.buttons.iter().zip(&HID_BUTTONS) {
            if let Some(f) = field
                && read_bits(body, f.bit, f.size)? != 0
            {
                pad |= button;
            }
        }
        Some(pad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typical gamepad: report ID 1, 12 buttons, 4 pad bits, hat, X/Y.
    const DESC: &[u8] = &[
        0x05, 0x01, // Usage Page (Generic Desktop)
        0x09, 0x05, // Usage (Game Pad)
        0xA1, 0x01, // Collection (Application)
        0x85, 0x01, //   Report ID 1
        0x05, 0x09, //   Usage Page (Button)
        0x19, 0x01, 0x29, 0x0C, // Usage Min 1, Max 12
        0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x0C, 0x81, 0x02, // 12 x 1 bit
        0x95, 0x04, 0x81, 0x03, //   4 bits padding (constant)
        0x05, 0x01, 0x09, 0x39, //   Usage (Hat switch)
        0x15, 0x00, 0x25, 0x07, 0x75, 0x04, 0x95, 0x01, 0x81, 0x42,
        0x95, 0x01, 0x81, 0x03, //   4 bits padding
        0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x02,
        0xC0,
    ];

    #[test]
    fn finds_hat_and_buttons() {
        let l = parse(DESC).expect("gamepad layout");
        assert_eq!(l.report_id, Some(1));
        assert_eq!(l.buttons[0].map(|f| f.bit), Some(0));
        assert_eq!(l.buttons[11].map(|f| f.bit), Some(11));
        assert_eq!(l.buttons[12], None);
        assert_eq!(l.hat, Some(Field { bit: 16, size: 4, logical_min: 0 }));
    }

    #[test]
    fn decodes_reports() {
        let l = parse(DESC).unwrap();
        // A pressed, hat = 6 (left), sticks centred.
        assert_eq!(l.decode(&[1, 0x01, 0x00, 0x06, 0, 0]), Some(Pad::A | Pad::LEFT));
        // Hat 0x0F = out of range = centred.
        assert_eq!(l.decode(&[1, 0x00, 0x00, 0x0F, 0, 0]), Some(Pad::NONE));
        // Wrong report ID.
        assert_eq!(l.decode(&[2, 0x01, 0x00, 0x06]), None);
    }

    #[test]
    fn rejects_non_gamepad() {
        // Vendor-defined collection (like the Claw's config interface).
        let desc = [
            0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x10, 0x15, 0x00, 0x26, 0xFF, 0x00,
            0x75, 0x08, 0x95, 0x3F, 0x09, 0x01, 0x81, 0x02, 0xC0,
        ];
        assert_eq!(parse(&desc), None);
    }
}
