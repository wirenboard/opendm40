//! Wrong-connection detection ("misconnect sentinel"),
//! docs/power-meter-and-misconnect.md section 4.
//!
//! In every non-current mode the A jack rests on the 47.5 ohm uA shunt
//! (PE0 = PE1 = 0, always in circuit), so a current in the A jack - the
//! leads in A/COM while the meter measures V / ohm / temperature, or a
//! source across A-COM - shows as a voltage on the shunt node PB1. Each mode
//! watches it with a block of the HY3131 that its stock row leaves idle
//! (`row_for`; the per-function table with the row overrides and the
//! thresholds is doc 4.2): the comparator on PB1 in DC V 6 V, DC mV, ohm and
//! temperature (`CmpA`); the AD1 peak hold on PB1 - PB3 in AC V / AC mV
//! (`PeakA`; frequency: a one-sided level only, `pk_dc`); the AD2 peak hold on OP2 <- PB1 in continuity, diode and
//! capacitance r0..4 (`Ad2A`); V-JACK? (a voltage on V/ohm while measuring
//! current) by the comparator on SENSE via the /1000 tap in DC A (`CmpV`)
//! and the AD1 peak hold on SENSE - AGND in AC A (`PeakV`). AC+DC carries
//! the sentinels of its two phases (the DC phase's R20 through dmm.rs `acdc_src`).
//! No sentinel: DC V 60 V+ (and so AC+DC V above 6 V),
//! capacitance r5..7, the power meter. Host tests: the byte diff against the
//! stock rows is exactly the intended fields. In ohm, continuity and the
//! resistive sensors a hit is checked for a source on V-COM (JACK CHECK, 4.13).
//! "LIVE?" (a source on the circuit in ohm / continuity / diode): a negative
//! result or a driven input node (the jack check) is something only an
//! external source produces (a positive reading cannot be told from "open").
//!
//! A-jack protection (doc 4.8, owner 2026-10-22): in the comparator modes a
//! hit also starts the probe (OP2 x1 on PB2, the 10 mohm element that every
//! A-jack current passes, AD2 peak hold: `probe_row`, `probe_amps`); a
//! current that would overheat the 47.5 ohm resistor (`aprot_trips`)
//! switches the A jack to the 10 mohm shunt (PE0 = 1) and latches the red
//! A-PROT alert until the user acts and the current is gone (`dmm.rs`).
//! Below that it only warns (A-JACK).

use crate::hy3131::CFG_LEN;

/// Alert enable mask bits (SYST:ALER:ENAB, Settings "Jack alerts").
pub const M_AJACK: u8 = 1;
pub const M_VJACK: u8 = 2;
pub const M_LIVE: u8 = 4;
/// The sentinel alerts ("Jack alerts" ON = all three).
pub const M_JACK: u8 = 7;
pub const M_ALL: u8 = 7;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Alert {
    None,
    /// The uA / mA shunt overloaded: the meter moved to the 10 A shunt
    /// (`dmm.rs` OVERLOAD latch, doc 4.3); red, not maskable.
    Overload,
    /// Current in the A jack while not in a current mode (red).
    AJack,
    /// A-jack protection (doc 4.8): a current that would overheat the 47.5
    /// ohm resistor switched the A jack to the 10 mohm shunt; latched in
    /// `dmm.rs`; red, not maskable.
    AProt,
    /// Voltage on V/ohm, no current, in a current mode (amber).
    VJack,
    /// A source on the circuit in ohm / continuity / diode (amber).
    Live,
    /// Hint (grey): AC V, a DC voltage on V/ohm and (almost) no AC.
    Dc,
    /// Hint (grey): DC V 6 V range, the V tap swings both ways (AC).
    Ac,
    /// Hint (grey): an A function read exactly zero for 60 s.
    Fuse,
    /// The OVERLOAD latch after its first OVL_RED_MS (display only:
    /// `Sentinel::shown`; SYST:ALER? / STAT:QUES keep OVERLOAD): the meter
    /// measures fine on 10 A, so no red any more, an amber "10A" that says
    /// the range is not the user's (owner 2026-10-22).
    Ovl10A,
}

impl Alert {
    /// SYST:ALER? name.
    pub fn name(self) -> &'static str {
        match self {
            Alert::None => "NONE",
            Alert::Overload | Alert::Ovl10A => "OVERLOAD",
            Alert::AJack => "AJACK",
            Alert::AProt => "APROT",
            Alert::VJack => "VJACK",
            Alert::Live => "LIVE",
            Alert::Dc => "DC",
            Alert::Ac => "AC",
            Alert::Fuse => "FUSE",
        }
    }
    /// Badge text (DMM status slot, combined-view strip).
    pub fn badge(self) -> &'static str {
        match self {
            Alert::None => "",
            // "OVERLOAD" does not fit the 83 px slot in TEXT22.
            Alert::Overload => "OVLD",
            Alert::AJack => "A-JACK",
            Alert::AProt => "A-PROT",
            Alert::VJack => "V-JACK?",
            Alert::Live => "LIVE?",
            Alert::Dc => "DC?",
            Alert::Ac => "AC?",
            Alert::Fuse => "FUSE?",
            Alert::Ovl10A => "10A",
        }
    }
    /// Combined-view strip badge (its pill holds 6 characters).
    pub fn strip_badge(self) -> &'static str {
        match self {
            Alert::VJack => "V-JACK",
            Alert::Live => "LIVE",
            a => a.badge(),
        }
    }
    /// Badge colour: red for A-JACK / OVERLOAD, the stock amber for the
    /// hints.
    pub fn color(self) -> u16 {
        match self {
            Alert::AJack | Alert::AProt | Alert::Overload => 0xF800,
            // The hints: the status bar's dim grey.
            Alert::Dc | Alert::Ac | Alert::Fuse => 0xAD55,
            _ => 0xFC40,
        }
    }
    /// A red (safety) alert: white on red in the status slot, alternates
    /// with HOLD (decision #5), STAT:QUES bit 9.
    pub fn red(self) -> bool {
        matches!(self, Alert::AJack | Alert::AProt | Alert::Overload)
    }
    /// Toast shown once per assertion.
    pub fn toast(self) -> &'static str {
        match self {
            Alert::None => "",
            Alert::Overload => "Overload: 10 A shunt selected",
            Alert::AJack => "Current in A jack: check leads",
            Alert::AProt => "A jack current: 10 A shunt on",
            Alert::VJack => "Voltage on V jack, no current",
            Alert::Live => "Voltage on circuit - power off",
            Alert::Dc => "DC voltage here: use DC V?",
            Alert::Ac => "AC voltage here: use AC V?",
            Alert::Fuse => "0 A for 60 s: check the fuse?",
            // (the latch's toast came with its red phase)
            Alert::Ovl10A => "",
        }
    }
}

/// STATus:QUEStionable bits of an alert (doc 4.4): bit 9 (SCPI-99: free
/// for the instrument) while a red alert (A-JACK, OVERLOAD) is on, plus bit
/// 1 (SCPI CURRent / the 34401A "current overload") for OVERLOAD.
pub fn ques_bits(a: Alert) -> u16 {
    (if a.red() { 1 << 9 } else { 0 }) | (if a == Alert::Overload { 1 << 1 } else { 0 })
}

/// STATus:QUEStionable:EVENt: the bits seen since the last read / *CLS.
pub static QUES_EVENT: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);

/// Which sentinel the current row carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Off,
    /// Comparator on PB1 (A-JACK).
    CmpA,
    /// AD1 peak hold on PB1 - PB3 (A-JACK).
    PeakA,
    /// Comparator on SENSE via the /1000 tap (V-JACK?).
    CmpV,
    /// AD1 peak hold on SENSE - AGND via the /1000 tap (V-JACK?, AC A).
    PeakV,
    /// AD2 peak hold on OP2 (x1) <- PB1, AD2 = OP2O - PB3 (A-JACK;
    /// continuity, diode, capacitance r0..4).
    Ad2A,
}

impl Kind {
    /// What the kind detects: 0 nothing, 1 A-JACK, 2 V-JACK?.
    pub fn family(self) -> u8 {
        match self {
            Kind::Off => 0,
            Kind::CmpA | Kind::PeakA | Kind::Ad2A => 1,
            Kind::CmpV | Kind::PeakV => 2,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Off => "OFF",
            Kind::CmpA => "CMP_PB1",
            Kind::PeakA => "AD1PK_PB1",
            Kind::CmpV => "CMP_SENSE",
            Kind::PeakV => "AD1PK_SENSE",
            Kind::Ad2A => "AD2PK_PB1",
        }
    }
}

/// R20 with SCMPI = PB1 (100), ENCMP.
pub const R20_PB1: u8 = 0x90;
/// R20 with SCMPI = SENSE (000), ENCMP.
pub const R20_SENSE: u8 = 0x10;
/// R2C bit 4: SS5, the /1000 tap to SENSE.
pub const R2C_SS5: u8 = 0x10;
/// AD1 sentinel input: SAD1FP = PB1 (1001), SAD1FN = PB3 (101).
pub const R24_PB1_PB3: u8 = 0x95;
/// AD1 on SENSE - PB5 (0000 / 111): the AC V rows' V-tap window (DC?).
pub const R24_SENSE_PB5: u8 = 0x07;
/// AD1 V-JACK? input (AC A): SAD1FP = SENSE (0000), SAD1FN = AGND (011).
/// Was SENSE - PB5 (40728b8 and before): PB5 is a reference only the DC V
/// 60 V+ rows define (their power bus / OP3, SMODE 0100); the A rows run the
/// bus as SGND and leave PB5 to itself, and the device's AD1 peak there ran
/// from ~-117 000 at 0.7 s to the negative rail (-262 144) by ~2 s with
/// V-COM open, 1, 5, 10 or 24 V alike: a false V-JACK? in every AC A range
/// (device-check 40728b8 2a). AGND is what the DC A comparator's thresholds
/// hang on (R21 AGNDP / AGNDN<1>).
pub const R24_SENSE_AGND: u8 = 0x03;
/// R25 AD2IG = 11 (2.0).
pub const R25_AD2IG_20: u8 = 0xC0;
/// R26: ENAD2, AD2OSR 011 (256), no chopper, AD2RG 0.
pub const R26_AD2_256: u8 = 0x83;
/// R27: SAD2IP = OP2O, SAD2IN = PB3, SAD2RH = PB6, SAD2RL = AGND.
pub const R27_OP2O_PB3: u8 = 0x6D;
/// R32 high nibble: ENOP2, SOP2P = PB1 (101).
pub const R32_OP2_PB1: u8 = 0xD0;
/// R32 high nibble: ENOP2, SOP2P = PB2 (110): the A-jack probe.
pub const R32_OP2_PB2: u8 = 0xE0;
/// R25 AD1IG = 11 (3.6).
pub const R25_AD1IG_36: u8 = 0x30;
/// R29: ENPKH, PKHSEL = 01 (AD1<23:5>).
pub const R29_PKH_AD1: u8 = 0x05;
/// R29 bit 2.
pub const R29_ENPKH: u8 = 0x04;
/// CTSTA CMPHO | CMPLO.
pub const CTSTA_CMP: u8 = 0x30;
/// CTSTA CMPLO.
pub const CTSTA_CMPLO: u8 = 0x10;

/// R33 bits 7:6: OP1CHOP (00 = CHOP_CLK held at 0).
pub const R33_OP1CHOP: u8 = 0xC0;
/// R33 bits 1:0: SAD1I, AD1's input path (00 = the R24 pair through the
/// FP / FN filter; 10 / 11 = the temperature sensor).
pub const R33_SAD1I: u8 = 0x03;

/// Device-check variants (DBG:SENTinel:VARiant, RAM only, 0 = the build's
/// choice): bit 0 keeps the row's OP1 chopper in the AC V sentinel row (the
/// f80524f row), bit 1 drops the AC V sentinel (stock AC V row), bit 2 puts
/// the comparator sentinel back on DC V 60 / 600 / 1000 V (the f80524f rows).
pub const VAR_ACV_CHOP: u8 = 1;
pub const VAR_ACV_OFF: u8 = 2;
pub const VAR_DCV_HI: u8 = 4;
/// bit 3: none of the sentinels added after f80524f's device check (AC+DC,
/// AC A, continuity, diode, capacitance r0..4, frequency: their stock rows).
pub const VAR_NEW_OFF: u8 = 8;
/// bit 4: no input alternation for the DC? / AC? hints (AC V's AD1 stays on
/// PB1 - PB3, DC V r0's comparator on PB1).
pub const VAR_HINTS_OFF: u8 = 16;
/// bit 5: AC A's V-JACK? AD1 back on SENSE - PB5 (the 40728b8 row, device
/// comparison of the 2a drift).
pub const VAR_ACA_PB5: u8 = 32;
/// bit 6: AC A's V-JACK? AD1 at IG 0.9 instead of 3.6 (device diagnostic:
/// where SENSE sits in the A rows; at IG 3.6 the hold is at the rail,
/// below -347 mV, with V-COM at 1 V or the source off: device d2aff22).
/// The swing threshold is not rescaled (4x fewer codes per volt).
pub const VAR_ACA_IG09: u8 = 64;
static VARIANT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

pub fn set_variant(v: u8) {
    VARIANT.store(v & 127, core::sync::atomic::Ordering::Relaxed);
}

pub fn variant() -> u8 {
    VARIANT.load(core::sync::atomic::Ordering::Relaxed)
}

/// The stock row of `mode` / `range` with the sentinel overrides (module
/// table) when the alert that uses it is enabled in `mask`, and the
/// sentinel kind.
///
/// Device check of f80524f (docs/device-check-2026-10-21.md):
/// * DC V 60 / 600 / 1000 V (r1..r3) carry no comparator sentinel: there
///   the comparator on PB1 tripped for as long as 10 V was on V-COM with
///   the A jack open (r0 at 1 V: never). Those rows drive the power bus
///   from OP3 (R2F SMODE 0100, PS6 + PS8 closed; r0 leaves the bus off), so
///   PB1 picks up an input-dependent level on the board. Leads in A/COM
///   with the function on DC V leave V-COM open: the autorange then sits on
///   6 V (r0), where the sentinel stays.
/// * AC V: the stock row keeps OP1CHOP = 10 (2 kHz) with AD1 off. OP1's
///   chopper clock evidently runs only with AD1 converting (the stock pairs
///   them in DC V 6 V / DC A), so turning AD1 on for the peak hold started a
///   2 kHz square of OP1's offset (x10 in 6 V) at OP1O -> PB4 -> AD2 / AD3:
///   a constant ~37 mV RMS floor for any input (AC mV reads OP2: clean).
///   The sentinel row holds the chopper off (OP1CHOP = 00); AC V is
///   AC-coupled, so OP1's static offset does not reach the reading.
pub fn sentinel_row(mode: u8, range: u8, acdc: bool, mask: u8, row: &[u8; CFG_LEN]) -> ([u8; CFG_LEN], Kind) {
    row_for(mode, range, acdc, mask, variant(), row)
}

/// `sentinel_row` with an explicit variant (host tests).
pub fn row_for(mode: u8, range: u8, acdc: bool, mask: u8, var: u8, row: &[u8; CFG_LEN]) -> ([u8; CFG_LEN], Kind) {
    let mut c = *row;
    // AC+DC: both phases carry their mode's sentinel (the DC phases' R20
    // also goes through `acdc_src`: freq_secondary rewrites it every 350 ms).
    if var & VAR_NEW_OFF != 0 && (acdc || matches!(mode, 4..=6 | 11..=14)) {
        return (c, Kind::Off);
    }
    let kind = match mode {
        1 if range > 0 && var & VAR_DCV_HI == 0 => Kind::Off,
        // AC+DC V above 6 V: its DC phase has no sentinel (DC V 60 V+), so an
        // AC-phase A-JACK would be wiped every phase switch (family change)
        // and re-asserted - a toast and beeps every cycle. Both phases quiet.
        0 if acdc && range > 0 && var & VAR_DCV_HI == 0 => Kind::Off,
        1 | 3 | 10 | 15 if mask & M_AJACK != 0 => {
            c[0] = R20_PB1;
            Kind::CmpA
        }
        0 if var & VAR_ACV_OFF != 0 => Kind::Off,
        0 | 2 if mask & M_AJACK != 0 => {
            c[2] = 0x00;
            c[3] |= 0x80;
            c[4] = R24_PB1_PB3;
            c[5] |= R25_AD1IG_36;
            c[9] = (c[9] & 0xF8) | R29_PKH_AD1;
            if mode == 0 && var & VAR_ACV_CHOP == 0 {
                c[19] &= !R33_OP1CHOP;
            }
            Kind::PeakA
        }
        7..=9 if mask & M_VJACK != 0 => {
            c[0] = R20_SENSE;
            c[12] |= R2C_SS5;
            Kind::CmpV
        }
        // Frequency (14): AD1 runs a zero reading (PB5 - PB5) nobody reads;
        // it takes PB1 - PB3 with the AC rows' peak-hold settings instead
        // (AD1 already on, OP1's chopper already off in the row). The row's
        // R33 has SAD1I = 11 (the temperature sensor), so SAD1I goes to 00:
        // the standing ~54 mV of d8e249d came with 11 (INFERRED). With 00
        // (device 2026-10-27, PR #55, IG 0.9) PB1 - PB3 is +2.5 mV with
        // V-COM open, -59 / -89 / -116 / -140 mV at 3 / 3.7 / 5 / 10 V DC on
        // V-COM (the row leaves its input divider open, R2C / R2D 0), so a
        // hit is a one-sided level (`Sentinel::pk_dc`, `pk_dc_hit`).
        14 if mask & M_AJACK != 0 => {
            c[2] = 0x00;
            c[4] = R24_PB1_PB3;
            c[5] |= R25_AD1IG_36;
            c[9] = (c[9] & 0xF8) | R29_PKH_AD1;
            c[19] &= !R33_SAD1I;
            Kind::PeakA
        }
        // AC A: AD1 (off in the row) on SENSE - PB5 with the /1000 tap
        // (SS5 only, as DC A), IG 3.6, the peak hold; OP1's chopper held off
        // as in the AC V sentinel row (AD1 on would start it: f80524f).
        4..=6 if mask & M_VJACK != 0 => {
            c[2] = 0x00;
            c[3] |= 0x80;
            c[4] = if var & VAR_ACA_PB5 != 0 { R24_SENSE_PB5 } else { R24_SENSE_AGND };
            if var & VAR_ACA_IG09 == 0 {
                c[5] |= R25_AD1IG_36;
            } else {
                c[5] &= !R25_AD1IG_36;
            }
            c[9] = (c[9] & 0xF8) | R29_PKH_AD1;
            c[12] |= R2C_SS5;
            c[19] &= !R33_OP1CHOP;
            Kind::PeakV
        }
        // Continuity, diode, capacitance r0..4: AD2 and OP2 are free. OP2
        // (x1, OPS2 = 0) on PB1, AD2 = OP2O - PB3 (refs PB6 / AGND) at IG
        // 2.0, OSR 256, the peak hold on AD2 (PKHSEL 00). AD1 (on in
        // continuity / diode) is written last (`ad1_last`, note 7).
        11 | 12 if mask & M_AJACK != 0 => ad2_sentinel(&mut c),
        13 if range <= 4 && mask & M_AJACK != 0 => ad2_sentinel(&mut c),
        _ => Kind::Off,
    };
    (c, kind)
}

