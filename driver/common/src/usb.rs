//! MSI Claw USB identifiers and descriptor helpers.
//!
//! IDs and the config protocol come from the Linux `hid-msi` driver
//! (Derek J. Clark, v13, July 2026).

/// MSI's vendor ID for the Claw gamepad MCU.
pub const MSI_VID: u16 = 0x0DB0;

/// Known Claw gamepad product IDs and the mode each one means.
pub const CLAW_PIDS: [(u16, &str); 4] = [
    (0x1901, "XInput"),
    (0x1902, "DInput"),
    (0x1903, "Desktop"),
    (0x1904, "BIOS"),
];

/// IN endpoints of the vendor config interface (DInput / XInput modes).
pub const CONFIG_EP_IN: [u8; 2] = [0x82, 0x83];

/// Config protocol: output report ID, input report ID, packet size.
pub const CFG_OUT_ID: u8 = 0x0F;
pub const CFG_IN_ID: u8 = 0x10;
pub const CFG_PACKET: usize = 64;
/// Config protocol: "read gamepad mode" command and its reply.
pub const CMD_READ_MODE: u8 = 0x26;
pub const CMD_MODE_ACK: u8 = 0x27;

/// How the driver reads an interface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IfaceKind {
    /// Vendor class 0xFF/0x5D/0x01: Xbox 360 wired protocol.
    XInput,
    /// Non-boot HID interface; needs its report descriptor to confirm.
    Hid,
}

/// Returns the mode name for a Claw PID, or `None` for other devices.
pub fn claw_mode(vid: u16, pid: u16) -> Option<&'static str> {
    if cfg!(feature = "qemu-test") && (vid, pid) == (0x0627, 0x0001) {
        return Some("QEMU tablet (test)");
    }
    (vid == MSI_VID)
        .then(|| CLAW_PIDS.iter().find(|(p, _)| *p == pid).map(|(_, m)| *m))
        .flatten()
}

/// Decides whether an interface of a Claw device can carry gamepad input.
/// Boot keyboards/mice (HID protocol 1/2) are left to the firmware.
pub fn classify(vid: u16, pid: u16, class: u8, subclass: u8, protocol: u8) -> Option<IfaceKind> {
    claw_mode(vid, pid)?;
    match (class, subclass, protocol) {
        (0xFF, 0x5D, 0x01) => Some(IfaceKind::XInput),
        (0x03, _, 0) => Some(IfaceKind::Hid),
        _ => None,
    }
}

/// Builds a 64-byte config command frame: `0f 00 00 3c <cmd> <data..>`.
pub fn config_frame(cmd: u8, data: &[u8]) -> [u8; CFG_PACKET] {
    let mut f = [0u8; CFG_PACKET];
    f[..5].copy_from_slice(&[CFG_OUT_ID, 0, 0, 0x3C, cmd]);
    let n = data.len().min(CFG_PACKET - 5);
    f[5..5 + n].copy_from_slice(&data[..n]);
    f
}

/// Walks a full configuration descriptor and returns the HID report
/// descriptor length declared for interface `iface` (alternate setting 0).
pub fn hid_report_len(config: &[u8], iface: u8) -> Option<u16> {
    let mut in_iface = false;
    let mut rest = config;
    while let [len, kind, ..] = *rest {
        let len = usize::from(len);
        if len < 2 || len > rest.len() {
            return None;
        }
        let d = &rest[..len];
        match kind {
            0x04 if len >= 4 => in_iface = d[2] == iface && d[3] == 0,
            0x21 if in_iface && len >= 9 && d[6] == 0x22 => {
                return Some(u16::from_le_bytes([d[7], d[8]]));
            }
            _ => {}
        }
        rest = &rest[len..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_modes() {
        assert_eq!(classify(MSI_VID, 0x1901, 0xFF, 0x5D, 0x01), Some(IfaceKind::XInput));
        assert_eq!(classify(MSI_VID, 0x1902, 0x03, 0x00, 0x00), Some(IfaceKind::Hid));
        assert_eq!(classify(MSI_VID, 0x1903, 0x03, 0x01, 0x01), None); // boot keyboard
        assert_eq!(classify(0x045E, 0x028E, 0xFF, 0x5D, 0x01), None); // real Xbox pad
    }

    #[test]
    fn finds_report_len() {
        let cfg = [
            9, 2, 34, 0, 1, 1, 0, 0x80, 50, // config
            9, 4, 0, 0, 1, 3, 0, 0, 0, // interface 0, HID
            9, 0x21, 0x11, 0x01, 0, 1, 0x22, 0x3B, 0x01, // HID, report len 0x13B
            7, 5, 0x81, 3, 64, 0, 4, // endpoint
        ];
        assert_eq!(hid_report_len(&cfg, 0), Some(0x13B));
        assert_eq!(hid_report_len(&cfg, 1), None);
    }

    #[test]
    fn frame_layout() {
        let f = config_frame(CMD_READ_MODE, &[]);
        assert_eq!(&f[..6], &[0x0F, 0, 0, 0x3C, 0x26, 0]);
    }
}
