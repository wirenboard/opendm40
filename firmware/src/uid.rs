//! The unit's serial number: the GD32F427's 96-bit unique ID (0x1FFF7A10,
//! 12 bytes) as 24 uppercase hex digits, bytes in memory order (word 0 low
//! byte first): the same digits as `xxd -p -u INFO/UID.BIN` on the USB volume.
//!
//! Used for the USB iSerial, the SCSI unit serial
//! number (INQUIRY VPD page 0x80, msc.rs) and the serial field of *IDN?
//! (scpi.rs; not in the SYST:IDN:EMUL 34401A identity). Before 2026-10-23
//! every unit said "0001" (USB) / "0" (*IDN?): udisks merges disks with the
//! same vendor / model / serial into one drive, so two meters (or the
//! emulator next to a meter) shared one udisks drive and an Eject / Power
//! Off could act on the other one.
//!
//! The UID is in system memory (0x1FFF7A10..0x1FFF7A1B), not in the main
//! flash's slow bank (0x08080000..), but it is still read once: `init` in
//! native::start, before the normal boot, into a
//! TCM static. Privacy: docs/native-boot.md "USB serial number".

/// The 96-bit unique ID (GD32F4xx system memory).
pub const ADDR: u32 = 0x1FFF_7A10;

/// 24 hex digits; '0's until `init`.
static mut HEX: [u8; 24] = [b'0'; 24];

/// Read the UID and keep it as hex (native::start, once, before USB).
#[inline(never)]
pub fn init() {
    let mut h = [0u8; 24];
    for i in 0..3 {
        // SAFETY: fixed, always-mapped read-only system memory; no side effects.
        let w = unsafe { core::ptr::read_volatile((ADDR + 4 * i as u32) as *const u32) };
        for (j, b) in w.to_le_bytes().iter().enumerate() {
            let k = 8 * i + 2 * j;
            h[k] = digit(b >> 4);
            h[k + 1] = digit(b & 15);
        }
    }
    unsafe { *core::ptr::addr_of_mut!(HEX) = h };
}

fn digit(n: u8) -> u8 {
    if n < 10 { b'0' + n } else { b'A' + n - 10 }
}

/// The serial as ASCII bytes (24).
pub fn bytes() -> &'static [u8; 24] {
    unsafe { &*core::ptr::addr_of!(HEX) }
}

/// The serial as a string (USB string descriptor).
pub fn serial() -> &'static str {
    // SAFETY: only ASCII hex digits are ever stored.
    unsafe { core::str::from_utf8_unchecked(bytes()) }
}