/// The AD2 / OP2 A-JACK overrides (`row_for`).
fn ad2_sentinel(c: &mut [u8; CFG_LEN]) -> Kind {
    c[5] = (c[5] & 0x3D) | R25_AD2IG_20;
    c[6] = R26_AD2_256;
    c[7] = R27_OP2O_PB3;
    c[9] = (c[9] & 0xF8) | R29_ENPKH;
    c[18] = R32_OP2_PB1 | (c[18] & 0x0F);
    Kind::Ad2A
}

/// The row has AD1 and AD2 both on: write it with AD1 off and switch AD1
/// on last, so AD2 never starts while AD1 converts (datasheet 13.1 note 7;
/// `powermeter::configure_dc` does the same).
pub fn ad1_last(kind: Kind, row: &[u8; CFG_LEN]) -> bool {
    kind == Kind::Ad2A && row[3] & 0x80 != 0
}

// V_GATE (TODO "False A-JACK in ohm / temperature", devices 22c85ba /
// c2ddd2a): in the ohm and temperature rows PB1 follows a voltage on V-COM
// (5 / 10 V DC and the 0..9.6 V square tripped the comparator with the A
// jack open; 1 V did not; the A-PROT probe on PB2 read ~0.34 A per volt
// there). The comparator gives two bits per window, above VRH / below VRL,
// so a PB1 excursion from V-COM and one from an A-jack current look the
// same: no threshold or offset tells them apart (as in Hz, which has no
// sentinel). The rows' own V side can in part: ohm's AD1 code goes negative
// for a negative source (LIVE?, `dmm.rs` LIVE_CODE; a positive one above the
// open-circuit voltage reads OL on the device, 519fb8a 7b: the JACK CHECK
// below covers that), and the thermocouple input reads far beyond any
// thermocouple (T_VDRV_MV). The case A-JACK is for there - the leads in A / COM, V-COM
// open - shows neither, so the gate keeps it: A-JACK asserts only after a
// V-side result since the hit run began said V-COM is free (ohm: the next
// conversion; temperature: up to one ~2.2 s CJ / TC cycle later), and an
// asserted one drops when V-COM turns out driven (ohm: then LIVE?).

// JACK CHECK (device 519fb8a 7b / E4, 40728b8 item 4, d2aff22 S6, 2f56ff9
// new bug 1; TODO "False A-JACK in ohm"). V_GATE's ohm premise fails on the
// device: a positive source above the ohm source's open-circuit voltage
// reads OL there (5 / 10 V on every range 600 ohm .. 60 Mohm), as open leads
// do, so the reading cannot say "V-COM driven". Continuity at 5 V: A-JACK
// from the AD2 peak on PB1 (-99 100 / +124 800, ~+-0.25 V at PB1). What
// differs between the two cases in those rows is the input node itself:
// with the leads in A / COM and V-COM open it sits at the row's own source
// (ohm: the open-circuit voltage, 0.78 .. 2.17 V on the stock taps, 1.38 V
// on the 60 Mohm current-source row, device source map
// docs/device-check-2026-10-21.md "Ohm test current map"; continuity: its
// ~-0.48 V source), with a source on V-COM it follows the source. The PB1
// pickup started between 3 and 5 V in RES (519fb8a 7c: no hits up to
// 3.0 V). So when PB1 hits in those rows, AD2 reads OP1O - PB3 (OP1 is the
// row's own x1 buffer of the input, the numerator of the stock ratio) at
// IG 0.5 (+-2.5 V), and the peak pair says driven / free (`jack_driven`).
// Ohm / the resistive temperature sensors: the whole row for as long as
// the hits last (`jack_row`, through the probe's row rewrite: AD2 is off
// in those rows, AD1 is written last, note 7). Continuity: AD2 is the
// sentinel there, so its inputs are switched for one window
// (`JACK_R25_MASK`, R27_OP1O_PB3) and back (dmm.rs `cont_jack`). Driven ->
// no A-JACK, LIVE? instead; free -> A-JACK as before. INFERRED: that OP1O
// follows a source above ~2.4 V up to its swing (the device read the PB0
// node itself up to 2.47 V with AD1 in the source map, not OP1O); if OP1
// clipped lower, a source would read "free" and the A-JACK would stay as
// on the old code (the check never turns an open jack into "driven").
// Device 1a11eda showed exactly that: OP1O peaked at 2.25 V (5 V DC, 600 ohm),
// 2.14 V (5 V, 60 Mohm), 2.02-2.03 V (10 V), inside the band of the rows' own
// sources, so RES kept its false A-JACK at 5 / 10 V and the square.
//
// BOTH (device 1a11eda, f540f0d E4): in those RES runs the comparator on PB1
// read CMPHO and CMPLO in one CTSTA read (0x70) with 5 / 10 V DC on V-COM,
// 0x70 and 0 in turn with the square; 1 V and 3 V gave no hit. CTSTA never
// reads CMPHO alone on the device (hw/README "Measured node behaviour"):
// 0x70 is what any level above the window reads, and a current in A puts
// one there too (device 2026-10-26, RES AUTO, 20 mA in A, V open: LIVE?
// with the rule below in its first form, "0x70 = driven"). So in the
// jack-check comparator rows (ohm, the resistive temperature sensors;
// `v_both`): a MIXED window (0x70 and 0 reads, no one-bit read) = the
// square, driven; 0x70 reads are driven unless the input shows the row's
// own source (`own`: on the 6 / 60 Mohm rows OP1O below JACK_OWN_V and a
// reading within the range).
// Driven -> no A-JACK, LIVE? (once the run's jack check is in); DBG:SENT?
// field 14 reads 2 then. Gaps: on the rows below 6 Mohm, and in the
// resistive temperature rows, a DC
// current in A still shows LIVE?; an AC one (0x70 and 0 in turn) everywhere.

/// AD2 codes per volt at IG 0.5 (19-bit codes, PB6 / AGND = 1.25 V).
pub const JACK_CODES_PER_V: f32 = 0.5 / 1.25 * 262_144.0;
/// R27 for the jack check: SAD2IP = OP1O, SAD2IN = PB3, the sentinel's refs.
pub const R27_OP1O_PB3: u8 = R27_OP2O_PB3 & 0x3F;
/// R25 AND mask for the jack check: AD2IG = 00 (0.5).
pub const JACK_R25_MASK: u8 = 0x3D;
/// Ohm: driven above this (V): over the highest stock open-circuit voltage
/// (2.17 V, VDS15 / R1K) and below where the pickup starts (> 3.0 V).
pub const JACK_OHM_HI: f32 = 2.35;
/// Continuity: driven above this (V; its own source is ~-0.48 V, a short
/// ~0 V; any positive source is "the other sign", as LIVE? reads it).
pub const JACK_CONT_HI: f32 = 0.3;
/// Driven below this (V), both rows (ohm never goes below COM by itself;
/// continuity's source is ~-0.48 V).
pub const JACK_LO: f32 = -0.9;
/// Ohm, the 6 / 60 Mohm rows (r5 / r6): OP1O below this (V) in the jack
/// check = the input sits at the row's own source (1.62 / 1.38 V open,
/// device "Ohm test current map"), not at a source on V-COM (OP1O 2.14 V at
/// 5 V on the 60 Mohm row, 2.02-2.25 V at 5 / 10 V, device 1a11eda). The
/// line between is INFERRED. The lower rows (2.08-2.17 V open) have none.
pub const JACK_OWN_V: f32 = 1.8;
/// ... in this many jack checks in a row (one window each, ~0.1 s apart in
/// ohm): this branch's device run (~5-15 V with a resistance on V-COM, RES
/// AUTO reading 28.6 Mohm in range, OP1O 2.14 V) showed A-JACK for 80 ms
/// once with a single low verdict counting (INFERRED: a check right after
/// a range step, the node not settled).
pub const JACK_OWN_N: u8 = 3;
/// Ohm: an in-range reading counts for `own` only from this long after the
/// hit run began (a conversion that started before a source came on still
/// reads in range; the slowest ohm conversion is 400 ms).
pub const OHM_VIN_LAG_MS: u64 = 450;
/// Continuity: the jack check repeats this often while the hit run lasts
/// (its windows are the sentinel's; ohm reads it every window).
pub const JACK_RECHECK_MS: u64 = 300;

/// The jack check's verdict from its AD2 peak pair (`ohm`: the ohm row's
/// limits, else continuity's): Some(driven), None for an empty hold.
pub fn jack_driven(ohm: bool, pkmin: i32, pkmax: i32) -> Option<bool> {
    if pkmax < pkmin {
        return None;
    }
    let hi = if ohm { JACK_OHM_HI } else { JACK_CONT_HI };
    Some(pkmax as f32 > hi * JACK_CODES_PER_V || (pkmin as f32) < JACK_LO * JACK_CODES_PER_V)
}

/// The jack check's row (ohm / resistive temperature, over `row_for`'s
/// comparator row): AD2 = OP1O - PB3 at IG 0.5, OSR 256, the peak hold on
/// AD2; OP2 untouched (off). AD1 written last (`ad1_last`'s rule).
pub fn jack_row(row: &[u8; CFG_LEN]) -> [u8; CFG_LEN] {
    let mut c = *row;
    ad2_sentinel(&mut c);
    c[5] &= JACK_R25_MASK;
    c[7] = R27_OP1O_PB3;
    c[18] = row[18];
    c
}

/// AC mV (V_GATE): the result's node peak estimate |LPF| + sqrt(2) x RMS
/// above this many times the range = the V side is driven far beyond the
/// range (device 2f56ff9: the 0 / 9.5 V square, clipped by the mV input,
/// read 0.606 V RMS on 600 mV: ~2.4 x; a full-scale sine is 1.41 x). The
/// PB1 pickup there comes with the input clamp's current (emulator
/// `mvpb1`, INFERRED), i.e. only with the input beyond the clamp.
pub const MV_DRV_K: f32 = 1.5;
/// AC mV: a "free" V-side result counts for A-JACK only from this long
/// after the hit run began (the RMS / LPF results lag the input by a block;
/// a peak hold sees the onset first).
pub const MV_VGATE_LAG_MS: u16 = 250;

/// AC V (DC?) and plain AC mV (V_GATE): the row's frequency counter (the
/// secondary readout, every 350 ms) above this (kHz) = V-COM carries a
/// signal the RMS block cannot see (the AC bandwidth is about 20 kHz, -3 dB,
/// hw/README.md "DMM front end"; half of it here). That the counter sees
/// such a signal is INFERRED: the device's AC mV 600 mV with a 66 kHz
/// 0 / 9.5 V square did not say driven by it (2026-10-26; MV_DC_DRV_MV did
/// it in the emulator).
pub const V_HF_KHZ: f32 = 10.0;
/// AC mV (V_GATE): |LPF| above this (mV) = a DC level on the DC-coupled
/// node that an open V jack does not show. Device 2026-10-26 (A jack empty,
/// a 66 kHz 0 / 9.5 V square on V-COM): AC mV 600 mV read ~30 mV RMS and
/// raised A-JACK, DC mV read OL; the mV input clips the square (emulator
/// `mvclamp`): the device's AC-phase LPF read 0.509 V (AC+DC 600 mV,
/// 0.2.5), under MV_DRV_K x 600 mV. An input left
/// charged after a source (185c066, OL) counts as driven too, as AC+DC
/// mV's `mv_dc_drv` does.
pub const MV_DC_DRV_MV: f32 = 300.0;
/// DC mV: a V-side result counts for V_GATE only from this long after the
/// hit run began (a conversion started before the onset is not the input).
pub const DCMV_VGATE_LAG_MS: u16 = 250;

/// Temperature: |the thermocouple input| above this (mV) = a source on
/// V-COM, not a thermocouple (type E: 76.4 mV at 1000 C; K 54.9 mV at
/// 1372 C; the readout is OL above 1000 C K = 41.3 mV anyway).
pub const T_VDRV_MV: f32 = 80.0;

/// A-JACK on the AD1 peak: |PKH| above 0.5 mA on 47.5 ohm (23.75 mV at
/// PB1): IG 3.6 * 23.75 mV / 1.25 V * 2^23 / 2^5 = 17 932.
pub const PK_THR: i32 = 17_900;
/// AD1 peak codes per mV on PB1 - PB3 (IG 3.6; PK_THR / 23.75 mV).
pub const PK_CODES_PER_MV: f32 = 3.6 / 1250.0 * 262_144.0;
/// A one-sided level on PB1 - PB3 (`pk_level`) that only a current in A is
/// known to give: AC mV's V_GATE override (`dmm.rs` `mv_vside`), frequency's
/// A-JACK on a positive level (`pk_dc_hit`). ~180 mV, a DC current from
/// ~3.8 mA on 47.5 ohm. AC mV: twice -89 mV, the largest one-sided V-side
/// level seen with ~5-15 V on V-COM (device 2026-10-26; +97 mV in the AC V
/// rows); PR #55 read -112 mV at 10 V DC (IG 0.9), and AC mV also needs the
/// level above the node's |LPF|. Frequency (SAD1I 00, PR #55): +2.5 mV at
/// most on the positive side (V-COM open up to 10 V DC). INFERRED that no
/// V-COM source puts more there (AC sources on V-COM, and DC above 10 V,
/// not measured in Hz).
pub const PK_DC_THR: i32 = 136_000;
/// Frequency's A-JACK on a negative one-sided level (`pk_dc_hit`): ~280 mV,
/// twice -140 mV, PB1 - PB3 with 10 V DC on V-COM (PR #55: -59 / -89 /
/// -116 / -140 mV at 3 / 3.7 / 5 / 10 V; PK_DC_THR would leave a 1.29 x
/// margin). A negative DC current from ~5.9 mA.
pub const PK_DC_NEG_THR: i32 = 211_400;

/// Frequency's AD1 peak window hit: a one-sided level above PK_DC_THR
/// (positive) or PK_DC_NEG_THR (negative), 0 for a hold across zero.
pub fn pk_dc_hit(pkmin: i32, pkmax: i32) -> bool {
    pkmax >= pkmin && (pkmin > PK_DC_THR || pkmax < -PK_DC_NEG_THR)
}

/// The AD1 peak pair's one-sided level (codes): the smaller magnitude of a
/// hold that stays on one side of zero (a DC level on PB1), 0 for a hold
/// that crosses zero or is empty.
pub fn pk_level(pkmin: i32, pkmax: i32) -> i32 {
    if pkmax < pkmin {
        0
    } else if pkmin > 0 {
        pkmin
    } else if pkmax < 0 {
        -pkmax
    } else {
        0
    }
}

/// A-JACK on the AD2 peak (19-bit AD2 codes, IG 2.0, OP2 x1): 0.5 mA on
/// 47.5 ohm = 23.75 mV: 2.0 * 23.75 mV / 1.25 V * 2^18 = 9 961.
pub const PK2_THR: i32 = 9_900;
/// V-JACK? on the AD1 peak in AC A: 2 V on V/ohm through the /1000 tap =
/// 2 mV at SENSE: 3.6 * 2 mV / 1.25 V * 2^18 = 1 510 codes (doc 4.2: ~2 V).
pub const PKV_THR: i32 = 1_510;
/// AC A counts the SWING of the hold (max - min) only, from 2 V rms on V
/// (2 x 1.414 x PKV_THR = 4 270 codes peak to peak): a DC level on SENSE
/// cannot be told from the A rows' own offset there. The DC A comparator
/// (DBG:SENT? 0 with V-COM open, i.e. outside its +-20.8 mV window) and
/// the device's AD1 (2a) put that offset at tens of mV, i.e. tens of volts
/// on V, so a level threshold of 2 V is meaningless. AC on V is the hazard
/// in an AC current function (mains on the V jack); DC on V is the DC A
/// comparator's (and AC+DC / AUTO A's DC phase). A hold pinned at a rail
/// with no swing is the drift of 2a, no hit either way.
///
/// Device d2aff22: the A rows' SENSE (the /1000 tap through SS5) is no
/// defined node either; SENSE - AGND sat at the negative rail (< -347 mV)
/// from ~0.9 s with V-COM at 1 V / the source off, and with 24 V DC it
/// RAMPED to the rail over ~3 s: 9 000-12 000 codes per hold, a false
/// V-JACK? at 1.5-3.5 s in 60 mA / 600 uA. A ramp moves the hold's range
/// from one window to the next (consecutive holds barely overlap); an AC
/// signal fills the same range again in every window. So a hit needs the
/// swing in this hold AND an overlap of at least PKV_SWING_THR with the
/// previous hold's range (`pk_prev`): the signal came back over the same
/// span (a reversal), also when one side is clipped at the rail. The
/// first hold after a row has no previous one: +1 window (~107 ms).
pub const PKV_SWING_THR: i32 = 4_270;
/// Window length (ms) of the comparator samples; a window with any trip is
/// a "hit".
pub const WIN_MS: u64 = 100;
/// Switch blanking (owner 2026-10-23: AC? and A-JACK / A-PROT came on
/// falsely after function / range / coupling switches). No comparator or
/// peak sample counts for BLANK_ROW_MS after a row went out (the relays,
/// the converters' first results; matters where samples run through the
/// settle: the AC+DC phases), nor for BLANK_SETTLED_MS after the row's
/// settle ended (`rearm`; the peak hold, on since the row went out, is
/// cleared there by `dmm.rs`). Cost for a real fault present at a switch:
/// BLANK_SETTLED_MS + up to one window more before its first hit window;
/// nothing for one that starts later.
pub const BLANK_ROW_MS: u64 = 150;
/// The A rows' comparator on SENSE after leaving the ohm network (relay PA
/// back to the V/A path) or after a chip restart (`dmm.rs` `start`: the
/// Scope page / Wave view / logger back to the DMM). Device 469f354 check E
/// (397 switches): RES -> A / mA with V-COM open or at 1 V, CTSTA reads 0
/// for ~0.5-0.7 s, then 0x70 (CMPHO + CMPLO) in every window to ~4.3 s,
/// 0x50 to ~4.5 s, then 0 -> V-JACK? from 1.5-1.97 s to ~5 s; A at 5 V DC,
/// Scope page, back: 0x70 from 0.51 s, V-JACK? 1.4-6 s. 0x70 is the
/// comparator overdriven or unsettled (the device also reads 0x70 in DC V's
/// V-tap window on a steady 5 V), not a level V-JACK? can use while it
/// lasts: for OHM_EXIT_CAP_MS after the event a 0x70 read is no hit, nor is
/// a CMPLO-only read within OHM_EXIT_TAIL_MS of the event or of the last
/// 0x70 read (`sample_cmp`). A source that reads CMPHO alone keeps its
/// V-JACK? timing (~2 s); one that reads 0x70 (a voltage high enough to
/// overdrive the tap, if it does) or CMPLO alone waits for the cap: V-JACK?
/// ~8 s after the switch at worst. Outside the cap 0x70 counts as before.
/// Check E: the last 0x70 read at 4.2-4.4 s (RES -> A, 6 runs) and 5.24 s
/// (A at 5 V, Scope page, back), the last CMPLO-only one 0.1-0.2 s later.
///
/// The cap slides (device 6bc494a new bug 4: A / mA -> Scope -> DMM at 5 V,
/// 0x70 to 7.75 s, 0x50 to 8.01 s, then 0; c785807: 0x70 to 6.34 s; the
/// fixed 7 s cap let the tail's last 0.75 s count: V-JACK? for 0.5 s at
/// 8.06 s): a 0x70 read inside the window moves its end to OHM_EXIT_QUIET_MS
/// after it, at most OHM_EXIT_MAX_MS after the event. So the mask ends when
/// 0x70 has stopped for OHM_EXIT_QUIET_MS (a CMPLO-only tail OHM_EXIT_TAIL_MS
/// after the last 0x70, within the window), not at a fixed time. Why the
/// tail is longer since the fast-switch builds is not known (the emulator
/// replays it, `ohmexit`; device numbers only). Cost: a source that reads
/// 0x70 steadily (a voltage that overdrives the tap, if any does) gets its
/// V-JACK? ~13 s after the switch instead of ~8 s; CMPHO alone keeps ~2 s.
pub const OHM_EXIT_TAIL_MS: u64 = 2500;
pub const OHM_EXIT_CAP_MS: u64 = 7000;
pub const OHM_EXIT_QUIET_MS: u64 = 1000;
pub const OHM_EXIT_MAX_MS: u64 = 12000;
pub const BLANK_SETTLED_MS: u64 = 100;
/// `win_both` in the comparator's A-JACK windows (DC V / DC mV, MIXED in
/// `sample_cmp`): a read with one bit, with none, with both.
const W_ONE: u8 = 1;
const W_ZERO: u8 = 2;
const W_BOTH: u8 = 4;
/// Hit windows in a row before A-JACK is asserted (2 polls >= 100 ms apart).
pub const AJACK_WINDOWS: u8 = 2;
/// A-JACK clears this long after the last hit.
pub const AJACK_CLEAR_MS: u64 = 1000;
/// V-JACK?: voltage seen (and the current small) this long; clears 0.5 s
/// after the last hit.
pub const VJACK_ASSERT_MS: u64 = 1000;
pub const VJACK_CLEAR_MS: u64 = 500;
/// LIVE?: a live result this long; clears 1 s after the last one.
pub const LIVE_ASSERT_MS: u64 = 300;
pub const LIVE_CLEAR_MS: u64 = 1000;

