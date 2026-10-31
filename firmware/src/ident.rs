//! opendm40's identity: the firmware version and the meter model.
//!
//! `VERSION` is the one version string, Cargo.toml's package version as it
//! is (0.1.0, 0.1.0-pre2, 0.1.2): *IDN? (also in the 34401A emulation),
//! Settings > Device, README.TXT / BACKUP.TXT, the CSV / log headers.
//! A pull request build in CI appends `-pr<N>.<commit>` (build.rs,
//! DM40_VERSION_SUFFIX). build.sh names the install file
//! `dm40_opendm40_v<VERSION>.bin`. Host tests built without Cargo (no
//! OPENDM40_VERSION / CARGO_PKG_VERSION) get "dev".

use core::sync::atomic::{AtomicU16, Ordering};

const PKG: &str = match option_env!("OPENDM40_VERSION") {
    Some(v) => v,
    None => match option_env!("CARGO_PKG_VERSION") {
        Some(v) => v,
        None => "dev",
    },
};

pub const VERSION: &str = PKG;

/// Model index (0 DM40A, 1 DM40B, 2 DM40C) and the raw calibration byte
/// (record 0x080C0000 + 0x168, docs/model-variants.md), set once by App::new
/// from `Dmm::model()` / `Dmm::model_raw()`. *IDN?, SYSTem:MODel? and the
/// CSV / log headers report it.
static MODEL: AtomicU16 = AtomicU16::new(0);

pub fn set_model(index: u8, raw: u8) {
    MODEL.store(index.min(2) as u16 | (raw as u16) << 8, Ordering::Relaxed);
}

/// (model index, raw calibration byte).
pub fn model() -> (u8, u8) {
    let v = MODEL.load(Ordering::Relaxed);
    (v as u8, (v >> 8) as u8)
}

/// "DM40A" / "DM40B" / "DM40C".
pub fn model_name() -> &'static str {
    match model().0 {
        1 => "DM40B",
        2 => "DM40C",
        _ => "DM40A",
    }
}
