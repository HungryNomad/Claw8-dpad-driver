//! Wire-format-independent button state, plus the XInput (Xbox 360) parser.

use core::ops::{BitAnd, BitOr, BitOrAssign, Not};

/// Bitmask of pressed gamepad buttons, independent of the report format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pad(pub u16);

impl Pad {
    pub const NONE: Self = Self(0);
    pub const UP: Self = Self(1 << 0);
    pub const DOWN: Self = Self(1 << 1);
    pub const LEFT: Self = Self(1 << 2);
    pub const RIGHT: Self = Self(1 << 3);
    pub const A: Self = Self(1 << 4);
    pub const B: Self = Self(1 << 5);
    pub const X: Self = Self(1 << 6);
    pub const Y: Self = Self(1 << 7);
    pub const LB: Self = Self(1 << 8);
    pub const RB: Self = Self(1 << 9);
    pub const START: Self = Self(1 << 10);
    pub const BACK: Self = Self(1 << 11);
    pub const GUIDE: Self = Self(1 << 12);
    /// All four d-pad directions.
    pub const DPAD: Self = Self(0b1111);

    /// Returns `true` if every button in `other` is pressed.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns `true` if no button is pressed.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Pad {
    type Output = Self;

    /// Union of two button sets.
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Pad {
    /// Adds the buttons of `rhs` to `self`.
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Pad {
    type Output = Self;

    /// Intersection of two button sets.
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl Not for Pad {
    type Output = Self;

    /// Complement of a button set.
    fn not(self) -> Self {
        Self(!self.0)
    }
}

/// Converts a normalised 8-way hat value (0 = up, clockwise) to directions.
/// Values outside `0..8` mean "centred".
pub const fn hat_to_pad(value: i32) -> Pad {
    match value {
        0 => Pad::UP,
        1 => Pad(Pad::UP.0 | Pad::RIGHT.0),
        2 => Pad::RIGHT,
        3 => Pad(Pad::DOWN.0 | Pad::RIGHT.0),
        4 => Pad::DOWN,
        5 => Pad(Pad::DOWN.0 | Pad::LEFT.0),
        6 => Pad::LEFT,
        7 => Pad(Pad::UP.0 | Pad::LEFT.0),
        _ => Pad::NONE,
    }
}

/// XInput button bits as `(byte index, mask, button)`, per the Xbox 360 wired
/// input report `00 14 <b2> <b3> <lt> <rt> <lx..> ...`.
const XINPUT_BITS: [(usize, u8, Pad); 13] = [
    (2, 0x01, Pad::UP),
    (2, 0x02, Pad::DOWN),
    (2, 0x04, Pad::LEFT),
    (2, 0x08, Pad::RIGHT),
    (2, 0x10, Pad::START),
    (2, 0x20, Pad::BACK),
    (3, 0x01, Pad::LB),
    (3, 0x02, Pad::RB),
    (3, 0x04, Pad::GUIDE),
    (3, 0x10, Pad::A),
    (3, 0x20, Pad::B),
    (3, 0x40, Pad::X),
    (3, 0x80, Pad::Y),
];

/// Parses an Xbox 360 wired input report. Returns `None` for other message
/// types (LED status, rumble acks, headset messages) and short reads.
pub fn parse_xinput(report: &[u8]) -> Option<Pad> {
    let [0x00, len, ..] = *report else {
        return None;
    };
    if len < 4 || report.len() < 4 {
        return None;
    }
    let pad = XINPUT_BITS
        .iter()
        .filter(|&&(i, mask, _)| report[i] & mask != 0)
        .fold(Pad::NONE, |acc, &(_, _, b)| acc | b);
    Some(pad)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xinput_dpad_and_face_buttons() {
        let mut r = [0u8; 20];
        r[1] = 0x14;
        r[2] = 0x01 | 0x08; // up + right
        r[3] = 0x10 | 0x20; // A + B
        assert_eq!(
            parse_xinput(&r),
            Some(Pad::UP | Pad::RIGHT | Pad::A | Pad::B)
        );
    }

    #[test]
    fn xinput_ignores_non_input_messages() {
        assert_eq!(parse_xinput(&[0x01, 0x03, 0x0e]), None);
        assert_eq!(parse_xinput(&[0x00]), None);
    }

    #[test]
    fn hat_diagonals_and_centre() {
        assert_eq!(hat_to_pad(3), Pad::DOWN | Pad::RIGHT);
        assert_eq!(hat_to_pad(8), Pad::NONE);
        assert_eq!(hat_to_pad(-1), Pad::NONE);
    }
}