// --------------------------------------------------- A-jack protection
//
// Doc 4.8. At rest (PE0 = PE1 = 0) an A-jack current runs through the
// fuse, 47.5 ohm, 0.5 ohm and the 10 mohm element to COM. The 47.5 ohm part
// (owner: a ~2512 package, assume 1 W, derate to 0.5 W) takes I^2 x 47.5:
// 5 V across A-COM 104 mA = 0.52 W, 12 V 250 mA = 3 W, 24 V 0.5 A = 12 W.
// PB1 (the resistor's top) is far outside the converter range at those
// currents; PB2 (the 10 mohm element, in series in every PE state -
// INFERRED from the shunt chain) gives I x 10 mohm: 0.25 A = 2.5 mV.

/// The probe: OP2 (x1) on PB2, AD2 = OP2O - PB3 at IG 2.0, OSR 256, the
/// peak hold on AD2 (PKHSEL 00), over a comparator row (`row_for`'s CmpA
/// rows: DC V 6 V, DC mV, ohm, temperature: OP2 and AD2 unused there). AD1
/// stays on: the caller writes it last (`ad1_last`'s rule, note 7).
pub fn probe_row(row: &[u8; CFG_LEN]) -> [u8; CFG_LEN] {
    let mut c = *row;
    ad2_sentinel(&mut c);
    c[18] = R32_OP2_PB2 | (c[18] & 0x0F);
    c
}

/// Amps from the probe's 19-bit AD2 peak pair: 2.0 x 10 mohm / 1.25 V x
/// 2^18 = 4 194 codes per A; the larger of |max|, |min| (DC either sign,
/// AC peak: >= its RMS, so conservative). An empty hold (max < min) = 0.
pub fn probe_amps(pkmin: i32, pkmax: i32) -> f32 {
    if pkmax < pkmin {
        return 0.0;
    }
    pkmax.unsigned_abs().max(pkmin.unsigned_abs()) as f32 / PROBE_CODES_PER_A
}
pub const PROBE_CODES_PER_A: f32 = 2.0 * 0.01 / 1.25 * 262_144.0;

/// Sustained limit (A): above it for APROT_HOT_MS (from the first hit
/// window, which itself lags the onset by up to one 100 ms window: ~0.5 s
/// from the onset in all) -> the 10 mohm shunt.
/// 125 mA = 0.74 W in 47.5 ohm: above the owner's 0.5 W derating, below
/// the assumed 1 W rating, and clear of 5 V across A-COM (104 mA, which the
/// owner wants as a warning only) and of 12 V (250 mA).
pub const APROT_HOT_A: f32 = 0.125;
pub const APROT_HOT_MS: u64 = 400;
/// At once (A): 300 mA = 4.3 W.
pub const APROT_NOW_A: f32 = 0.3;
/// After a row write with the probe (its entry, a range / function change
/// while it stays): the AD2 peak hold is cleared this long after it (AD2
/// restarted: its first results are not the input), and the first window
/// counts from there (ms). Device d8e249d: A-PROT latched on the first
/// window with the A jack open (the probe read 15 mA afterwards).
pub const PROBE_SETTLE_MS: u64 = 30;
/// Latched: the current counts as gone below this (on the 10 mohm path;
/// the probe's peak-hold noise floor is ~30 mA in the model) for
/// APROT_GONE_MS. Any real source across A-COM drives amps into 10 mohm.
pub const APROT_GONE_A: f32 = 0.05;
pub const APROT_GONE_MS: u64 = 1000;

/// The protection rule: `amps` from the probe, `hot_ms` since the first
/// hit window of the current run (the comparator saw the current start).
pub fn aprot_trips(amps: f32, hot_ms: u64) -> bool {
    amps >= APROT_NOW_A || (amps >= APROT_HOT_A && hot_ms >= APROT_HOT_MS)
}

// ------------------------------------------------------------- OVERLOAD
//
// Doc 4.3 / 4.7 and the manual (one 10 A ceramic fuse in front of all
// shunts: the 47.5 ohm uA resistor and the 0.5 ohm mA shunt have no fuse of
// their own). `dmm.rs` applies the rule; the pure parts are here.

/// Shunt of a current mode: 0 = A (10 mohm), 1 = mA (0.5 ohm), 2 = uA
/// (47.5 ohm chain); None outside the current modes (V / ohm / ... switch
/// the shunt MOSFETs only for the A-jack protection, doc 4.8).
pub fn shunt_of(mode: u8) -> Option<u8> {
    match mode {
        4 | 7 => Some(0),
        5 | 8 => Some(1),
        6 | 9 => Some(2),
        _ => None,
    }
}

/// Full scale (A) of a current range: unit (`shunt_of`) and range 0 / 1
/// (DM40C: 6 / 10 A, 60 / 600 mA, 600 / 6000 uA).
pub fn current_fs(unit: u8, range: u8) -> f32 {
    match (unit, range) {
        (0, 0) => 6.0,
        (0, _) => 10.0,
        (1, 0) => 0.06,
        (1, _) => 0.6,
        (_, 0) => 600e-6,
        _ => 6000e-6,
    }
}

/// The rule: on the uA / mA shunt (`unit` 2 / 1) a reading over its range
/// (`over`: |I| above 1.03 x the range, i.e. OL) moves the meter to the
/// 10 A shunt when the range is manual or the top range of that shunt
/// (r1). In auto at r0 the stock range step (r0 -> r1, the same shunt, 10x)
/// comes first; if r1 is over too the rule trips there. Never on the A
/// shunt, never outside the current modes.
pub fn overload_trips(unit: u8, range: u8, manual: bool, over: bool) -> bool {
    unit >= 1 && unit <= 2 && over && (manual || range >= 1)
}

/// The latch lets go after the current stayed below the release threshold
/// for this long (on whatever range the latch left the meter on).
pub const OVL_RELEASE_MS: u64 = 5000;

/// The OVERLOAD badge is red (with the beep and the toast) this long after
/// the trip, then amber "10A" while the latch holds (`Sentinel::shown`;
/// owner 2026-10-22: a red OVLD while the meter measures fine on 10 A).
pub const OVL_RED_MS: u64 = 3000;

/// Release threshold as a fraction of the full scale the latch returns to:
/// the reading fits that range with margin (the trip is at 1.03 x; a
/// flapping current re-trips within OVL_RETRIP_MS: sticky). Was 0.5: manual
/// 60 mA with 40 mA flowing never came back (owner 2026-10-22).
pub const OVL_RELEASE_FRAC: f32 = 0.85;

/// The 10 A range cannot tell less than this from zero (1 mA digits, a few
/// counts of noise): the release threshold is never below it.
pub const OVL_ZERO_A: f32 = 2e-3;

/// A trip this soon after a timed release makes the new latch sticky (no
/// timed release: only a user choice ends it). Covers the one case where
/// the threshold exceeds the range it returns to (manual 600 uA: 0.6..2 mA
/// would otherwise cycle trip / release every ~5 s).
pub const OVL_RETRIP_MS: u64 = 30_000;

/// Release threshold (A): OVL_RELEASE_FRAC of the full scale the meter
/// goes back to - the manual range it came from (`back`: unit, range),
/// else (auto: no latch in the DMM since the quiet hop, the power meter's
/// I AUTO neither) the top range of the barred shunt - so the return does
/// not overload again at once; at least OVL_ZERO_A.
pub fn overload_release_thr(barred: u8, back: Option<(u8, u8)>) -> f32 {
    let t = match back {
        Some((u, r)) => OVL_RELEASE_FRAC * current_fs(u, r),
        None => OVL_RELEASE_FRAC * current_fs(barred, 1),
    };
    t.max(OVL_ZERO_A)
}

/// Hints (doc 4.4, grey, silent, toast once): enabled with V-JACK? (Settings
/// "Jack alerts" ON = all; "A-JACK only" / OFF drop them).
pub const M_HINTS: u8 = M_VJACK;
/// DC?: |DC| on the V tap above this (V) and the AC reading below
/// DC_HINT_AC x |DC|, for HINT_ASSERT_MS; AC?: the V tap above +20.8 mV and
/// below -20.8 mV in one comparator window (DC V r0: +-0.21 V at the jack).
pub const DC_HINT_V: f32 = 0.5;
pub const DC_HINT_AC: f32 = 0.1;
pub const HINT_ASSERT_MS: u64 = 2000;
/// AC?: a V-tap window counts only while the DC V reading is below this
/// (V). Both comparator bits mean the tap swings below -0.21 V and above
/// +0.21 V, so the wave's mean (the DC reading) sits well inside its swing;
/// a DC reading this far from zero with both bits is the comparator, not
/// the input: the device showed AC? on 5 V DC and on the 0..9.5 V square
/// (DC 4.77 V; device-check-2026-10-21 "False A-JACK in Hz", "6. The lost
/// first V tap"), not on 1 V. Also: no count before the row's first DC
/// reading (a switch in, owner 2026-10-23).
pub const AC_HINT_DC_MAX: f32 = 2.0;
pub const HINT_CLEAR_MS: u64 = 1000;
/// FUSE?: an A function "at zero" this long: |I| below FUSE_FRAC of the
/// range (0.1 %: 60 counts of 60000; a blown fuse leaves the amplifier
/// offset and noise) in DC, below AC_FLOOR_FRAC in AC / AC+DC (the RMS floor
/// of the uA path is ~1.5 % of 600 uA, acdc doc 6b).
pub const FUSE_HINT_MS: u64 = 60_000;
pub const FUSE_FRAC: f32 = 0.001;
pub const AC_FLOOR_FRAC: f32 = 0.05;

/// A condition that asserts after being seen in every sample for
/// `assert_ms` and clears `clear_ms` after it was last seen (for the
/// hints). Before it asserted, one sample without it starts the count over
/// (owner 2026-10-23: AC? came on falsely after switches; a sample now and
/// then over 2 s - a settling front end, mains pickup on open leads - was
/// enough while a gap under `clear_ms` kept the count). A real AC / DC
/// shows in every window.
#[derive(Clone, Copy)]
struct Held {
    t0: Option<u64>,
    last: u64,
}

impl Held {
    const NEW: Held = Held { t0: None, last: 0 };
    fn feed(&mut self, now: u64, on: bool, assert_ms: u64, clear_ms: u64) {
        if on {
            if self.t0.is_none() {
                self.t0 = Some(now);
            }
            self.last = now;
        } else if self.t0.is_some_and(|t0| self.last.saturating_sub(t0) < assert_ms)
            || now.saturating_sub(self.last) >= clear_ms
        {
            self.t0 = None;
        }
    }
    fn asserted(&self, now: u64, was: bool, assert_ms: u64, clear_ms: u64) -> bool {
        match self.t0 {
            Some(_) if was => now.saturating_sub(self.last) < clear_ms,
            Some(t0) => self.last.saturating_sub(t0) >= assert_ms,
            None => false,
        }
    }
}

// ----------------------------------------------------------- fuse check
//
// Doc 4.3: a blown fuse is invisible to the meter (the A side of the fuse
// has no path into the chip); the user checks it with a lead from V/ohm to
// A in ohm mode. The shunt MOSFETs rest off in ohm (PE = 2), so the path is
// fuse + 10 mohm + 0.5 + 47.5 ohm ~ 48 ohm to COM (+ the lead).

/// The fuse check's result (SYST:FUSE?, the dialog).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FuseRes {
    Idle,
    Busy,
    /// ~48 ohm: the fuse and the shunt chain conduct.
    Ok(f32),
    /// OL: no path from V/ohm through A (the fuse, or the lead).
    Open,
    /// Another resistance (ohm): a lead elsewhere, a short V-COM, ...
    Odd(f32),
    /// No ohm reading came within the time.
    NoReading,
}

/// Accepted window for "fuse OK" (ohm): 47.5 + 0.5 + 0.06 ohm -10 % / +10 %
/// plus 1 ohm of lead and contact.
pub const FUSE_OHM: (f32, f32) = (44.0, 54.0);

pub fn fuse_verdict(ohm: f32, ol: bool) -> FuseRes {
    if ol || !ohm.is_finite() {
        FuseRes::Open
    } else if (FUSE_OHM.0..=FUSE_OHM.1).contains(&ohm) {
        FuseRes::Ok(ohm)
    } else {
        FuseRes::Odd(ohm)
    }
}

pub struct Sentinel {
    pub kind: Kind,
    pub mask: u8,
    pub alert: Alert,
    /// Assertions so far (the app toasts on a change).
    pub seq: u32,
    win_t0: u64,
    /// No sample counts before this (ms): a new row's switch (`blank`:
    /// BLANK_ROW_MS) and the first moments past its settle (`rearm`:
    /// BLANK_SETTLED_MS).
    blank_until: u64,
    /// The row left the ohm network for a V-JACK? comparator row (`dmm.rs`):
    /// until this time (ms) a CMPHO | CMPLO read extends the blank.
    pub ohm_exit_cap: u64,
    /// The exit event (ms) and the latest end the window may slide to
    /// (OHM_EXIT_MAX_MS after it; 0 = no window).
    ohm_exit_t0: u64,
    ohm_exit_max: u64,
    /// ... and a CMPLO-only read counts only from this time on.
    lo_mask_until: u64,
    win_hit: bool,
    /// ... a read with CMPHO (JCHK hits).
    win_hi: bool,
    /// CTSTA bits ORed over the window (AC?); in the DC V / DC mV A-JACK
    /// windows the kinds of reads (W_ONE / W_ZERO / W_BOTH, MIXED).
    win_both: u8,
    /// ... the changes between a read of no bit and one of both, and the
    /// last such read (MIXED).
    win_flips: u8,
    win_last: u8,
    streak: u8,
    /// Time of the last hit window (comparator / peak) and of its first one
    /// in the current run of hits.
    last_hit: u64,
    run_t0: u64,
    /// V-JACK?: current small since (None: not small).
    low_since: Option<u64>,
    /// LIVE?: first / last live reading.
    live_t0: Option<u64>,
    live_last: u64,
    /// Diagnostics (DBG:SENT?): last CTSTA, last peak pair, hit windows.
    pub ctsta: u8,
    pub pk: (i32, i32),
    pub hits: u32,
    pub samples: u32,
    /// OVERLOAD latch in force (set by `dmm.rs`, `set_overload`) and since
    /// when (`shown`).
    pub ovl: bool,
    ovl_t0: u64,
    /// A-jack protection latch in force (`dmm.rs`, `set_aprot`); the last
    /// probe value (A) and the trip latency (ms from the first hit window
    /// to the switch) for DBG:SENT?.
    pub aprot: bool,
    pub probe_a: f32,
    pub aprot_ms: u32,
    /// Hints: DC? / AC? / FUSE? conditions.
    dc: Held,
    ac: Held,
    fuse: Held,
    /// The row alternates its sentinel input every window (AC V: AD1 PB1
    /// <-> the V tap for DC?; DC V r0: the comparator PB1 <-> SENSE for
    /// AC?); `alt_v`: the window in progress watches the V tap.
    pub alt: bool,
    pub alt_v: bool,
    /// AC?: the row's own DC reading since the row went out is below
    /// AC_HINT_DC_MAX (`dmm.rs` per DC V reading); V-tap windows count only
    /// then.
    pub ac_dc_ok: bool,
    /// The last V-tap window's peak pair (DC?), for `dmm.rs`.
    pub pk_v: Option<(i32, i32)>,
    /// AC A V-JACK?: the previous hold's (min, max) (PKV_SWING_THR).
    pk_prev: Option<(i32, i32)>,
    /// Ohm / temperature (set by `dmm.rs` with the row): the comparator's
    /// A-JACK needs the row's own V-side result to say V-COM is not driven
    /// (`v_side`, V_GATE below); `vside_t` = when the last one came,
    /// `vdrv` = it said driven.
    pub vgate: bool,
    /// AC+DC / AUTO A (set by `dmm.rs` with the row): an AC-phase V-JACK?
    /// peak window without swing neither hits nor ends the run the DC
    /// phase's comparator started (a DC source on V shows no swing; before
    /// the swing rule the AC phase's level kept the run going).
    pub bridge: bool,
    /// The row's A-JACK evidence can be faked by a voltage on V-COM (set by
    /// `dmm.rs` with the row: ohm, continuity, diode, capacitance, the
    /// resistive temperature sensors). PB1 (comparator / AD2 peak) follows
    /// a driven V-COM in those rows (device 22c85ba / c2ddd2a / 519fb8a 7b
    /// / 40728b8: A-JACK in RES and continuity at 5 V, V-COM's source
    /// reading OL, so V_GATE cannot tell), and the probe on PB2 does too
    /// (~0.34 A per volt, doc 4.8: it is off there). Such an A-JACK keeps
    /// its badge, toast and log entry but is not alarm-worthy
    /// (`ajack_alarm`, the danger alarm, doc 4.10).
    pub soft: bool,
    vside_t: Option<u64>,
    vdrv: bool,
    /// V_GATE: a V-side result counts only from `vlag` ms after the run
    /// began (AC mV: MV_VGATE_LAG_MS; set by `dmm.rs` with the row).
    pub vlag: u16,
    /// The row's A-JACK needs the jack check to say V-COM is free (JACK
    /// CHECK above; set by `dmm.rs` with the row: ohm, continuity, the
    /// resistive temperature sensors); `jack_t` = when the last verdict
    /// came (0 none), `jack_drv` = it said driven, `jack_mv` = its peak (mV).
    pub jchk: bool,
    jack_t: u64,
    jack_drv: bool,
    pub jack_mv: i32,
    /// JCHK, comparator rows (ohm, the resistive temperature sensors): the
    /// last CTSTA read with CMPHO and CMPLO at once (0 none; `v_both`).
    both_t: u64,
    /// ... the last of two windows in a row of such reads mixed with empty
    /// ones and no one-bit read (a square on V-COM, `v_both`; 0 none).
    mix_t: u64,
    /// ... and at the last window's close they said driven (DBG:SENT?
    /// field 14 = 2).
    both_drv: bool,
    /// Ohm (set by `dmm.rs` per reading): the last reading was within the
    /// range, at this time (0: it read OL); `own`.
    pub vin_t: u64,
    /// The jack check's OP1O stayed below the row's line (`jack_result`:
    /// its peak max under JACK_OWN_V on the 6 / 60 Mohm rows; none elsewhere): `own`.
    jack_low: u8,
    /// Frequency (set by `dmm.rs` with the row): an AD1 peak window hits
    /// only on a one-sided level (`pk_dc_hit`).
    pub pk_dc: bool,
    /// Assertion log (DBG:SENTinel:LOG?, the switch regression test
    /// emu/hy3131/switchtest.py): the last LOG_N alerts that came on, as
    /// (alert, kind, ms); `log_n` = assertions logged so far.
    log: [(Alert, Kind, u32); LOG_N],
    pub log_n: u32,
}

