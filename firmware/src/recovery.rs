//! Recovery mode (the shim decision). The shim takes it only on the
//! backup-SRAM request word (boot::REQ_MAGIC), which nothing sets now; the
//! recovery-file / recovery-usb variants below replace this action.
//!
//! Hands recovery to the vendor FS-loader's update mode by zeroing the 12-byte
//! app marker at 0x08008000 (boot::zero_marker: FMC word programs of 0, no
//! erase, as the loader itself does when it finds a dm40*.bin), then
//! SYSRESETREQ. The ROM and the loader both reject an app whose marker is not
//! valid, on any reset (cold or warm, whatever PA2 reads), so the loader shows
//! "open file error" and brings up its own working USB disk (28e9:028f) on
//! every boot until a valid dm40*.bin is installed through it
//! (stock/re/docs/fsloader-install-trigger.md, "Update-loader install & error paths").
//! The app bytes stay intact but the app cannot run again until that install:
//! this is a one-way door into the vendor updater, by design.
//!
//! User procedure: the meter restarts into the loader's update screen and a
//! USB disk appears -> copy a firmware dm40*.bin (the native opendm40 image, or
//! dm40_v_2_6.bin) onto it -> press "update" on the screen (or power-cycle).
//!
//! Recovery needs no SPI flash, FAT, USB, LCD, fonts, touch, HY3131, scope,
//! generator, BT or beeper: power hold (PE2), the FWDGT fed around the flash
//! operation, 3 word programs, a reset. The read-back decides: if the three
//! words do not read back as 0 (FMC error flags are only recorded, see
//! boot::fmc_last), it records the failure in backup SRAM
//! (BKP_TRIG = 0x54520011) and stops feeding the watchdog: the FWDGT resets,
//! and the shim (previous mode = recovery) boots normally.
//!
//! Non-default alternatives, kept for tests:
//!   `recovery-file`: write a 0-byte DM40RCVR.BIN root entry on the SPI FAT
//!     and power off; the next boot reaches the loader only with PA2 held low
//!     (PA2 = the debug header's TX pad, not the power key: hw/README.md
//!     "Hardware recovery"), which then zeroes the marker itself. Fragile:
//!     with PA2 high the ROM launches the app directly.
//!   `recovery-usb`: a USB mass-storage-only device on the W25Q128 (tested
//!     msc.rs path, no USBTMC); eject (any SCSI START STOP UNIT) or a 2 s key
//!     hold powers off, a short press reboots.

#[cfg(feature = "recovery-usb")]
use core::sync::atomic::Ordering;

#[cfg(feature = "recovery-usb")]
use usb_device::prelude::*;

use crate::boot;
#[cfg(any(feature = "recovery-usb", feature = "recovery-file"))]
use crate::clock::Clock;
#[cfg(feature = "recovery-usb")]
use crate::{msc, spiflash, usb};

/// Recovery entry.
pub fn run() -> ! {
    boot::feed();
    boot::latch_on();
    #[cfg(feature = "recovery-usb")]
    usb_disk(spiflash::SpiFlash::new());
    #[cfg(feature = "recovery-file")]
    {
        let mut disk = crate::spiflash::SpiFlash::new();
        let t = crate::fatren::make_trigger(&mut disk);
        boot::record_trigger(t as u32);
        power_off();
    }
    #[allow(unreachable_code)]
    {
        if boot::zero_marker() {
            boot::record_trigger(0x10);
            boot::sysreset();
        }
        boot::record_trigger(0x11);
        loop {
            core::hint::spin_loop();
        }
    }
}

/// `recovery-file`: drop the power hold, feed the watchdog 2 s, then stop.
#[cfg(feature = "recovery-file")]
fn power_off() -> ! {
    boot::latch_off();
    let mut clk = Clock::new();
    let t0 = clk.now_ms();
    while clk.now_ms() - t0 < 2000 {
        boot::feed();
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(feature = "recovery-usb")]
const SHORT_MIN_MS: u64 = 30;
#[cfg(feature = "recovery-usb")]
const SHORT_MAX_MS: u64 = 1500;
#[cfg(feature = "recovery-usb")]
const OFF_HOLD_MS: u64 = 2000;
#[cfg(feature = "recovery-usb")]
const ARM_MS: u64 = 50;
#[cfg(feature = "recovery-usb")]
const EJECT_DELAY_MS: u64 = 1000;

/// `recovery-usb` only: USB mass storage on the SPI flash.
#[cfg(feature = "recovery-usb")]
fn usb_disk(mut disk: spiflash::SpiFlash) -> ! {
    let mut clk = Clock::new();
    let hw = usb::init_hw(&mut clk);
    let _ = hw;
    boot::feed();
    let bus = synopsys_usb_otg::UsbBus::new(usb::UsbFs, unsafe { &mut *core::ptr::addr_of_mut!(crate::EP_MEMORY) });
    let mut msc = msc::Msc::new(
        &bus,
        unsafe { &mut *core::ptr::addr_of_mut!(crate::MSC_IO) },
        unsafe { &mut *core::ptr::addr_of_mut!(crate::MSC_BLK) },
        1,
    );
    let mut dev = UsbDeviceBuilder::new(&bus, crate::USB_VID_PID)
        .strings(&[StringDescriptors::default()
            .manufacturer("opendm40")
            .product("DM40 recovery disk (opendm40)")
            .serial_number(crate::uid::serial())])
        .unwrap()
        .build();
    usb::force_power_no_vbus();
    msc::STOP_UNIT.store(false, Ordering::Relaxed);

    let mut armed = false;
    let mut released_since = clk.now_ms();
    let mut pressed_since: Option<u64> = None;
    let mut eject_at: Option<u64> = None;
    let mut off = false;

    loop {
        boot::feed();
        let now = clk.now_ms();
        dev.poll(&mut [&mut msc]);
        msc.poll(&mut [&mut disk]);

        if eject_at.is_none() && msc::STOP_UNIT.load(Ordering::Relaxed) {
            eject_at = Some(now + EJECT_DELAY_MS);
        }
        if !off && eject_at.map(|t| now >= t).unwrap_or(false) {
            off = true;
            boot::latch_off();
        }

        let key = boot::pd6_pressed();
        if !armed {
            if key {
                released_since = now;
            } else if now - released_since >= ARM_MS {
                armed = true;
            }
            continue;
        }
        match (key, pressed_since) {
            (true, None) => pressed_since = Some(now),
            (true, Some(t)) if !off && now - t >= OFF_HOLD_MS => {
                off = true;
                boot::latch_off();
            }
            (false, Some(t)) => {
                pressed_since = None;
                let d = now - t;
                if (SHORT_MIN_MS..=SHORT_MAX_MS).contains(&d) {
                    boot::sysreset();
                }
            }
            _ => {}
        }
    }
}
