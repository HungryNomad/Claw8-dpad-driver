//! Button-to-key mapping with d-pad auto-repeat.

use crate::pad::Pad;

/// A key as UEFI's `EFI_INPUT_KEY` sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub scan: u16,
    pub unicode: u16,
}

impl Key {
    pub const UP: Self = Self::scan(0x01);
    pub const DOWN: Self = Self::scan(0x02);
    pub const RIGHT: Self = Self::scan(0x03);
    pub const LEFT: Self = Self::scan(0x04);
    pub const HOME: Self = Self::scan(0x05);
    pub const END: Self = Self::scan(0x06);
    pub const F2: Self = Self::scan(0x0C);
    pub const ESC: Self = Self::scan(0x17);
    pub const ENTER: Self = Self { scan: 0, unicode: 0x0D };

    /// A key identified only by its UEFI scan code.
    const fn scan(code: u16) -> Self {
        Self { scan: code, unicode: 0 }
    }
}

/// Button-to-key table. The d-pad entries come first; only they repeat.
///
/// rEFInd: arrows move, Enter boots, Esc backs out, F2 opens the selected
/// entry's options submenu, Home/End jump to the first/last entry.
pub const KEYMAP: [(Pad, Key); 10] = [
    (Pad::UP, Key::UP),
    (Pad::DOWN, Key::DOWN),
    (Pad::LEFT, Key::LEFT),
    (Pad::RIGHT, Key::RIGHT),
    (Pad::A, Key::ENTER),
    (Pad::START, Key::ENTER),
    (Pad::B, Key::ESC),
    (Pad::Y, Key::F2),
    (Pad::LB, Key::HOME),
    (Pad::RB, Key::END),
];

/// Turns button states into key presses: one key per new press, and
/// keyboard-style repeat while a single d-pad direction is held.
#[derive(Debug, Default)]
pub struct Mapper {
    prev: Pad,
    repeat: Option<(Pad, Key)>,
    next_repeat_ms: u32,
}

impl Mapper {
    /// Hold time before a d-pad direction starts repeating.
    pub const REPEAT_DELAY_MS: u32 = 400;
    /// Interval between repeats once repeating.
    pub const REPEAT_RATE_MS: u32 = 110;

    /// Creates a mapper with nothing held.
    pub const fn new() -> Self {
        Self { prev: Pad::NONE, repeat: None, next_repeat_ms: 0 }
    }

    /// Feeds a new button state seen at `now_ms`, calling `emit` for each key
    /// a newly pressed button maps to.
    pub fn update(&mut self, pad: Pad, now_ms: u32, mut emit: impl FnMut(Key)) {
        let pressed = pad & !self.prev;
        for &(button, key) in &KEYMAP {
            if pressed.contains(button) && !button.is_empty() {
                emit(key);
            }
        }
        if let Some(&(button, key)) = KEYMAP
            .iter()
            .find(|(b, _)| Pad::DPAD.contains(*b) && pressed.contains(*b))
        {
            self.repeat = Some((button, key));
            self.next_repeat_ms = now_ms.wrapping_add(Self::REPEAT_DELAY_MS);
        } else if self.repeat.is_some_and(|(b, _)| !pad.contains(b)) {
            self.repeat = None;
        }
        self.prev = pad;
    }

    /// Advances time to `now_ms`, emitting a repeat if one is due.
    pub fn tick(&mut self, now_ms: u32, mut emit: impl FnMut(Key)) {
        if let Some((_, key)) = self.repeat
            && now_ms.wrapping_sub(self.next_repeat_ms) as i32 >= 0
        {
            emit(key);
            self.next_repeat_ms = now_ms.wrapping_add(Self::REPEAT_RATE_MS);
        }
    }

    /// Forgets all held buttons (e.g. after a transfer error).
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collects the keys one call emits.
    fn keys(f: impl FnOnce(&mut dyn FnMut(Key))) -> Vec<Key> {
        let mut v = Vec::new();
        f(&mut |k| v.push(k));
        v
    }

    #[test]
    fn press_emits_once_release_emits_nothing() {
        let mut m = Mapper::new();
        assert_eq!(keys(|e| m.update(Pad::A, 0, e)), [Key::ENTER]);
        assert_eq!(keys(|e| m.update(Pad::A, 10, e)), []);
        assert_eq!(keys(|e| m.update(Pad::NONE, 20, e)), []);
        assert_eq!(keys(|e| m.update(Pad::B, 30, e)), [Key::ESC]);
    }

    #[test]
    fn dpad_repeats_after_delay() {
        let mut m = Mapper::new();
        assert_eq!(keys(|e| m.update(Pad::DOWN, 0, e)), [Key::DOWN]);
        assert_eq!(keys(|e| m.tick(399, e)), []);
        assert_eq!(keys(|e| m.tick(400, e)), [Key::DOWN]);
        assert_eq!(keys(|e| m.tick(450, e)), []);
        assert_eq!(keys(|e| m.tick(510, e)), [Key::DOWN]);
        m.update(Pad::NONE, 520, |_| {});
        assert_eq!(keys(|e| m.tick(2000, e)), []);
    }

    #[test]
    fn face_buttons_do_not_repeat() {
        let mut m = Mapper::new();
        m.update(Pad::A, 0, |_| {});
        assert_eq!(keys(|e| m.tick(5000, e)), []);
    }
}