/// Entries of the assertion log.
pub const LOG_N: usize = 8;

impl Sentinel {
    pub const fn new() -> Self {
        Sentinel {
            kind: Kind::Off,
            mask: M_ALL,
            alert: Alert::None,
            seq: 0,
            win_t0: 0,
            blank_until: 0,
            ohm_exit_cap: 0,
            ohm_exit_t0: 0,
            ohm_exit_max: 0,
            lo_mask_until: 0,
            win_hit: false,
            win_hi: false,
            win_both: 0,
            win_flips: 0,
            win_last: 0,
            streak: 0,
            last_hit: 0,
            run_t0: 0,
            low_since: None,
            live_t0: None,
            live_last: 0,
            ctsta: 0,
            pk: (0, 0),
            hits: 0,
            samples: 0,
            ovl: false,
            ovl_t0: 0,
            aprot: false,
            probe_a: 0.0,
            aprot_ms: 0,
            dc: Held::NEW,
            ac: Held::NEW,
            fuse: Held::NEW,
            alt: false,
            alt_v: false,
            ac_dc_ok: false,
            pk_v: None,
            pk_prev: None,
            vgate: false,
            bridge: false,
            soft: false,
            vside_t: None,
            vdrv: false,
            vlag: 0,
            jchk: false,
            jack_t: 0,
            jack_drv: false,
            jack_mv: 0,
            both_t: 0,
            mix_t: 0,
            both_drv: false,
            vin_t: 0,
            jack_low: 0,
            pk_dc: false,
            log: [(Alert::None, Kind::Off, 0); LOG_N],
            log_n: 0,
        }
    }

    /// The assertion log, oldest first (DBG:SENTinel:LOG?).
    pub fn log(&self) -> impl Iterator<Item = &(Alert, Kind, u32)> {
        let n = (self.log_n as usize).min(LOG_N);
        let i0 = self.log_n as usize + LOG_N - n;
        (0..n).map(move |k| &self.log[(i0 + k) % LOG_N])
    }

    /// A new row was written (`write_config`). Another kind or a function
    /// change (`reset`) drops the hit history and the alert; a range change
    /// of the same kind keeps it (an autoranging DC V must not restart it).
    ///
    /// Kinds of one family (the A-jack ones: comparator and AD1 peak) keep
    /// the history too: AC+DC alternates them every phase. OVERLOAD is the
    /// latch's (`set_overload`) and survives a row change (the latch steps
    /// through ranges of its own).
    pub fn configured(&mut self, kind: Kind, now: u64, reset: bool) {
        self.alt = false;
        self.alt_v = false;
        self.ac_dc_ok = false;
        self.pk_v = None;
        self.pk_prev = None;
        if kind.family() != self.kind.family() || reset {
            self.streak = 0;
            self.low_since = None;
            self.live_t0 = None;
            self.dc = Held::NEW;
            self.ac = Held::NEW;
            self.fuse = Held::NEW;
            self.vside_t = None;
            self.vdrv = false;
            self.jack_t = 0;
            self.jack_drv = false;
            self.both_t = 0;
            self.mix_t = 0;
            self.both_drv = false;
            self.vin_t = 0;
            if !matches!(self.alert, Alert::None | Alert::Overload | Alert::AProt) {
                self.alert = Alert::None;
            }
        }
        self.kind = kind;
        self.blank_until = now;
        self.win_t0 = now;
        self.win_hit = false;
        self.win_hi = false;
        self.win_both = 0;
        self.win_flips = 0;
        self.win_last = 0;
    }

    /// No sample counts for `ms` from `now` (`dmm.rs` after a row write:
    /// BLANK_ROW_MS); the first window starts after it.
    pub fn blank(&mut self, now: u64, ms: u64) {
        self.blank_until = self.blank_until.max(now + ms);
        self.win_t0 = self.blank_until;
        self.win_hit = false;
        self.win_hi = false;
        self.win_both = 0;
        self.win_flips = 0;
        self.win_last = 0;
    }

    /// The row's settle is over (`dmm.rs` `sent_rearm`): samples count from
    /// BLANK_SETTLED_MS on, and the first window is a whole one from there
    /// (it closed on the one sample taken right after the settle).
    pub fn rearm(&mut self, now: u64) {
        self.blank_until = self.blank_until.max(now + BLANK_SETTLED_MS);
        self.win_t0 = self.blank_until;
        self.win_hit = false;
        self.win_hi = false;
        self.win_both = 0;
        self.win_flips = 0;
        self.win_last = 0;
    }

    /// End a blank now (`dmm.rs` `probe_rewrite`: a row rewrite that is
    /// no switch).
    pub fn unblank(&mut self, now: u64) {
        self.blank_until = now;
        self.win_t0 = now;
    }

    /// The relay left the ohm network at `t` (ms) and the row is a V-JACK?
    /// comparator row (`dmm.rs`, every row write within OHM_EXIT_MAX_MS);
    /// None: not. A row write after the same event keeps the window's end
    /// as far as a 0x70 read slid it (`sample_cmp`).
    pub fn ohm_exit(&mut self, t: Option<u64>) {
        match t {
            Some(t) => {
                if self.ohm_exit_max == 0 || t != self.ohm_exit_t0 {
                    self.ohm_exit_t0 = t;
                    self.ohm_exit_cap = t + OHM_EXIT_CAP_MS;
                } else {
                    self.ohm_exit_cap = self.ohm_exit_cap.max(t + OHM_EXIT_CAP_MS);
                }
                self.ohm_exit_max = t + OHM_EXIT_MAX_MS;
                self.lo_mask_until = self.lo_mask_until.max(t + OHM_EXIT_TAIL_MS);
            }
            None => {
                self.ohm_exit_cap = 0;
                self.ohm_exit_max = 0;
                self.lo_mask_until = 0;
            }
        }
    }

    /// Samples are blanked now (`blank`, `rearm`).
    pub fn blanked(&self, now: u64) -> bool {
        now < self.blank_until
    }

    /// A function / range change (`dmm.rs` `set`): the run of hit windows
    /// starts over (two windows of the new row before A-JACK / V-JACK?);
    /// an alert in force keeps its own clear rule. Not for the A-jack
    /// probe's row rewrites or the AC+DC phase switches (`write_config`
    /// alone): those keep the run. The jack check's verdict was the old
    /// row's (DBG:SENT? fields 14 / 15 kept it after a switch between
    /// ohm-network rows, device c6a3ac3 note 3); a new run needs its own
    /// anyway (`decide`: `jack_t >= run_t0`).
    pub fn new_run(&mut self) {
        self.streak = 0;
        self.jack_t = 0;
        self.jack_low = 0;
        self.jack_drv = false;
        self.jack_mv = 0;
    }

    /// OVERLOAD latch on / off (`dmm.rs`; not maskable: it reports a
    /// protective range change, not a guess).
    pub fn set_overload(&mut self, now: u64, on: bool) {
        if on && !self.ovl {
            self.ovl_t0 = now;
        }
        self.ovl = on;
        self.decide(now);
    }

    /// The alert to draw (badge, strip): `alert`, except OVERLOAD after its
    /// first OVL_RED_MS, which is the amber `Alert::Ovl10A` (no new toast or
    /// beep: `seq` is unchanged).
    pub fn shown(&self, now: u64) -> Alert {
        if self.alert == Alert::Overload && now.saturating_sub(self.ovl_t0) >= OVL_RED_MS {
            Alert::Ovl10A
        } else {
            self.alert
        }
    }

    /// A-jack protection latch on / off (`dmm.rs`; not maskable).
    pub fn set_aprot(&mut self, now: u64, on: bool) {
        self.aprot = on;
        self.decide(now);
    }

    /// The comparator / peak run in progress: since when hit windows came in
    /// a row (None: the last window had no hit).
    pub fn run_since(&self) -> Option<u64> {
        (self.streak > 0).then_some(self.run_t0)
    }

    /// Alerts masked off by the user are dropped at once.
    pub fn set_mask(&mut self, mask: u8) {
        self.mask = mask & M_ALL;
        let bit = match self.alert {
            Alert::AJack => M_AJACK,
            Alert::VJack => M_VJACK,
            Alert::Live => M_LIVE,
            Alert::Dc | Alert::Ac | Alert::Fuse => M_HINTS,
            Alert::None | Alert::Overload | Alert::Ovl10A | Alert::AProt => 0,
        };
        if bit != 0 && self.mask & bit == 0 {
            self.alert = Alert::None;
        }
    }

    /// One CTSTA sample (comparator kinds), taken on every poll.
    /// True = a window closed: with `alt` the caller points the comparator
    /// at the next window's node (`alt_v`: SENSE, else PB1).
    pub fn sample_cmp(&mut self, now: u64, ctsta: u8) -> bool {
        self.ctsta = ctsta;
        // CMPHO and CMPLO in one read: the node cannot be above VRH and below
        // VRL at once, the comparator is not settled / out of its range.
        let both_now = ctsta & CTSTA_CMP == CTSTA_CMP;
        let mut bits = ctsta & CTSTA_CMP;
        if self.kind == Kind::CmpV && now < self.ohm_exit_cap {
            // Leaving ohm (device 469f354, OHM_EXIT_*): the decay reads 0x70,
            // then CMPLO alone. Neither counts; CMPHO (a source above +21 V on
            // V, or an AC one's positive half) counts at once.
            if both_now {
                // the tail goes on: the window ends OHM_EXIT_QUIET_MS after it
                self.ohm_exit_cap = self.ohm_exit_cap.max(now + OHM_EXIT_QUIET_MS).min(self.ohm_exit_max);
                self.lo_mask_until = self.lo_mask_until.max((now + OHM_EXIT_TAIL_MS).min(self.ohm_exit_cap));
                bits = 0;
            } else if bits == CTSTA_CMPLO && now < self.lo_mask_until {
                bits = 0;
            }
        }
        if self.blanked(now) {
            self.win_t0 = self.blank_until;
            return false;
        }
        self.samples = self.samples.wrapping_add(1);
        if both_now && self.jchk && self.kind == Kind::CmpA {
            // JCHK: PB1 above the window, a source on V-COM or a current in
            // A (BOTH below; `v_both` tells them apart)
            self.both_t = now.max(1);
        }
        if self.alt && self.alt_v {
            // V-tap window (AC?): both bits within the window = the tap swings
            // both ways; both in one read is the comparator, not the tap (no
            // count: the device's AC? on 5 V DC).
            if !both_now {
                self.win_hit |= ctsta & CTSTA_CMP != 0;
                self.win_both |= ctsta & CTSTA_CMP;
            }
        } else {
            self.win_hit |= bits != 0;
            self.win_hi |= bits & 0x20 != 0;
            if self.kind == Kind::CmpA {
                // (MIXED below: the window's kinds of reads, and how often a
                // read of no bit and one of both followed each other)
                let w = match bits {
                    CTSTA_CMP => W_BOTH,
                    0 => W_ZERO,
                    _ => W_ONE,
                };
                if w != W_ONE {
                    if self.win_last | w == W_BOTH | W_ZERO {
                        self.win_flips = self.win_flips.saturating_add(1);
                    }
                    self.win_last = w;
                }
                self.win_both |= w;
            }
        }
        let closed = now.saturating_sub(self.win_t0) >= WIN_MS;
        if closed {
            let wb = self.win_both;
            // MIXED (DC V 6 V / DC mV, device 2026-10-26: the 66 kHz
            // 0 / 9.5 V square on V-COM, A jack empty, read CTSTA 0x70 and 0
            // in turn on PB1; in DC V AUTO two such windows in a row logged
            // AJACK,CMP_PB1): a window of CMPHO + CMPLO reads mixed with
            // empty ones is the comparator overdriven by the V side, no hit.
            // A current in A gives one-bit reads (INFERRED, as BOTH): a DC
            // one in every read, an AC one at least while PB1 passes
            // between the window and the comparator's range; one that
            // pushes PB1 out of range all the time reads 0x70 in every read,
            // which still counts. So does the window in which such a level
            // comes on: no bit, then 0x70 to its end, one change (the device
            // reads 0x70 for any level above the window). A run of 0x70 that
            // ends within the window stays MIXED (the V side's sparse 0x70).
            let mix = wb == W_BOTH | W_ZERO && !(self.win_flips == 1 && self.win_last == W_BOTH);
            let hit = if self.kind == Kind::CmpA && !self.jchk {
                wb & W_ONE != 0 || wb & W_BOTH != 0 && !mix
            } else if self.kind == Kind::CmpA {
                // JCHK: a hit needs CMPHO (PB1 above the window: the device
                // reads 0x70 there); CMPLO alone is a source near the pickup's
                // onset (device 2026-10-26, ~4.7 V on V-COM in RES: A-JACK for
                // 5 ms before the 0x70 reads made it LIVE?)
                self.win_hi
            } else {
                self.win_hit
            };
            let both = wb == CTSTA_CMP;
            self.win_t0 = now;
            self.win_hit = false;
            self.win_hi = false;
            self.win_both = 0;
            self.win_flips = 0;
            self.win_last = 0;
            if self.alt && self.alt_v {
                self.ac.feed(now, both && self.ac_dc_ok, HINT_ASSERT_MS, HINT_CLEAR_MS);
            } else {
                self.window(now, hit);
                // MIXED in a JCHK row: the square on V-COM (device c6a3ac3:
                // 0x70 on about half of the polls, 0 on the rest)
                if self.jchk && mix {
                    self.mix_t = now;
                }
                // (a 0x70 run counts as driven once the jack check of the run
                // has said whether the input sits at the row's own source)
                let mixed = self.mix_t != 0 && now.saturating_sub(self.mix_t) < AJACK_CLEAR_MS;
                self.both_drv = self.jchk && (mixed || (self.v_both(now) && self.jack_in_run() && (self.vin_t == 0 || self.jack_low == 0)));
                if hit && self.both_drv {
                    // driven (BOTH): LIVE? instead of A-JACK
                    self.live_reading(now, true);
                }
            }
            if self.alt {
                self.alt_v = !self.alt_v;
            }
        }
        self.decide(now);
        closed
    }

    /// One peak-hold harvest (AD1 kind; every AC result, ~107 ms apart).
    /// `pkmin` / `pkmax`: the 19-bit PKHMIN / PKHMAX; an empty hold (no
    /// conversion since the restart: max < min) is no hit.
    pub fn sample_peak(&mut self, now: u64, pkmin: i32, pkmax: i32) {
        self.pk = (pkmin, pkmax);
        if self.blanked(now) || pkmax < pkmin {
            // Blanked, or an empty hold (no conversion since a restart: a
            // harvest right after `dmm.rs` cleared it) - no window: the
            // caller restarts the hold, the alternation stays put.
            return;
        }
        self.samples = self.samples.wrapping_add(1);
        let thr = match self.kind {
            Kind::PeakV => PKV_THR,
            Kind::Ad2A => PK2_THR,
            _ => PK_THR,
        };
        let hit = if self.kind == Kind::PeakV {
            let prev = core::mem::replace(&mut self.pk_prev, Some((pkmin, pkmax)));
            pkmax - pkmin > PKV_SWING_THR
                && prev.is_some_and(|(a, b)| pkmax.min(b) - pkmin.max(a) > PKV_SWING_THR)
        } else if self.pk_dc {
            pk_dc_hit(pkmin, pkmax)
        } else {
            pkmax >= pkmin && (pkmax > thr || pkmin < -thr)
        };
        if self.alt && self.alt_v {
            // V-tap window (DC?): `dmm.rs` turns the pair into volts.
            self.pk_v = (pkmax >= pkmin).then_some((pkmin, pkmax));
        } else if hit || !(self.kind == Kind::PeakV && self.bridge) {
            self.window(now, hit);
        } else if self.streak > 0 {
            // AC+DC: the DC phase's verdict holds through the AC phase (its
            // next DC phase confirms or ends it).
            self.last_hit = now;
        }
        if self.alt {
            self.alt_v = !self.alt_v;
        }
        self.decide(now);
    }

    /// A-JACK in force with evidence a driven V-COM cannot fake (`soft`
    /// rows: never; A-PROT, which needs the DC V / mV probe, has its own
    /// alarm rule in `app.rs`). The danger alarm keys on this, not on
    /// `alert == AJack`.
    pub fn ajack_alarm(&self) -> bool {
        self.alert == Alert::AJack && !self.soft
    }

    /// Re-evaluate the alert without a sample (`dmm.rs`, every poll of
    /// AC+DC A's AC phase, `bridge`): the DC phase's run asserts on time
    /// and holds through the phase (its settle and blanks have no window).
    pub fn tick(&mut self, now: u64) {
        if self.bridge && self.kind == Kind::PeakV && self.streak > 0 {
            // the DC phase's verdict holds through the AC phase
            self.last_hit = now;
        }
        self.decide(now);
    }

    /// DC? condition from the last V-tap window (`dmm.rs`).
    pub fn dc_hint(&mut self, now: u64, on: bool) {
        self.dc.feed(now, on, HINT_ASSERT_MS, HINT_CLEAR_MS);
        self.decide(now);
    }

    /// Power meter coupling hints (doc 3.16), per reading: `dc` = no AC
    /// part (AC+DC: DC? "No AC: DC mode is more precise"), `ac` = an AC part
    /// on V in DC (AC? "AC present: use AC+DC"); the DC? / AC? timing and
    /// the hints mask.
    pub fn pw_hints(&mut self, now: u64, dc: bool, ac: bool) {
        // (assert_ms 0: per reading of settled parts, the old rule - a gap under
        // HINT_CLEAR_MS keeps the count; no false-hint report against these)
        self.dc.feed(now, dc, 0, HINT_CLEAR_MS);
        self.ac.feed(now, ac, 0, HINT_CLEAR_MS);
        self.decide(now);
    }

    /// FUSE? condition: an A-function reading at zero (`on`) or not.
    pub fn fuse_reading(&mut self, now: u64, zero: bool) {
        // (assert_ms 0: FUSE? keeps its 500 ms tolerance of a non-zero reading)
        self.fuse.feed(now, zero, 0, 500);
        self.decide(now);
    }

    fn window(&mut self, now: u64, hit: bool) {
        if hit {
            if self.streak == 0 {
                self.run_t0 = now;
            }
            self.streak = self.streak.saturating_add(1);
            self.last_hit = now;
            self.hits = self.hits.wrapping_add(1);
        } else {
            self.streak = 0;
        }
    }

    /// Ohm / temperature: a result of the row's own V input, `driven` =
    /// it shows a source on V-COM (V_GATE).
    pub fn v_side(&mut self, now: u64, driven: bool) {
        self.v_side_at(now, now, driven);
    }

    /// `v_side` for evidence taken at `t` (<= now): plain AC mV's "free"
    /// needs the frequency counter too, whose last gate may be older.
    #[inline(never)]
    pub fn v_side_at(&mut self, now: u64, t: u64, driven: bool) {
        self.vside_t = Some(t.min(now));
        self.vdrv = driven;
        self.decide(now);
    }

    /// V_GATE: the row's V side said "free" since the hit run began (+
    /// `vlag`), or the row has no gate. A-JACK's assertion and (DC mV) the
    /// A-PROT trip need it.
    #[inline(always)]
    pub fn v_free(&self) -> bool {
        let t0 = self.run_t0 + self.vlag as u64;
        !self.vgate || (!self.vdrv && self.vside_t.is_some_and(|t| t >= t0))
    }

    /// Jack check (JCHK): a verdict is due (the hit run has none yet, or
    /// its last is JACK_RECHECK_MS old). Continuity's window (`dmm.rs`).
    #[inline(never)]
    pub fn jack_due(&self, now: u64) -> bool {
        self.jchk
            && (self.streak > 0 || self.alert == Alert::AJack)
            && (self.jack_t < self.run_t0.max(1) || now.saturating_sub(self.jack_t) >= JACK_RECHECK_MS)
    }

    /// Jack check result (`ohm`: the ohm rows' limits): the AD2 peak pair of
    /// OP1O - PB3. Driven also counts as a live result (LIVE? instead of
    /// A-JACK: the row's own reading cannot show it, OL).
    #[inline(never)] // .cold (link-native.x): only while PB1 hits
    pub fn jack_result(&mut self, now: u64, ohm: bool, pkmin: i32, pkmax: i32, line: f32) {
        let Some(d) = jack_driven(ohm, pkmin, pkmax) else { return };
        self.jack_low = if (pkmax as f32) < line * JACK_CODES_PER_V { self.jack_low.saturating_add(1) } else { 0 };
        let m = if pkmax.unsigned_abs() >= pkmin.unsigned_abs() { pkmax } else { pkmin };
        self.jack_mv = (m as f32 * 1000.0 / JACK_CODES_PER_V) as i32;
        self.jack_t = now.max(1);
        self.jack_drv = d;
        if d {
            self.live_reading(now, true);
        } else {
            self.decide(now);
        }
    }

    /// JCHK, comparator rows: V-COM driven by the comparator's reads within
    /// the last AJACK_CLEAR_MS (BOTH): a MIXED window, or 0x70 reads while
    /// the input does not show the row's own source (`own`). Not "since the
    /// run began": a source taken off with a current in A at once keeps the
    /// hit run going.
    #[inline(never)] // .cold (link-native.x): JCHK rows only
    fn v_both(&self, now: u64) -> bool {
        let rec = |t: u64| t != 0 && now.saturating_sub(t) < AJACK_CLEAR_MS;
        rec(self.mix_t) || (rec(self.both_t) && !self.own())
    }

    /// The jack check has a verdict of the present hit run.
    fn jack_in_run(&self) -> bool {
        self.jack_t != 0 && self.jack_t >= self.run_t0
    }

    /// JCHK, ohm: the input shows the row's own source, so 0x70 on PB1 is a
    /// current in A, not a source on V-COM: the run's jack check read OP1O
    /// below the row's line AND a reading within the range came since the
    /// run began (+ OHM_VIN_LAG_MS). Either alone is not enough: a source on
    /// V-COM read 47 Mohm in range with 0x70 on PB1 (OP1O 2.14 V; device
    /// 2026-10-26, this branch's first run), and OP1O falls with the source's
    /// volts (2.25 V at 5 V, 2.03 V at 10 V, 1a11eda; 24 V not measured, the
    /// emulator's line gives 1.41 V) while such a source reads OL.
    fn own(&self) -> bool {
        self.vin_t != 0 && self.vin_t >= self.run_t0 + OHM_VIN_LAG_MS && self.jack_low >= JACK_OWN_N && self.jack_in_run()
    }

    /// Current-mode reading: the current is below 0.5 % of the range (the
    /// V-JACK? precondition), or not.
    pub fn current_small(&mut self, now: u64, small: bool) {
        if !small {
            self.low_since = None;
        } else if self.low_since.is_none() {
            self.low_since = Some(now);
        }
        self.decide(now);
    }

    /// Ohm / continuity / diode reading: `live` = a result only an external
    /// source gives.
    pub fn live_reading(&mut self, now: u64, live: bool) {
        if live {
            if self.live_t0.is_none() {
                self.live_t0 = Some(now);
            }
            self.live_last = now;
        } else if now.saturating_sub(self.live_last) >= LIVE_CLEAR_MS {
            self.live_t0 = None;
        }
        self.decide(now);
    }

    /// The alert from the hit history (priority OVERLOAD > A-JACK > V-JACK?
    /// > LIVE?).
    fn decide(&mut self, now: u64) {
        let ajack = match self.kind {
            Kind::CmpA | Kind::PeakA | Kind::Ad2A => {
                let on = matches!(self.alert, Alert::AJack);
                if on {
                    now.saturating_sub(self.last_hit) < AJACK_CLEAR_MS
                        && !(self.vgate && self.vdrv)
                        && !(self.jchk && (self.jack_drv || self.v_both(now)))
                } else {
                    // V_GATE: a V-side result since the run began (+ vlag) said
                    // V-COM is free; JCHK: so did the jack check
                    self.streak >= AJACK_WINDOWS
                        && self.v_free()
                        && (!self.jchk || (!self.jack_drv && !self.v_both(now) && self.jack_in_run()))
                }
            }
            _ => false,
        } && self.mask & M_AJACK != 0;
        let vjack = self.kind.family() == 2 && self.mask & M_VJACK != 0 && {
            let on = matches!(self.alert, Alert::VJack);
            let low = self.low_since.map_or(false, |t| now.saturating_sub(t) >= VJACK_ASSERT_MS);
            if on {
                self.low_since.is_some() && now.saturating_sub(self.last_hit) < VJACK_CLEAR_MS
            } else {
                low && self.streak > 0 && now.saturating_sub(self.run_t0) + WIN_MS >= VJACK_ASSERT_MS
            }
        };
        let live = self.mask & M_LIVE != 0 && {
            let on = matches!(self.alert, Alert::Live);
            // (a run of live results LIVE_CLEAR_MS old asserts no more: a
            // poll between its clearing and the next reading, which drops
            // `live_t0`, re-asserted it; device 1a11eda note 3, LIVE
            // flapping every poll after the source went off)
            let fresh = now.saturating_sub(self.live_last) < LIVE_CLEAR_MS;
            match self.live_t0 {
                Some(_) if on => fresh,
                Some(t0) => fresh && self.live_last.saturating_sub(t0) >= LIVE_ASSERT_MS,
                None => false,
            }
        };
        let hints = self.mask & M_HINTS != 0;
        let dc = hints && self.dc.asserted(now, self.alert == Alert::Dc, HINT_ASSERT_MS, HINT_CLEAR_MS);
        let ac = hints && self.ac.asserted(now, self.alert == Alert::Ac, HINT_ASSERT_MS, HINT_CLEAR_MS);
        let fuse = hints && self.fuse.asserted(now, self.alert == Alert::Fuse, FUSE_HINT_MS, 500);
        let a = if self.ovl {
            Alert::Overload
        } else if self.aprot {
            Alert::AProt
        } else if ajack {
            Alert::AJack
        } else if vjack {
            Alert::VJack
        } else if live {
            Alert::Live
        } else if dc {
            Alert::Dc
        } else if ac {
            Alert::Ac
        } else if fuse {
            Alert::Fuse
        } else {
            Alert::None
        };
        if a != self.alert {
            if !matches!(a, Alert::None) {
                self.seq = self.seq.wrapping_add(1);
                self.log[self.log_n as usize % LOG_N] = (a, self.kind, now as u32);
                self.log_n = self.log_n.wrapping_add(1);
            }
            self.alert = a;
            QUES_EVENT.fetch_or(ques_bits(a), core::sync::atomic::Ordering::Relaxed);
        }
    }

    /// DBG:SENT? text: kind,alert,mask,ctsta,pkmin,pkmax,streak,hits,samples,
    /// probe mA, A-PROT ms, V side (V_GATE), soft (badge-only A-JACK), jack
    /// check (-1 none, 0 free, 1 driven by OP1O, 2 driven by the comparator's
    /// both-bit reads, BOTH), the OP1O peak (mV). The OP1O verdict and its
    /// peak are those of the present hit run (or of an A-JACK in force):
    /// -1 / 0 once the run is over, as after a range switch (a few hits at
    /// a switch between ohm-network rows left 0 / 2150 mV on an open
    /// input, device c6a3ac3 note 3).
    #[inline(never)] // .cold (link-native.x): DBG reply
    #[cfg_attr(not(feature = "dbg-sent"), allow(dead_code))]
    pub fn diag_text(&self, buf: &mut [u8]) -> usize {
        let mut n = 0;
        let mut put = |s: &[u8]| {
            let k = s.len().min(buf.len() - n);
            buf[n..n + k].copy_from_slice(&s[..k]);
            n += k;
        };
        let mut nb = [0u8; 12];
        put(self.kind.name().as_bytes());
        put(b",");
        put(self.alert.name().as_bytes());
        // ..., probe mA, the A-PROT trip latency (ms).
        // ..., the V side of ohm / temperature (V_GATE: -1 none / not
        // gated, 0 free, 1 driven).
        let pa = (self.probe_a * 1000.0) as i32;
        let vd = if self.vgate && self.vside_t.is_some() { self.vdrv as i32 } else { -1 };
        // ctsta: -1 when the kind samples no comparator (the field would be
        // the last comparator row's, stale: the 40728b8 check's "AC V reads
        // 0x70 after TEMP" was TEMP's last read).
        let ct = if matches!(self.kind, Kind::CmpA | Kind::CmpV) { self.ctsta as i32 } else { -1 };
        let run = self.jack_t != 0 && self.jack_t >= self.run_t0 && (self.streak > 0 || self.alert == Alert::AJack);
        let jd = if !self.jchk {
            -1
        } else if self.both_drv {
            2
        } else if run {
            self.jack_drv as i32
        } else {
            -1
        };
        let jmv = if self.jchk && run { self.jack_mv } else { 0 };
        for v in [self.mask as i32, ct, self.pk.0, self.pk.1, self.streak as i32, self.hits as i32, self.samples as i32, pa, self.aprot_ms as i32, vd, self.soft as i32, jd, jmv] {
            put(b",");
            put(itoa(&mut nb, v));
        }
        n
    }
}

#[cfg_attr(not(feature = "dbg-sent"), allow(dead_code))]
fn itoa(b: &mut [u8; 12], v: i32) -> &[u8] {
    let mut i = b.len();
    let neg = v < 0;
    let mut u = v.unsigned_abs();
    loop {
        i -= 1;
        b[i] = b'0' + (u % 10) as u8;
        u /= 10;
        if u == 0 {
            break;
        }
    }
    if neg {
        i -= 1;
        b[i] = b'-';
    }
    &b[i..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hy3131_tables::*;

    /// Bytes of `got` that differ from `stock`, as (index, stock, got).
    fn diff(stock: &[u8; CFG_LEN], got: &[u8; CFG_LEN]) -> Vec<(usize, u8, u8)> {
        (0..CFG_LEN).filter(|&i| stock[i] != got[i]).map(|i| (i, stock[i], got[i])).collect()
    }

    #[test]
    fn comparator_rows_change_only_r20() {
        // DC V r0..r3, DC mV, ohm (both variants: also the resistive
        // temperature sensors), thermocouple.
        let mut rows: Vec<(u8, u8, &[u8; CFG_LEN])> = vec![(1, 0, &CFG_F1[0])];
        rows.extend(CFG_F3.iter().enumerate().map(|(i, r)| (3u8, i as u8, r)));
        rows.extend(CFG_F10_P8SET.iter().enumerate().map(|(i, r)| (10u8, i as u8, r)));
        rows.extend(CFG_F10_P8CLR.iter().enumerate().map(|(i, r)| (10u8, i as u8, r)));
        rows.extend(CFG_F10_P8CLR.iter().enumerate().map(|(i, r)| (15u8, i as u8, r)));
        rows.extend(CFG_F15.iter().map(|r| (15u8, 0, r)));
        // DC V 60 V+ only with the device-check variant.
        rows.extend(CFG_F1[1..].iter().map(|r| (1u8, 0x80, r)));
        for (m, rr, r) in rows {
            let var = if rr & 0x80 != 0 { VAR_DCV_HI } else { 0 };
            let (c, k) = row_for(m, rr & 0x7F | (rr >> 7), false, M_ALL, var, r);
            assert_eq!(k, Kind::CmpA);
            assert_eq!(diff(r, &c), vec![(0, r[0], 0x90)], "mode {m}");
            // The stock R20 of these rows: SCMPI = PB4 (0xf0) or ENCMP off
            // (0xe0, 0xc0): no counter, no CMP pin either way.
            assert_eq!(r[0] & 0x0E, 0, "mode {m}");
            // R21 = 0xdd: AGNDP1 / AGNDN1 = +-20.8 mV (ohm r7 0xcf is the
            // only exception; it is not used in opendm40 autorange).
        }
        for r in CFG_F1.iter().chain(CFG_F3.iter()).chain(CFG_F15.iter()) {
            assert_eq!(r[1], 0xdd);
        }
    }

    #[test]
    fn dcv_high_ranges_have_no_comparator() {
        // f80524f: A-JACK for as long as 10 V sat on the 60 V range (open A
        // jack). DC V r1..r3 keep the stock row; r0 keeps the sentinel.
        for (i, r) in CFG_F1.iter().enumerate() {
            let (c, k) = row_for(1, i as u8, false, M_ALL, 0, r);
            if i == 0 {
                assert_eq!((k, c[0]), (Kind::CmpA, R20_PB1));
            } else {
                assert_eq!((c, k), (*r, Kind::Off), "r{i}");
                // What sets them apart: OP3 drives the power bus (SMODE 0100).
                assert_eq!(r[15] & 0x0F, 0x04);
            }
        }
        assert_eq!(CFG_F1[0][15] & 0x0F, 0);
    }

    #[test]
    fn peak_rows_change_only_the_ad1_fields() {
        for (m, rows) in [(0u8, &CFG_F0[..]), (2, &CFG_F2[..])] {
            for (i, r) in rows.iter().enumerate() {
                let (c, k) = row_for(m, i as u8, false, M_ALL, 0, r);
                assert_eq!(k, Kind::PeakA);
                let d = diff(r, &c);
                let idx: Vec<usize> = d.iter().map(|x| x.0).collect();
                // R22 OSR, R23 ENAD1, R24 inputs, R25 AD1IG, R29 PKH; AC V
                // also R33 OP1CHOP held off (the f80524f 37 mV floor).
                if m == 0 {
                    assert_eq!(idx, vec![2, 3, 4, 5, 9, 19], "mode {m}");
                    assert_eq!(c[19], r[19] & 0x3F);
                    assert_eq!(r[19] & 0xC0, 0x80, "stock: OP1CHOP 2 kHz");
                    let (c1, _) = row_for(m, i as u8, false, M_ALL, VAR_ACV_CHOP, r);
                    assert_eq!(c1[19], r[19]);
                    assert_eq!(row_for(m, i as u8, false, M_ALL, VAR_ACV_OFF, r), (*r, Kind::Off));
                } else {
                    assert_eq!(idx, vec![2, 3, 4, 5, 9], "mode {m}");
                    assert_eq!(row_for(m, i as u8, false, M_ALL, VAR_ACV_OFF | VAR_ACV_CHOP, r).0, c);
                }
                assert_eq!(r[3] & 0x80, 0, "AD1 off in the stock AC row");
                assert_eq!(c[3], r[3] | 0x80);
                assert_eq!(c[4], 0x95);
                assert_eq!(c[5] & 0xCF, r[5] & 0xCF, "AD2IG / SACM / OPS kept");
                assert_eq!(c[5] & 0x30, 0x30);
                assert_eq!(c[9] & 0xF8, r[9] & 0xF8, "RMS / LPF / LPFBW kept");
                assert_eq!(c[9] & 7, 5);
                assert_eq!(c[2], 0x00);
                // R28 refs PB6 / AGND as the row (VR = 1.25 V).
                assert_eq!(c[8], 0x31);
            }
        }
    }

    #[test]
    fn current_rows_close_ss5_only() {
        for (m, rows) in [(7u8, &CFG_F7[..]), (8, &CFG_F8_9[..]), (9, &CFG_F8_9[..])] {
            for (i, r) in rows.iter().enumerate() {
                let (c, k) = row_for(m, i as u8, false, M_ALL, 0, r);
                assert_eq!(k, Kind::CmpV);
                assert_eq!(diff(r, &c), vec![(0, r[0], 0x10), (12, r[12], r[12] | 0x10)]);
                // The A rows: power bus as SGND (R2F 0x81), PS5 never closed.
                assert_eq!(r[15], 0x81);
                assert_eq!(c[12] & 0x80, 0);
                assert_eq!(r[12], 0);
            }
        }
    }

    #[test]
    fn masks_and_ramp_rows_untouched() {
        // Capacitance r5..7 (the ramp: both op amps on PB0, AD1 and the
        // comparator busy) and r8: no free block (doc 4.2) - the stock rows.
        for r in 5..7u8 {
            let row = &CFG_F13[r as usize];
            assert_eq!(row_for(13, r, false, M_ALL, 0, row), (*row, Kind::Off), "cap r{r}");
            assert_eq!(row_for(13, r + 2, false, M_ALL, 0, row).1, Kind::Off);
        }
        assert_eq!(row_for(1, 0, false, M_VJACK | M_LIVE, 0, &CFG_F1[0]), (CFG_F1[0], Kind::Off));
        assert_eq!(row_for(0, 0, false, 0, 0, &CFG_F0[0]), (CFG_F0[0], Kind::Off));
        assert_eq!(row_for(7, 0, false, M_AJACK, 0, &CFG_F7[0]), (CFG_F7[0], Kind::Off));
        assert_eq!(row_for(4, 0, false, M_AJACK, 0, &CFG_F4[0]), (CFG_F4[0], Kind::Off));
        for (m, r) in [(11u8, &CFG_F11[0]), (12, &CFG_F12[0]), (13, &CFG_F13[0]), (14, &CFG_F14[0])] {
            assert_eq!(row_for(m, 0, false, M_VJACK | M_LIVE, 0, r), (*r, Kind::Off), "mode {m}");
        }
        // VAR_NEW_OFF: the later sentinels (and AC+DC) back to the stock rows;
        // the f80524f ones stay.
        for (m, r) in [(4u8, &CFG_F4[0]), (5, &CFG_F5_6[1]), (11, &CFG_F11[0]), (12, &CFG_F12[0]), (13, &CFG_F13[0]), (14, &CFG_F14[0])] {
            assert_eq!(row_for(m, 0, false, M_ALL, VAR_NEW_OFF, r), (*r, Kind::Off), "mode {m}");
        }
        assert_eq!(row_for(1, 0, true, M_ALL, VAR_NEW_OFF, &CFG_F1[0]), (CFG_F1[0], Kind::Off));
        assert_eq!(row_for(1, 0, false, M_ALL, VAR_NEW_OFF, &CFG_F1[0]).1, Kind::CmpA);
    }

    /// Owner 2026-10-23 (false alerts after switches): the hints assert
    /// only on a condition seen in every window for HINT_ASSERT_MS; the
    /// first window after a settle is a whole one (`rearm`); a function /
    /// range change restarts the hit run (`new_run`) but keeps an alert.
    #[test]
    fn switch_blank_rules() {
        // One 100 ms window: reads every 25 ms; `v` = what a V-tap window reads (a swing:
        // CMPHO and CMPLO in different reads; `one` = both bits in each read).
        fn win(s: &mut Sentinel, t: &mut u64, swing: bool, one: bool) {
            for i in 0..4u64 {
                *t += 25;
                let c = if !s.alt_v || !swing { 0 } else if one { CTSTA_CMP } else if i % 2 == 0 { 0x20 } else { 0x10 };
                s.sample_cmp(*t, c);
            }
        }
        // AC?: a swing in every other V-tap window (a settling node, pickup) for 4 s: never.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.alt = true;
        s.ac_dc_ok = true;
        let mut t = 0;
        for k in 0..40u64 {
            win(&mut s, &mut t, (k / 2) % 2 == 0, false);
        }
        assert_eq!(s.alert, Alert::None);
        // a real AC: a swing in every V-tap window -> AC? after HINT_ASSERT_MS
        let t0 = t;
        while s.alert != Alert::Ac {
            win(&mut s, &mut t, true, false);
            assert!(t - t0 <= HINT_ASSERT_MS + 300, "AC? late");
        }
        // once on, a missed window does not drop it (HINT_CLEAR_MS)
        win(&mut s, &mut t, false, false);
        win(&mut s, &mut t, false, false);
        assert_eq!(s.alert, Alert::Ac);
        // a swing with a 5 V DC reading / before the row's first reading, or both bits in
        // every single read (the device's AC? on 5 V DC): never
        for (dc_ok, one) in [(false, false), (true, true)] {
            let mut s = Sentinel::new();
            s.configured(Kind::CmpA, 0, true);
            s.alt = true;
            s.ac_dc_ok = dc_ok;
            let mut t = 0;
            for _ in 0..60 {
                win(&mut s, &mut t, true, one);
            }
            assert_eq!(s.alert, Alert::None, "dc_ok {dc_ok} one {one}");
        }
        // Leaving ohm into DC A (device 469f354): CMP_SENSE 0x70 for 3.5 s, 0x50 to 5 s,
        // nothing connected: no V-JACK?.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpV, 0, true);
        s.ohm_exit(Some(0));
        let mut t = 0;
        while t < 9000 {
            t += 10;
            s.current_small(t, true);
            let c = if t < 3500 { 0x70 } else if t < 5000 { 0x50 } else { 0 };
            s.sample_cmp(t, c);
            assert_eq!(s.alert, Alert::None, "t {t}");
        }
        // device 6bc494a (A -> Scope -> DMM at 5 V): 0x70 to 7.75 s, 0x50 to 8.01 s, row
        // rewrites on the way (each re-reports the same event): no V-JACK?; a 0x70 that
        // never stops counts from OHM_EXIT_MAX_MS
        for (end, steady) in [(7750u64, false), (u64::MAX, true)] {
            let mut s = Sentinel::new();
            s.configured(Kind::CmpV, 0, true);
            s.ohm_exit(Some(0));
            let mut t = 0;
            let mut vj = None;
            while t < 16000 {
                t += 10;
                if t % 2000 == 0 {
                    s.ohm_exit(Some(0));
                }
                s.current_small(t, true);
                let c = if t < end { 0x70 } else if t < 8010 { 0x50 } else { 0 };
                s.sample_cmp(t, c);
                if s.alert == Alert::VJack && vj.is_none() {
                    vj = Some(t);
                }
            }
            if steady {
                let v = vj.expect("steady 0x70: V-JACK? after the max window");
                assert!(v >= OHM_EXIT_MAX_MS && v <= OHM_EXIT_MAX_MS + VJACK_ASSERT_MS + 300, "steady 0x70: V-JACK? at {v}");
            } else {
                assert_eq!(vj, None, "the 7.75 s tail: V-JACK? at {vj:?}");
            }
        }
        // a real source on V after the exit: positive (CMPHO) or AC (CMPHO / CMPLO reads)
        // as without the exit, even through the decay's 0x70 reads; a negative DC one
        // (CMPLO alone) after the tail
        for (src, lim) in [(0x60u8, VJACK_ASSERT_MS + 200), (0, VJACK_ASSERT_MS + 200), (0x50, OHM_EXIT_TAIL_MS + VJACK_ASSERT_MS + 200)] {
            let mut s = Sentinel::new();
            s.configured(Kind::CmpV, 0, true);
            s.ohm_exit(Some(0));
            let mut t = 0;
            while s.alert != Alert::VJack {
                t += 10;
                s.current_small(t, true);
                // src 0: AC, CMPHO and CMPLO in alternate reads, with a 0x70 read now and then
                let c = if src != 0 { src } else if t % 70 == 0 { 0x70 } else if (t / 10) % 2 == 0 { 0x60 } else { 0x50 };
                s.sample_cmp(t, c);
                assert!(t <= lim, "V-JACK? late for {src:#x}: {t}");
            }
        }

        // Comparator after a settle: one CTSTA sample right after it must not close a
        // window by itself, nor anything within BLANK_SETTLED_MS of it; then two whole
        // hit windows -> A-JACK (a real fault present at the switch: settle + 300 ms).
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.blank(0, BLANK_ROW_MS);
        assert!(!s.sample_cmp(140, CTSTA_CMP), "blanked after the row write");
        s.rearm(450);
        for t in [455, 500, 545] {
            assert!(!s.sample_cmp(t, CTSTA_CMP), "blanked after the settle");
        }
        assert_eq!(s.run_since(), None);
        s.sample_cmp(560, CTSTA_CMP);
        s.sample_cmp(645, CTSTA_CMP);
        assert_eq!(s.run_since(), None, "the first window is a whole one");
        s.sample_cmp(655, CTSTA_CMP);
        assert_eq!(s.run_since(), Some(655));
        s.sample_cmp(760, CTSTA_CMP);
        assert_eq!(s.alert, Alert::AJack);
        // a range change keeps the alert, restarts the run
        s.new_run();
        assert_eq!(s.run_since(), None);
        s.sample_cmp(870, 0);
        assert_eq!(s.alert, Alert::AJack);
        // one stale hit before a switch + one after: no A-JACK
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.sample_cmp(100, CTSTA_CMP);
        s.configured(Kind::CmpA, 120, false);
        s.blank(120, BLANK_ROW_MS);
        s.new_run();
        s.rearm(570);
        s.sample_cmp(771, CTSTA_CMP);
        assert_eq!(s.alert, Alert::None);
        assert_eq!(s.log_n, 0);
        // the peak hold: a harvest inside the blank is no window, the alternation stays
        let mut s = Sentinel::new();
        s.configured(Kind::PeakA, 0, true);
        s.alt = true;
        s.blank(0, BLANK_ROW_MS);
        s.sample_peak(100, -PK_THR * 3, PK_THR * 3);
        assert!(!s.alt_v && s.run_since().is_none());
        s.sample_peak(200, -PK_THR * 3, PK_THR * 3);
        assert!(s.alt_v && s.run_since() == Some(200));
        // the assertion log
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        for k in 1..=3 {
            s.sample_cmp(100 * k, CTSTA_CMP);
        }
        assert_eq!(s.log_n, 1);
        assert_eq!(s.log().next(), Some(&(Alert::AJack, Kind::CmpA, 200)));
    }

    #[test]
    fn aprot_probe_and_rule() {
        // The probe: only AD2IG, R26, R27, R29 ENPKH and OP2 (on PB2) differ
        // from the comparator row; OP1's nibble kept.
        for (m, r, row) in [(1u8, 0u8, &CFG_F1[0]), (1, 2, &CFG_F1[2]), (3, 0, &CFG_F3[0]), (10, 3, &CFG_F10_P8CLR[3]), (15, 0, &CFG_F15[0])] {
            let (c, _) = row_for(m, r, false, M_ALL, 0, row);
            let p = probe_row(&c);
            let d: Vec<usize> = diff(&c, &p).iter().map(|x| x.0).collect();
            assert!(d.iter().all(|i| [5, 6, 7, 9, 18].contains(i)), "mode {m} r{r}: {d:?}");
            assert_eq!(p[18] >> 4, R32_OP2_PB2 >> 4);
            assert_eq!(p[18] & 0x0F, c[18] & 0x0F);
            assert_eq!(p[6], R26_AD2_256);
            assert_eq!(p[9] & 0x07, R29_ENPKH);
        }
        // 4 194 codes per A; either sign; an empty hold reads 0.
        assert!((probe_amps(-10, 1049) - 0.25).abs() < 0.001);
        assert!((probe_amps(-2097, 5) - 0.5).abs() < 0.001);
        assert_eq!(probe_amps(10, -10), 0.0);
        // 5 V across A-COM (104 mA): never; 12 V (250 mA): after 0.5 s;
        // 24 V (0.5 A): at once.
        assert!(!aprot_trips(0.104, 60_000));
        assert!(!aprot_trips(0.25, 399));
        assert!(aprot_trips(0.25, 400));
        assert!(aprot_trips(0.5, 0));
        // The alert: red, above A-JACK, below OVERLOAD, not maskable.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.set_aprot(10, true);
        assert_eq!(s.alert, Alert::AProt);
        assert!(Alert::AProt.red());
        s.set_mask(0);
        assert_eq!(s.alert, Alert::AProt);
        s.configured(Kind::Off, 20, true);
        assert_eq!(s.alert, Alert::AProt);
        s.set_overload(30, true);
        assert_eq!(s.alert, Alert::Overload);
        s.set_overload(40, false);
        s.set_aprot(50, false);
        assert_eq!(s.alert, Alert::None);
    }

    #[test]
    fn acdc_phases_carry_their_modes_sentinel() {
        // AC+DC: the DC phases as plain DC (comparator), the AC phases as
        // plain AC (AD1 peak); DC V 60 V+ still none.
        for (m, r, row, k) in [
            (1u8, 0u8, &CFG_F1[0], Kind::CmpA),
            (3, 1, &CFG_F3[1], Kind::CmpA),
            (0, 0, &CFG_F0[0], Kind::PeakA),
            (2, 0, &CFG_F2[0], Kind::PeakA),
            (7, 1, &CFG_F7[1], Kind::CmpV),
            (8, 0, &CFG_F8_9[0], Kind::CmpV),
            (4, 1, &CFG_F4[1], Kind::PeakV),
            (5, 0, &CFG_F5_6[0], Kind::PeakV),
            (1, 2, &CFG_F1[2], Kind::Off),
        ] {
            assert_eq!(row_for(m, r, true, M_ALL, 0, row), row_for(m, r, false, M_ALL, 0, row), "mode {m}");
            assert_eq!(row_for(m, r, true, M_ALL, 0, row).1, k, "mode {m}");
        }
        // Above 6 V both phases are quiet (the DC phase has none): the same
        // family in both phases, never Off <-> PeakA every switch.
        for r in 1..4u8 {
            assert_eq!(row_for(0, r, true, M_ALL, 0, &CFG_F0[r as usize]), (CFG_F0[r as usize], Kind::Off));
            assert_eq!(row_for(1, r, true, M_ALL, 0, &CFG_F1[r as usize]).1, Kind::Off);
            assert_eq!(row_for(0, r, false, M_ALL, 0, &CFG_F0[r as usize]).1, Kind::PeakA, "plain AC V keeps it");
        }
        // Every AC+DC phase pair shares one family.
        for (dc, ac, r) in [(1u8, 0u8, 0u8), (3, 2, 1), (7, 4, 1), (8, 5, 0), (9, 6, 1)] {
            let rows = |m: u8| -> &[u8; CFG_LEN] {
                match m {
                    0 => &CFG_F0[r as usize],
                    1 => &CFG_F1[r as usize],
                    2 => &CFG_F2[r as usize],
                    3 => &CFG_F3[r as usize],
                    4 => &CFG_F4[r as usize],
                    5 | 6 => &CFG_F5_6[r as usize],
                    7 => &CFG_F7[r as usize],
                    _ => &CFG_F8_9[r as usize],
                }
            };
            let (a, b) = (row_for(dc, r, true, M_ALL, 0, rows(dc)).1, row_for(ac, r, true, M_ALL, 0, rows(ac)).1);
            assert_eq!(a.family(), b.family(), "{dc}/{ac} r{r}");
        }
        assert_eq!(Kind::CmpA.family(), Kind::PeakA.family());
        assert_eq!(Kind::CmpV.family(), Kind::PeakV.family());
    }

    #[test]
    fn aca_rows_ad1_on_sense_with_ss5() {
        for (m, rows) in [(4u8, &CFG_F4[..]), (5, &CFG_F5_6[..]), (6, &CFG_F5_6[..])] {
            for (i, r) in rows.iter().enumerate() {
                let (c, k) = row_for(m, i as u8, false, M_ALL, 0, r);
                assert_eq!(k, Kind::PeakV);
                let idx: Vec<usize> = diff(r, &c).iter().map(|x| x.0).collect();
                // R22 OSR, R23 ENAD1, R24 inputs, R25 AD1IG, R29 PKH, R2C
                // SS5, R33 OP1CHOP held off.
                assert_eq!(idx, vec![2, 3, 4, 5, 9, 12, 19], "mode {m} r{i}");
                assert_eq!(r[3] & 0x80, 0, "AD1 off in the stock AC A row");
                // SENSE - AGND (was SENSE - PB5: the 2a drift; VAR_ACA_PB5 keeps it).
                assert_eq!((c[2], c[3], c[4]), (0x00, r[3] | 0x80, 0x03));
                assert_eq!(row_for(m, i as u8, false, M_ALL, VAR_ACA_PB5, r).0[4], 0x07);
                assert_eq!(c[5], r[5] | 0x30);
                assert_eq!(c[9], (r[9] & 0xF8) | 5);
                assert_eq!((r[12], c[12]), (0, 0x10), "SS5 only, never PS5");
                assert_eq!((r[19] & 0xC0, c[19]), (0x80, r[19] & 0x3F));
                // The A rows run the power bus as SGND (R2F 0x81); refs PB6 / AGND.
                assert_eq!((r[15], c[8]), (0x81, 0x31));
            }
        }
    }

    #[test]
    fn freq_row_ad1_peak_on_pb1() {
        let r = &CFG_F14[0];
        let (c, k) = row_for(14, 0, false, M_ALL, 0, r);
        assert_eq!(k, Kind::PeakA);
        let idx: Vec<usize> = diff(r, &c).iter().map(|x| x.0).collect();
        // R22 OSR 256 unchopped, R24 PB1 - PB3 (was PB5 - PB5), R25 AD1IG,
        // R29 PKH, R33 SAD1I 11 -> 00 (PR #55); AD1 already on, OP1's
        // chopper already off.
        assert_eq!(idx, vec![2, 4, 5, 9, 19]);
        assert_eq!((r[3] & 0x80, r[4], r[19] & 0xC0, r[19] & 3), (0x80, 0xDF, 0, 3));
        assert_eq!(c[19], r[19] & !R33_SAD1I);
        assert_eq!((c[2], c[4], c[5] & 0x30, c[9] & 7), (0, 0x95, 0x30, 5));
        assert_eq!(c[9] & 0xF8, r[9] & 0xF8, "RMS / LPF kept");
        assert_eq!(row_for(14, 0, false, M_VJACK, 0, r), (*r, Kind::Off));
    }

    #[test]
    fn freq_hits_on_a_one_sided_level_only() {
        // PR #55 (SAD1I 00): PB1 - PB3 +2.5 mV with V-COM open, -140 mV at
        // 10 V on V-COM; a current in A pins the hold (20 mA: 0.95 V on
        // 47.5 ohm).
        let mv = |v: f32| (v * PK_CODES_PER_MV) as i32;
        let mut s = Sentinel::new();
        s.configured(Kind::PeakA, 0, true);
        s.pk_dc = true;
        let mut t = 300;
        for lv in [2.5, -140.0, -250.0, 170.0] {
            for _ in 0..10 {
                s.sample_peak(t, mv(lv) - 1_000, mv(lv) + 1_000);
                t += 110;
            }
        }
        assert_eq!((s.alert, s.hits), (Alert::None, 0));
        assert!(pk_dc_hit(mv(190.0), mv(192.0)) && pk_dc_hit(mv(-300.0), mv(-290.0)));
        assert!(!pk_dc_hit(mv(-190.0), mv(-185.0)) && !pk_dc_hit(mv(-300.0), mv(300.0)));
        // a swing across zero (an AC current, a V-side pickup): no hit
        for _ in 0..10 {
            s.sample_peak(t, -200_000, 200_000);
            t += 110;
        }
        assert_eq!((s.alert, s.hits), (Alert::None, 0));
        for _ in 0..3 {
            s.sample_peak(t, 262_000, 262_143);
            t += 110;
        }
        assert_eq!(s.alert, Alert::AJack);
        assert_eq!((pk_level(-90_000, -85_000), pk_level(5, 3), pk_level(-1, 1)), (85_000, 0, 0));
    }

    #[test]
    fn ad2_rows_cont_diode_cap() {
        let mut rows: Vec<(u8, u8, &[u8; CFG_LEN])> = vec![(11, 0, &CFG_F11[0]), (12, 0, &CFG_F12[0])];
        rows.extend(CFG_F13[..5].iter().enumerate().map(|(i, r)| (13u8, i as u8, r)));
        for (m, i, r) in rows {
            let (c, k) = row_for(m, i, false, M_ALL, 0, r);
            assert_eq!(k, Kind::Ad2A, "mode {m} r{i}");
            let idx: Vec<usize> = diff(r, &c).iter().map(|x| x.0).collect();
            // R25 AD2IG (OPS2 x1), R26 ENAD2, R27 AD2 inputs, R29 ENPKH, R32 OP2.
            for j in &idx {
                assert!([5usize, 6, 7, 9, 18].contains(j), "mode {m} r{i}: byte {j}");
            }
            // The stock rows: AD2, OP2, RMS / LPF off; nothing else uses them.
            assert_eq!((r[6] & 0x80, r[18] & 0x80, r[9] & 0xC7, r[20] & 0x80), (0, 0, 0, 0), "mode {m} r{i}");
            assert_eq!(c[5] & 0x3F, r[5] & 0x3D, "AD1IG / SACM / OPS1 kept, OPS2 = 0");
            assert_eq!((c[5] >> 6, c[6], c[7], c[9], c[18] >> 4), (3, 0x83, 0x6D, r[9] | 4, 0xD));
            assert_eq!(c[18] & 0x0F, r[18] & 0x0F, "OP1 kept");
            // AD1 on (continuity, diode): written last; off (cap r0..4): as is.
            assert_eq!(ad1_last(k, &c), r[3] & 0x80 != 0);
        }
        assert_eq!(CFG_F13[0][3] & 0x80, 0);
        assert!(ad1_last(Kind::Ad2A, &CFG_F12[0]));
        assert!(!ad1_last(Kind::PeakA, &CFG_F12[0]));
    }

    #[test]
    fn peak_thresholds_per_kind() {
        let mut s = Sentinel::new();
        // AD2 kind: 0.5 mA on 47.5 ohm at IG 2.0 ~ 9 961 codes.
        s.configured(Kind::Ad2A, 0, true);
        s.sample_peak(100, -5_000, 9_000);
        s.sample_peak(200, -5_000, 9_000);
        assert_eq!(s.alert, Alert::None);
        s.sample_peak(300, -12_000, 0);
        s.sample_peak(400, -12_000, 0);
        assert_eq!(s.alert, Alert::AJack);
        // V-JACK? on the AD1 SENSE peak: needs the current small for 1 s.
        let mut s = Sentinel::new();
        s.configured(Kind::PeakV, 0, true);
        let mut t = 0;
        while t < 2000 {
            s.sample_peak(t, -1_000, 1_000);
            s.current_small(t, true);
            t += 107;
        }
        assert_eq!(s.alert, Alert::None);
        while t < 4000 {
            s.sample_peak(t, -3_000, 3_000);
            s.current_small(t, true);
            t += 107;
        }
        assert_eq!(s.alert, Alert::VJack);
        // 230 V and 12 mA flowing: none.
        s.current_small(t, false);
        assert_eq!(s.alert, Alert::None);
        // Device 40728b8 2a: the hold drifting to the negative rail (no
        // swing) is no hit; nor is a steady DC level of any size.
        let mut s = Sentinel::new();
        s.configured(Kind::PeakV, 0, true);
        let mut t = 0;
        let mut lvl = -117_000;
        while t < 6000 {
            s.sample_peak(t, (lvl - 1_400).max(-262_144), lvl);
            s.current_small(t, true);
            lvl = (lvl - 3_000).max(-262_144);
            t += 107;
        }
        assert_eq!((s.alert, s.hits), (Alert::None, 0));
        // AC+DC A (bridge): the DC phase's comparator run survives the AC
        // phase's swing-less windows, V-JACK? at 1 s from the run's start.
        let mut b = Sentinel::new();
        b.configured(Kind::CmpV, 0, true);
        b.bridge = true;
        let mut tb = 0;
        while tb < 600 {
            b.sample_cmp(tb, 0x70 & 0x20 | 0x10);
            b.current_small(tb, true);
            tb += 10;
        }
        b.configured(Kind::PeakV, tb, false);
        while tb < 1300 {
            b.sample_peak(tb, 18_090, 18_150);
            b.current_small(tb, true);
            tb += 107;
        }
        assert_eq!(b.alert, Alert::VJack);
        b.bridge = false;
        b.configured(Kind::PeakV, tb, true);
        b.sample_peak(tb + 107, 18_090, 18_150);
        assert_eq!(b.streak, 0);
        // Device d2aff22: 24 V DC, SENSE - AGND ramps to the rail, 9 000 -
        // 12 000 codes per hold, contiguous holds: no hit (was V-JACK? at
        // 1.5-3.5 s). Then mains clipped at the rail on one side: hits.
        let mut r = Sentinel::new();
        r.configured(Kind::PeakV, 0, true);
        let (mut tr, mut top) = (0u64, -48_000i32);
        while top > -262_144 {
            let bot = (top - 11_000).max(-262_144);
            r.sample_peak(tr, bot, top);
            r.current_small(tr, true);
            top = bot;
            tr += 107;
        }
        while tr < 6000 {
            r.sample_peak(tr, -262_144, -262_144);
            r.current_small(tr, true);
            tr += 107;
        }
        assert_eq!((r.alert, r.hits), (Alert::None, 0));
        while tr < 8000 {
            r.sample_peak(tr, -262_144, -200_000);
            r.current_small(tr, true);
            tr += 107;
        }
        assert_eq!(r.alert, Alert::VJack);
        // 2.5 V rms 50 Hz on V: +-3.5 mV at SENSE = +-2 670 codes.
        while t < 9000 {
            s.sample_peak(t, -2_670, 2_670);
            s.current_small(t, true);
            t += 107;
        }
        assert_eq!(s.alert, Alert::VJack);
    }

    #[test]
    fn ajack_asserts_after_two_windows_and_clears_after_1s() {
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        let mut t = 0;
        // Quiet for a while: no alert.
        while t < 1000 {
            s.sample_cmp(t, 0);
            t += 10;
        }
        assert_eq!(s.alert, Alert::None);
        // Current: one window is not enough, two are.
        let start = t;
        while s.alert == Alert::None {
            s.sample_cmp(t, 0x20);
            t += 10;
            assert!(t - start <= 300);
        }
        // Hits seen >= 100 ms apart (two window closes).
        assert!(t - start >= 100, "{}", t - start);
        assert_eq!(s.alert, Alert::AJack);
        assert_eq!(s.seq, 1);
        // An AC current toggling between the bits and the dead band: stays.
        for i in 0..300 {
            s.sample_cmp(t, if i % 3 == 0 { 0 } else { 0x10 });
            t += 10;
        }
        assert_eq!(s.alert, Alert::AJack);
        // Gone: clears 1 s later, not before.
        let off = t;
        while s.alert == Alert::AJack {
            s.sample_cmp(t, 0);
            t += 10;
        }
        assert!(t - off >= 1000 && t - off <= 1200, "{}", t - off);
        assert_eq!(s.seq, 1);
        // A single glitch window does not assert.
        for i in 0..100 {
            s.sample_cmp(t, if i == 50 { 0x20 } else { 0 });
            t += 10;
        }
        assert_eq!(s.alert, Alert::None);
    }

    #[test]
    fn ajack_masked_and_reset() {
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        for t in (0..500).step_by(10) {
            s.sample_cmp(t, 0x20);
        }
        assert_eq!(s.alert, Alert::AJack);
        s.set_mask(M_VJACK);
        assert_eq!(s.alert, Alert::None);
        s.set_mask(M_ALL);
        s.sample_cmp(510, 0x20);
        assert_eq!(s.alert, Alert::AJack);
        // A function change drops it; a same-kind range change keeps it.
        s.configured(Kind::CmpA, 520, false);
        assert_eq!(s.alert, Alert::AJack);
        s.configured(Kind::Off, 530, true);
        assert_eq!(s.alert, Alert::None);
    }

    #[test]
    fn peak_sentinel() {
        let mut s = Sentinel::new();
        s.configured(Kind::PeakA, 0, true);
        // Empty hold (preset) and noise: nothing.
        s.sample_peak(107, 0x3FFFF, -0x40000);
        s.sample_peak(214, -40, 35);
        s.sample_peak(321, -40, 35);
        assert_eq!(s.alert, Alert::None);
        // 1 mA AC: +-~50 k.
        s.sample_peak(428, -50_000, 50_000);
        assert_eq!(s.alert, Alert::None);
        s.sample_peak(535, -50_000, 50_000);
        assert_eq!(s.alert, Alert::AJack);
        // DC of the other sign only.
        s.sample_peak(642, -30_000, -29_000);
        assert_eq!(s.alert, Alert::AJack);
        for k in 0..12 {
            s.sample_peak(749 + k * 107, -20, 20);
        }
        assert_eq!(s.alert, Alert::None);
    }

    #[test]
    fn vjack_needs_voltage_and_no_current_for_1s() {
        let mut s = Sentinel::new();
        s.configured(Kind::CmpV, 0, true);
        let mut t = 0;
        // Voltage present, but a current flows: nothing.
        while t < 3000 {
            s.sample_cmp(t, 0x20);
            if t % 100 == 0 {
                s.current_small(t, false);
            }
            t += 10;
        }
        assert_eq!(s.alert, Alert::None);
        let t0 = t;
        while s.alert == Alert::None {
            s.sample_cmp(t, 0x20);
            if t % 100 == 0 {
                s.current_small(t, true);
            }
            t += 10;
            assert!(t - t0 < 2000);
        }
        assert!(t - t0 >= 1000);
        assert_eq!(s.alert, Alert::VJack);
        // Voltage gone: clears within ~0.5 s.
        let t1 = t;
        while s.alert == Alert::VJack {
            s.sample_cmp(t, 0);
            t += 10;
        }
        assert!(t - t1 >= 500 && t - t1 <= 700, "{}", t - t1);
        // Masked: never.
        s.set_mask(M_AJACK);
        for _ in 0..300 {
            s.sample_cmp(t, 0x20);
            s.current_small(t, true);
            t += 10;
        }
        assert_eq!(s.alert, Alert::None);
    }

    #[test]
    fn live_from_readings() {
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.live_reading(0, true);
        s.live_reading(100, true);
        assert_eq!(s.alert, Alert::None);
        s.live_reading(300, true);
        assert_eq!(s.alert, Alert::Live);
        s.live_reading(700, false);
        assert_eq!(s.alert, Alert::Live);
        s.live_reading(1400, false);
        assert_eq!(s.alert, Alert::None);
        // A-JACK wins over LIVE?.
        s.live_reading(1500, true);
        s.live_reading(1900, true);
        assert_eq!(s.alert, Alert::Live);
        for t in (1900..2300).step_by(10) {
            s.sample_cmp(t, 0x10);
        }
        assert_eq!(s.alert, Alert::AJack);
    }

    #[test]
    fn overload_rule() {
        // Only the current modes have a shunt; V / ohm / cont / diode / cap /
        // Hz / temperature never trip (doc 4.3: no MOSFET switching there).
        for m in (0..=3).chain(10..=17) {
            assert_eq!(shunt_of(m), None, "mode {m}");
        }
        assert_eq!([4, 5, 6, 7, 8, 9].map(shunt_of), [Some(0), Some(1), Some(2), Some(0), Some(1), Some(2)]);
        // A shunt: never.
        for (r, man) in [(0, false), (1, false), (0, true), (1, true)] {
            assert!(!overload_trips(0, r, man, true));
        }
        for u in [1, 2] {
            // Auto r0 over: the range step first; r1 over: trip.
            assert!(!overload_trips(u, 0, false, true));
            assert!(overload_trips(u, 1, false, true));
            // Manual: any OL.
            assert!(overload_trips(u, 0, true, true));
            assert!(overload_trips(u, 1, true, true));
            // In range: never.
            assert!(!overload_trips(u, 1, true, false));
        }
        assert!(!overload_trips(3, 1, true, true));
        // Release below 0.85 of the scale it returns to.
        let near = |a: f32, b: f32| (a - b).abs() <= 1e-6 * b;
        assert!(near(overload_release_thr(2, None), 5.1e-3));
        assert!(near(overload_release_thr(1, None), 0.51));
        assert_eq!(overload_release_thr(1, Some((2, 0))), OVL_ZERO_A);
        assert!(near(overload_release_thr(2, Some((2, 1))), 5.1e-3));
        assert!(near(overload_release_thr(2, Some((1, 1))), 0.51));
        // Manual 60 mA with 40 mA flowing comes back (owner 2026-10-22).
        assert!(0.040 < overload_release_thr(1, Some((1, 0))));
        // Below the range it returns to, except manual 600 uA (OVL_RETRIP_MS).
        for u in 0..3 {
            for r in 0..2 {
                let t = overload_release_thr(u, Some((u, r)));
                assert_eq!(t < current_fs(u, r) / 1.03, (u, r) != (2, 0), "{u} {r}");
            }
        }
    }

    #[test]
    fn overload_alert_latch() {
        let mut s = Sentinel::new();
        s.configured(Kind::CmpV, 0, true);
        s.set_overload(10, true);
        assert_eq!((s.alert, s.seq), (Alert::Overload, 1));
        // A range / shunt change of the latch (a new row, also a function
        // reset) keeps it and does not count a new assertion.
        s.configured(Kind::CmpV, 20, true);
        s.configured(Kind::Off, 30, true);
        s.sample_cmp(140, 0x20);
        assert_eq!((s.alert, s.seq), (Alert::Overload, 1));
        // Not maskable.
        s.set_mask(0);
        assert_eq!(s.alert, Alert::Overload);
        s.set_mask(M_ALL);
        // Over V-JACK?.
        s.configured(Kind::CmpV, 200, true);
        for t in (200..2000).step_by(10) {
            s.sample_cmp(t, 0x20);
            s.current_small(t, true);
        }
        assert_eq!(s.alert, Alert::Overload);
        s.set_overload(2000, false);
        assert_eq!((s.alert, s.seq), (Alert::VJack, 2));
        assert!(Alert::Overload.red() && Alert::AJack.red() && !Alert::VJack.red() && !Alert::Live.red());
    }

    #[test]
    fn ajack_gated_by_the_v_side() {
        // Ohm / temperature (V_GATE): comparator hits alone do not assert.
        let hits = |s: &mut Sentinel, t0: u64, t1: u64| {
            for t in (t0..t1).step_by(10) {
                s.sample_cmp(t, 0x20);
            }
        };
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.vgate = true;
        hits(&mut s, 0, 400);
        assert_eq!(s.alert, Alert::None);
        // V-COM driven (5 V on it): never A-JACK.
        s.v_side(400, true);
        hits(&mut s, 400, 3000);
        assert_eq!(s.alert, Alert::None);
        // V-COM free (leads in A / COM) during the run: A-JACK.
        s.v_side(3000, false);
        assert_eq!(s.alert, Alert::AJack);
        hits(&mut s, 3000, 3500);
        assert_eq!(s.alert, Alert::AJack);
        // ... and it drops when V-COM turns out driven.
        s.v_side(3500, true);
        assert_eq!(s.alert, Alert::None);
        // A V-side result from before the run does not count.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.vgate = true;
        s.v_side(0, false);
        hits(&mut s, 1000, 1500);
        assert_eq!(s.alert, Alert::None);
        s.v_side(1500, false);
        assert_eq!(s.alert, Alert::AJack);
        // Not gated (DC V): as before.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        hits(&mut s, 0, 300);
        assert_eq!(s.alert, Alert::AJack);
    }

    #[test]
    fn ajack_gated_by_the_jack_check() {
        // Ohm with 5 V on V-COM (device 519fb8a 7b): the reading is OL, V_GATE says free; the
        // jack check (OP1O at the comparator's run) says driven -> LIVE?, never A-JACK.
        let v = |x: f32| (x * JACK_CODES_PER_V) as i32;
        let hits = |s: &mut Sentinel, t0: u64, t1: u64| {
            for t in (t0..t1).step_by(10) {
                s.sample_cmp(t, 0x20);
            }
        };
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.vgate = true;
        s.jchk = true;
        hits(&mut s, 0, 150);
        assert!(s.jack_due(150));
        s.v_side(150, false);
        hits(&mut s, 150, 400);
        assert_eq!(s.alert, Alert::None, "no verdict yet");
        for t in (400..1500).step_by(100) {
            s.jack_result(t, true, v(2.45), v(2.5), f32::NEG_INFINITY);
            s.v_side(t, false);
            hits(&mut s, t, t + 100);
            assert_ne!(s.alert, Alert::AJack);
        }
        assert_eq!(s.alert, Alert::Live);
        assert_eq!(s.jack_mv, 2500);
        // Leads in A / COM, V-COM open (the open-circuit 2.17 V): A-JACK.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.vgate = true;
        s.jchk = true;
        hits(&mut s, 0, 300);
        s.v_side(300, false);
        assert_eq!(s.alert, Alert::None);
        s.jack_result(310, true, v(2.16), v(2.18), f32::NEG_INFINITY);
        assert_eq!(s.alert, Alert::AJack);
        // ... and drops when the jack turns driven
        hits(&mut s, 310, 600);
        s.jack_result(600, true, v(-0.2), v(4.0), f32::NEG_INFINITY);
        assert_ne!(s.alert, Alert::AJack);
        // Continuity: its -0.48 V source is free, a positive source driven.
        assert_eq!(jack_driven(false, v(-0.49), v(-0.47)), Some(false));
        assert_eq!(jack_driven(false, v(0.9), v(1.1)), Some(true));
        assert_eq!(jack_driven(true, v(0.77), v(0.79)), Some(false));
        assert_eq!(jack_driven(true, 10, 0), None);
        // A verdict from before the run does not count; one is due again.
        let mut s = Sentinel::new();
        s.configured(Kind::Ad2A, 0, true);
        s.jchk = true;
        s.jack_result(100, false, v(-0.49), v(-0.47), f32::NEG_INFINITY);
        s.sample_peak(1000, 0, 20_000);
        assert!(s.jack_due(1010));
        s.sample_peak(1100, 0, 20_000);
        s.sample_peak(1200, 0, 20_000);
        assert_eq!(s.alert, Alert::None);
        s.jack_result(1250, false, v(-0.49), v(-0.47), f32::NEG_INFINITY);
        assert_eq!(s.alert, Alert::AJack);
        assert!(!s.jack_due(1300));
        assert!(s.jack_due(1550));
    }

    #[test]
    fn mixed_overdrive_reads_are_no_hit() {
        // DC V / DC mV (MIXED): 0x70 and 0 in turn = the V side overdriving the comparator, no
        // hit; 0x70 in every read or a one-bit read = a current in A.
        let run = |reads: &[u8]| {
            let mut s = Sentinel::new();
            s.configured(Kind::CmpA, 0, true);
            for (i, t) in (0..1000).step_by(5).enumerate() {
                s.sample_cmp(t, reads[i % reads.len()]);
            }
            (s.alert, s.hits)
        };
        assert_eq!(run(&[0x70, 0x00]), (Alert::None, 0));
        assert_eq!(run(&[0x70, 0x00, 0x00, 0x00]), (Alert::None, 0));
        assert_eq!(run(&[0x70]).0, Alert::AJack);
        assert_eq!(run(&[0x20]).0, Alert::AJack);
        assert_eq!(run(&[0x70, 0x20, 0x00]).0, Alert::AJack);
        assert_eq!(run(&[0x10, 0x00, 0x20, 0x00]).0, Alert::AJack);
    }

    #[test]
    fn onset_window_of_a_level_is_a_hit() {
        // A DC current coming on in DC V: the window of its onset reads 0, then 0x70 (one
        // change, the device's 0x70 above the window): a hit, as the whole-0x70 windows after
        // it; the V side's 0x70 / 0 in turn (MIXED) is not.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        for t in (0..100).step_by(10) {
            s.sample_cmp(t, if t < 50 { 0 } else { 0x70 });
        }
        for t in (100..250).step_by(10) {
            s.sample_cmp(t, 0x70);
        }
        assert_eq!((s.alert, s.hits), (Alert::AJack, 2));
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        for t in (0..1000).step_by(10) {
            s.sample_cmp(t, if (t / 10) % 2 == 0 { 0 } else { 0x70 });
        }
        assert_eq!((s.alert, s.hits), (Alert::None, 0));
        // JCHK (ohm): CMPLO-only reads (0x10 / 0x50) are no hit
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.jchk = true;
        for t in (0..1000).step_by(10) {
            s.sample_cmp(t, if (t / 10) % 2 == 0 { 0x10 } else { 0x50 });
        }
        assert_eq!((s.alert, s.hits), (Alert::None, 0));
        // sparse 0x70 runs of the V side that end within a window (0x70, then 0): no hit
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        for t in (0..1000).step_by(10) {
            s.sample_cmp(t, if t % 100 < 40 { 0x70 } else { 0 });
        }
        assert_eq!((s.alert, s.hits), (Alert::None, 0));
    }

    #[test]
    fn ajack_gated_by_both_bit_reads() {
        // Device 1a11eda (RES, A jack empty, 5 / 10 V DC or the square on V-COM): CTSTA 0x70
        // in the hit windows and OP1O at 2.0-2.25 V, the jack check's "free" band: BOTH says
        // driven -> LIVE?, never A-JACK.
        let v = |x: f32| (x * JACK_CODES_PER_V) as i32;
        let run = |s: &mut Sentinel, t0: u64, t1: u64, ct: &dyn Fn(u64) -> u8| {
            for t in (t0..t1).step_by(10) {
                s.sample_cmp(t, ct(t));
                if t % 100 == 0 {
                    s.jack_result(t, true, v(2.13), v(2.25), f32::NEG_INFINITY);
                    s.v_side(t, false);
                }
            }
        };
        for sq in [false, true] {
            let mut s = Sentinel::new();
            s.configured(Kind::CmpA, 0, true);
            s.vgate = true;
            s.jchk = true;
            // the square: 0x70 in its high half only (40 Hz: 12.5 ms), 0 in the low one
            let ct = move |t: u64| if !sq || (t / 10) % 3 != 0 { 0x70 } else { 0 };
            let mut seen = false;
            let mut t = 0;
            while t < 3000 {
                run(&mut s, t, t + 100, &ct);
                assert_ne!(s.alert, Alert::AJack, "t {} sq {}", t, sq);
                seen |= s.alert == Alert::Live;
                t += 100;
            }
            assert!(seen && s.alert == Alert::Live, "sq {}", sq);
            let mut b = [0u8; 120];
            let n = s.diag_text(&mut b);
            let d = core::str::from_utf8(&b[..n]).unwrap();
            assert!(d.ends_with(",2,2249") || d.ends_with(",2,2250"), "{}", d);
        }
        // A current in A, V-COM open (device 2026-10-26, RES AUTO, 20 mA: LIVE? before the
        // fix, the reading 31 Mohm): PB1 above the window reads 0x70 in every read, as a source
        // on V-COM does. The input at the row's own source tells them apart: OP1O below the
        // 6 / 60 Mohm rows' line (1.38 V open on the 60 Mohm row) and a reading within the range.
        // Cases: (line, OP1O, in range) -> A-JACK; no line (600 ohm .. 600 kohm), OL (a source
        // on V-COM whose OP1O fell low, 24 V), OP1O high (a source read in range): LIVE?.
        for (line, op1o, in_range, aj) in [(JACK_OWN_V, 1.38, true, true), (f32::NEG_INFINITY, 1.38, true, false),
                                           (JACK_OWN_V, 1.41, false, false), (JACK_OWN_V, 2.14, true, false)] {
            let mut s = Sentinel::new();
            s.configured(Kind::CmpA, 0, true);
            s.vgate = true;
            s.jchk = true;
            for t in (0..1500).step_by(10) {
                s.sample_cmp(t, 0x70);
                if t % 100 == 50 {
                    s.jack_result(t, true, v(op1o - 0.01), v(op1o + 0.01), line);
                    s.v_side(t, false);
                }
                if t % 200 == 0 {
                    s.vin_t = if in_range { t.max(1) } else { 0 };
                }
                assert_ne!(s.alert, if aj { Alert::Live } else { Alert::AJack }, "{} {} {} t {}", line, op1o, in_range, t);
            }
            assert_eq!(s.alert, if aj { Alert::AJack } else { Alert::Live }, "{} {} {}", line, op1o, in_range);
            // ... and a source on V-COM coming on drops it (LIVE?): OL, OP1O at 2.14 V
            s.vin_t = 0;
            for t in (1500..2100).step_by(10) {
                s.sample_cmp(t, 0x70);
                if t % 100 == 50 {
                    s.jack_result(t, true, v(2.13), v(2.15), line);
                }
            }
            assert_eq!(s.alert, Alert::Live);
        }
        // The source taken off and a current in A at once (the hit run goes on): A-JACK once
        // the jack check sees the row's own source again.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.vgate = true;
        s.jchk = true;
        run(&mut s, 0, 1000, &|_| 0x70);
        assert_eq!(s.alert, Alert::Live);
        for t in (1000..2600).step_by(10) {
            s.sample_cmp(t, 0x70);
            if t % 100 == 0 {
                s.jack_result(t, true, v(1.37), v(1.39), JACK_OWN_V);
            }
            if t % 200 == 0 {
                s.vin_t = t;
            }
        }
        assert_eq!(s.alert, Alert::AJack);
        // Not in the rows without the jack check (DC V: A-JACK on any hit).
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        for t in (0..300).step_by(10) {
            s.sample_cmp(t, 0x70);
        }
        assert_eq!(s.alert, Alert::AJack);
    }

    #[test]
    fn live_does_not_flap_after_the_source() {
        // Device 1a11eda note 3: RES 600 ohm, the 5 V switched off: LIVE? came on again every
        // poll (5-10 ms) once it had cleared, until the next reading. Polls here every 5 ms
        // (the comparator samples), the readings ~400 ms apart.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        for t in (0..600).step_by(100) {
            s.live_reading(t, true);
        }
        assert_eq!(s.alert, Alert::Live);
        let n0 = s.log_n;
        let mut changes = 0;
        let mut was = s.alert;
        for t in (600..3000).step_by(5) {
            s.sample_cmp(t, 0);
            if t % 400 == 0 {
                s.live_reading(t, false);
            }
            if s.alert != was {
                changes += 1;
                was = s.alert;
            }
        }
        assert_eq!(s.alert, Alert::None);
        assert_eq!(changes, 1, "LIVE? -> NONE once");
        assert_eq!(s.log_n, n0, "no new log entries");
    }

    #[test]
    fn acmv_vside_lag() {
        // AC mV (V_GATE with vlag): a "free" result within the lag of the run's start does not
        // count (the RMS block lags the peak hold).
        let mut s = Sentinel::new();
        s.configured(Kind::PeakA, 0, true);
        s.vgate = true;
        s.vlag = MV_VGATE_LAG_MS;
        for t in (1000..1200).step_by(100) {
            s.sample_peak(t, 0, 20_000);
            s.v_side(t, false);
        }
        assert_eq!(s.alert, Alert::None);
        s.sample_peak(1300, 0, 20_000);
        s.v_side(1300, false);
        assert_eq!(s.alert, Alert::AJack);
    }

    #[test]
    fn v_free_driven_blocks_ajack() {
        // DC mV / AC mV (V_GATE): driven results block A-JACK and v_free (the A-PROT trip);
        // a free one inside the lag does not count, one after it does.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.vgate = true;
        s.vlag = DCMV_VGATE_LAG_MS;
        for t in (1000..1600).step_by(10) {
            s.sample_cmp(t, 0x20);
            if t % 100 == 0 {
                s.v_side(t, true);
            }
        }
        assert_eq!(s.alert, Alert::None);
        assert!(!s.v_free());
        // the source goes, the hits stay (a current in A): free after the lag -> A-JACK
        for t in (1600..1800).step_by(10) {
            s.sample_cmp(t, 0x20);
        }
        s.v_side(1800, false);
        assert!(s.v_free());
        assert_eq!(s.alert, Alert::AJack);
        // no gate (DC V): v_free always
        let mut d = Sentinel::new();
        d.configured(Kind::CmpA, 0, true);
        assert!(d.v_free());
    }

    #[test]
    fn overload_badge_red_then_amber() {
        let mut s = Sentinel::new();
        s.configured(Kind::CmpV, 0, true);
        s.set_overload(1000, true);
        assert_eq!((s.alert, s.seq), (Alert::Overload, 1));
        // Red for OVL_RED_MS (the toast and beep come with seq 1) ...
        assert_eq!(s.shown(1000), Alert::Overload);
        assert_eq!(s.shown(1000 + OVL_RED_MS - 1), Alert::Overload);
        // ... then amber "10A": not red, no new toast (seq), SCPI unchanged.
        let a = s.shown(1000 + OVL_RED_MS);
        assert_eq!(a, Alert::Ovl10A);
        assert!(!a.red() && a.badge() == "10A" && a.name() == "OVERLOAD" && a.toast().is_empty());
        assert_eq!((s.alert, s.seq), (Alert::Overload, 1));
        assert_eq!(ques_bits(s.alert), 1 << 9 | 1 << 1);
        // A row change of the latch (10 A range steps) keeps the time.
        s.configured(Kind::Off, 5000, true);
        s.set_overload(5000, true);
        assert_eq!(s.shown(5000), Alert::Ovl10A);
        // Released, and a new trip: red again.
        s.set_overload(6000, false);
        assert_eq!(s.shown(6000), Alert::None);
        s.set_overload(7000, true);
        assert_eq!(s.shown(7000), Alert::Overload);
        assert_eq!(s.seq, 2);
        // Other alerts are drawn as they are.
        let mut v = Sentinel::new();
        v.configured(Kind::CmpA, 0, true);
        for t in (0..300).step_by(10) {
            v.sample_cmp(t, 0x20);
        }
        assert_eq!(v.shown(100_000), Alert::AJack);
    }

    #[test]
    fn family_keeps_history_across_ajack_kinds() {
        // AC+DC alternates the comparator (DC phase) and the AD1 peak (AC
        // phase) sentinels: an asserted A-JACK survives the phase switch.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        for t in (0..300).step_by(10) {
            s.sample_cmp(t, 0x20);
        }
        assert_eq!(s.alert, Alert::AJack);
        s.configured(Kind::PeakA, 300, false);
        assert_eq!(s.alert, Alert::AJack);
        s.sample_peak(400, -50_000, 50_000);
        assert_eq!(s.alert, Alert::AJack);
        // Another family: dropped.
        s.configured(Kind::CmpV, 500, false);
        assert_eq!(s.alert, Alert::None);
    }

    #[test]
    fn alternating_comparator_ajack_and_ac_hint() {
        // DC V r0 with the AC? alternation: PB1 and SENSE windows take turns.
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.alt = true;
        s.ac_dc_ok = true; // (the DC V reading ~0)
        let mut t = 0;
        // Nothing anywhere: quiet.
        while t < 3000 {
            s.sample_cmp(t, 0);
            t += 10;
        }
        assert_eq!(s.alert, Alert::None);
        // AC on the V tap (both bits within SENSE windows), A jack quiet:
        // AC? after ~2 s, never A-JACK.
        let t0 = t;
        while s.alert == Alert::None {
            let v = s.alt_v;
            s.sample_cmp(t, if v { if (t / 10) % 2 == 0 { 0x20 } else { 0x10 } } else { 0 });
            t += 10;
            assert!(t - t0 < 4000);
        }
        assert_eq!(s.alert, Alert::Ac);
        assert!(t - t0 >= 2000, "{}", t - t0);
        // DC on the V tap (one bit only): AC? clears; no A-JACK either.
        let t1 = t;
        while s.alert == Alert::Ac {
            let v = s.alt_v;
            s.sample_cmp(t, if v { 0x20 } else { 0 });
            t += 10;
            assert!(t - t1 < 3000);
        }
        assert_eq!(s.alert, Alert::None);
        // Current in the A jack (PB1 windows): A-JACK within ~0.4 s.
        let t2 = t;
        while s.alert != Alert::AJack {
            let v = s.alt_v;
            s.sample_cmp(t, if v { 0 } else { 0x20 });
            t += 10;
            assert!(t - t2 < 500, "{}", t - t2);
        }
        // Hints off with "A-JACK only".
        s.set_mask(M_AJACK);
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.set_mask(M_AJACK | M_LIVE);
        s.alt = true;
        for t in (0..5000).step_by(10) {
            let v = s.alt_v;
            s.sample_cmp(t, if v { if (t / 10) % 2 == 0 { 0x20 } else { 0x10 } } else { 0 });
        }
        assert_eq!(s.alert, Alert::None);
    }

    #[test]
    fn alternating_peak_dc_window() {
        let mut s = Sentinel::new();
        s.configured(Kind::PeakA, 0, true);
        s.alt = true;
        // PB1 window, then the V-tap window: its pair is handed out, not
        // taken as an A-JACK hit even when large.
        s.sample_peak(107, -10, 10);
        assert!(s.alt_v && s.pk_v.is_none());
        s.sample_peak(214, 40_000, 41_000);
        assert_eq!(s.pk_v, Some((40_000, 41_000)));
        assert!(!s.alt_v);
        s.sample_peak(321, -10, 10);
        s.sample_peak(428, 40_000, 41_000);
        assert_eq!(s.alert, Alert::None);
        // DC? after 2 s of the condition.
        let mut t = 500;
        while t < 2400 {
            s.dc_hint(t, true);
            t += 214;
        }
        assert_eq!(s.alert, Alert::None);
        s.dc_hint(2600, true);
        assert_eq!(s.alert, Alert::Dc);
        s.dc_hint(3000, false);
        assert_eq!(s.alert, Alert::Dc);
        s.dc_hint(3700, false);
        assert_eq!(s.alert, Alert::None);
        // A-JACK (two PB1 windows in a row, the V window between) wins.
        s.sample_peak(3800, -50_000, 50_000);
        s.sample_peak(3907, 0, 0);
        s.sample_peak(4014, -50_000, 50_000);
        assert_eq!(s.alert, Alert::AJack);
    }

    #[test]
    fn fuse_hint_after_60s_of_zero() {
        let mut s = Sentinel::new();
        s.configured(Kind::CmpV, 0, true);
        let mut t = 0;
        while t < 59_000 {
            s.fuse_reading(t, true);
            t += 400;
        }
        assert_eq!(s.alert, Alert::None);
        while t < 61_000 {
            s.fuse_reading(t, true);
            t += 400;
        }
        assert_eq!(s.alert, Alert::Fuse);
        assert_eq!(s.alert.name(), "FUSE");
        assert!(!s.alert.red() && ques_bits(s.alert) == 0);
        // Any current: gone at once; the minute starts again.
        s.fuse_reading(t, false);
        s.fuse_reading(t + 600, false);
        assert_eq!(s.alert, Alert::None);
        s.fuse_reading(t + 1000, true);
        s.fuse_reading(t + 30_000, true);
        assert_eq!(s.alert, Alert::None);
        // A function change resets it.
        s.configured(Kind::Off, t + 30_100, true);
        s.fuse_reading(t + 60_000, true);
        assert_eq!(s.alert, Alert::None);
    }

    #[test]
    fn fuse_verdicts() {
        assert_eq!(fuse_verdict(48.1, false), FuseRes::Ok(48.1));
        assert_eq!(fuse_verdict(53.9, false), FuseRes::Ok(53.9));
        assert_eq!(fuse_verdict(600.0, true), FuseRes::Open);
        assert_eq!(fuse_verdict(f32::NAN, false), FuseRes::Open);
        // A short V-COM, the lead in COM instead of A, a leaky path.
        assert_eq!(fuse_verdict(0.2, false), FuseRes::Odd(0.2));
        assert_eq!(fuse_verdict(120.0, false), FuseRes::Odd(120.0));
    }

    #[test]
    fn diag_text_format() {
        let mut s = Sentinel::new();
        s.configured(Kind::PeakA, 0, true);
        s.sample_peak(100, -5, 7);
        let mut b = [0u8; 96];
        let n = s.diag_text(&mut b);
        assert_eq!(core::str::from_utf8(&b[..n]).unwrap(), "AD1PK_PB1,NONE,7,-1,-5,7,0,0,1,0,0,-1,0,-1,0");
        // A soft row's A-JACK: the badge, no alarm.
        let mut s = Sentinel::new();
        s.configured(Kind::Ad2A, 0, true);
        s.soft = true;
        s.sample_peak(100, -12_000, 12_000);
        s.sample_peak(200, -12_000, 12_000);
        assert_eq!((s.alert, s.ajack_alarm()), (Alert::AJack, false));
        s.soft = false;
        assert!(s.ajack_alarm());
    }

    #[test]
    fn jack_fields_reset_at_a_row_switch() {
        // Device c6a3ac3 note 3: RES AUTO -> 600 ohm with V-COM open, 3 PB1 hits at the
        // switch and their jack check (0 / 2150 mV, the 600 ohm row's own level), then no
        // hits: DBG:SENT? fields 14 / 15 kept 0 / 2150 for the whole open level (doc 4.13 check
        // list item 2 expects -1 / 0). RES -> continuity: 0 / -4 mV for ~3 s.
        let v = |x: f32| (x * JACK_CODES_PER_V) as i32;
        let jack = |s: &Sentinel| {
            let mut b = [0u8; 120];
            let n = s.diag_text(&mut b);
            let d = core::str::from_utf8(&b[..n]).unwrap().to_owned();
            let f: Vec<&str> = d.rsplitn(3, ',').collect();
            (f[1].parse::<i32>().unwrap(), f[0].parse::<i32>().unwrap())
        };
        // Ohm: a verdict while the hits last, then a range switch (`Dmm::set`: new_run).
        let mut s = Sentinel::new();
        s.configured(Kind::CmpA, 0, true);
        s.vgate = true;
        s.jchk = true;
        for t in (0..300).step_by(10) {
            s.sample_cmp(t, 0x20);
        }
        s.jack_result(300, true, v(2.14), v(2.15), f32::NEG_INFINITY);
        let mv = s.jack_mv;
        assert!((2140..=2150).contains(&mv), "{}", mv);
        assert_eq!(jack(&s), (0, mv));
        s.configured(Kind::CmpA, 310, false);
        s.new_run();
        assert_eq!(jack(&s), (-1, 0), "after the switch");
        // The switch's own hits in the new row, their verdict, then the open input: no hits.
        for t in (330..630).step_by(10) {
            s.sample_cmp(t, 0x20);
        }
        s.jack_result(630, true, v(2.14), v(2.15), f32::NEG_INFINITY);
        assert_eq!(jack(&s), (0, mv), "while the run lasts");
        for t in (640..900).step_by(10) {
            s.sample_cmp(t, 0);
        }
        assert_eq!(s.alert, Alert::None);
        assert_eq!(jack(&s), (-1, 0), "the run is over");
        // Continuity (AD2 peak): a verdict after the run's last hit window.
        let mut s = Sentinel::new();
        s.configured(Kind::Ad2A, 0, true);
        s.jchk = true;
        s.sample_peak(100, 0, 20_000);
        s.sample_peak(200, 0, 0);
        s.jack_result(260, false, v(-0.005), v(-0.003), f32::NEG_INFINITY);
        assert_eq!(jack(&s), (-1, 0));
        // An A-JACK in force keeps its verdict on show.
        s.sample_peak(300, 0, 20_000);
        s.sample_peak(400, 0, 20_000);
        s.jack_result(410, false, v(-0.49), v(-0.47), f32::NEG_INFINITY);
        assert_eq!((s.alert, jack(&s)), (Alert::AJack, (0, s.jack_mv)));
        assert!(s.jack_mv < -400);
    }
}
