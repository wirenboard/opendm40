//! Minimal SCPI command layer for the USBTMC interface (DMM subset).
//!
//! Every reply ends with '\n' on both transports (usbtmc.rs `terminate`,
//! bt.rs), definite-length blocks too (after the data).
//!
//! Commands are case-insensitive, long or short form (MEASure -> MEAS), several
//! per message separated by ';' (main.rs runs them one at a time through
//! scpi_msg.rs: replies in order, ';'-joined, one response). Supported:
//!   *IDN?  *RST  *CLS  *OPC?  *ESR?  *STB?  SYSTem:ERRor?
//!   CONFigure:<f>  MEASure:<f>?  READ?  FETCh?  [SENSe:]FUNCtion?
//!   [SENSe:]FUNCtion "<f>"   CONFigure?   INITiate  ABORt  SYSTem:LOCal|REMote
//!   (no-ops)   SYSTem:IDN:EMULation OFF|34401A / ?   SYSTem:MODel?
//! (*IDN? and SYSTem:MODel? name the model of the calibration record,
//! docs/model-variants.md; opendm40 uses the DM40C count limits on all models.)
//! where <f> = VOLTage[:DC] | VOLTage:AC | VOLTage:ACDC | CURRent[:DC] |
//! CURRent:AC | CURRent:ACDC | RESistance | CONTinuity | DIODe | CAPacitance |
//! FREQuency | TEMPerature. Functions are stock DMM mode numbers (see dmm.rs),
//! 16 / 17 = V / A AC+DC (true RMS of AC+DC = sqrt(AC^2 + DC^2), the stock
//! DM40B/C sub-mode; FUNCtion? answers "VOLT:ACDC" / "CURR:ACDC"),
//! 19 / 20 = V / A with the AUTO coupling (opendm40: VOLTage:AUTO /
//! CURRent:AUTO; the reading is the DC or the AC part, whichever AUTO picks,
//! dmm.rs AUTO_*; FUNCtion? answers "VOLT:AUTO" / "CURR:AUTO").
//!
//! Coupling of V / A (opendm40; the same switch as the V / A button):
//!   [SENSe:]VOLTage:COUPling DC|AC|ACDC|AUTO   select V with that coupling
//!   [SENSe:]CURRent:COUPling DC|AC|ACDC|AUTO   (= CONF:VOLT[:AC|:ACDC|:AUTO])
//!   [SENSe:]VOLTage:COUPling? / CURRent:COUPling?   DC / AC / ACDC, or
//!     AUTO,DC / AUTO,AC (the part AUTO shows now); DC while the function
//!     is not the current one (V / A start in DC, as the stock)
//!
//! Reply timing of READ? / MEASure:<f>? / FETCh? (main.rs, scpi_wait):
//!   - a reading of the current function already flowing (READ?, MEAS:<f>?
//!     naming the current function): the next fresh reading, or the latest
//!     one if none comes within SCPI_QUICK_MS (0.85 s). libsigrok's USBTMC
//!     transport has a hard 1000 ms Bulk-IN timeout, so this must hold also
//!     in a slow capacitance / temperature range or during a range hunt.
//!   - just switched (MEAS:<f>? changing the function, CONF:<f> then READ?,
//!     FETCh? before any reading): the first reading after the converter's
//!     settle deadline (Dmm::deadline: 2.5 s for resistance, then the
//!     autorange, about 3 s in all), MEAS:<f>? preferring a full-resolution
//!     average for at most SCPI_SETTLE_MS (1.5 s) after the command (a full
//!     window takes ~3.2 s at OSR 32768, so that first answer is usually a
//!     partial average; a later READ? has the +1 digit). Meant for hosts
//!     with a timeout of 5 s or more (pyvisa, a human); the 2 s PyVISA
//!     default is too short for resistance, and libsigrok waits after a
//!     function change itself.
//!   - +9.91E37 (SCPI NaN, "no reading"; +9.9E37 stays the overload) and
//!     "Data stale" in SYST:ERR? only if the current function has no
//!     reading at all SCPI_WAIT_MS (1.9 s) after that deadline (frequency:
//!     until AUTO_WAIT_MS after the entry at least: no signal reads 0 Hz
//!     ~3.3 s after it; a first gate below ~1 Hz can take longer). READ? /
//!     MEAS:RES? follow the ohm range hunt (each step moves the deadline,
//!     at most 8 s); FETCh? does not: at most SCPI_WAIT_MS after the
//!     deadline it was asked at, and +9.91E37 at once (no error) once an
//!     autorange step came since the entry (the range-change rule below;
//!     device 1a11eda: a hunt kept FETCh? silent for the driver's 5 s).
//!   - AUTO coupling before its first decision (its values are provisional,
//!     ../docs/history/switch-latency.md 6): FETCh? answers +9.91E37 at once (no wait,
//!     no error); READ? / MEAS? wait for the decision until AUTO_WAIT_MS
//!     (3.9 s) after the coupling was entered, then answer the part AUTO
//!     shows.
//!   - the settle wait is only for a change this host asked for (CONF,
//!     MEAS:<f>?, FUNC, TEMP:TRAN on the same transport, or READ? powering
//!     the DMM up). After a change made on the meter (touch, keys, the phone
//!     app, DBG:TAP / KEY, the other transport) READ? and MEAS:<f>? naming
//!     the current function answer within SCPI_QUICK_MS: the latest reading
//!     of the current function, else +9.91E37 and "Data stale" (libsigrok
//!     would otherwise time out after 1 s and end the acquisition; its DM40
//!     profile skips the NaN, other hosts see +inf as for an overload). A
//!     READ? already waiting when such a change comes answers at once with
//!     +9.91E37 and "Data stale": the host asked FUNCtion? before it, and a
//!     reading of the new function would be filed under the old one. An
//!     autorange step (V <-> mV, the uA / mA / A shunts) is no function
//!     change (func_id: FUNCtion? names the same function).
//!   - a range change (manual, SCPI, the range page, an autorange step;
//!     also a function change) with no reading of the new range yet
//!     (Dmm::reading_pending: the AC+DC / AUTO settle of the AC part, 3-5 s;
//!     the DC conversions vetted after a step, 0.1-0.4 s): READ? answers
//!     the first reading of the new range if it comes within SCPI_QUICK_MS,
//!     else +9.91E37 and "Data stale" as at the entry, never the latest
//!     reading from before the change (device decaf60: READ? held a 3.39 V
//!     transient for 5 s through an AC+DC autorange step); FETCh? answers
//!     +9.91E37 at once (not while HOLD freezes the readout).
//!   - no late reply to a query the host gave up on: USBTMC
//!     INITIATE_ABORT_BULK_IN / INITIATE_CLEAR, or a new command ("Query
//!     INTERRUPTED").
//!
//! CONFigure? answers as the Agilent / HP meters: "<func> <range>,<resolution>"
//! with <func> as FUNCtion? ("VOLT", "VOLT:AC", "CURR:ACDC", "RES", "CAP",
//! "FREQ", "TEMP", ...; "CONT" and "DIOD" bare; the power meter "POW
//! <V range>|AUTO,<A range>|AUTO"), the resolution that of the readout (its
//! decimals, one more with the +1 digit), the range the meter's present one
//! (`range_si`: 6 / 60 / 600..., 1000 V, 10 A; not the 34401A's 1 / 10 /
//! 100...; temperature: 60000 counts of the readout). No reading of the
//! current function yet: the range's 60000-count step as the resolution.
//!
//! Ranges and resolution (opendm40, 34401A / 34465A style; App::scpi_range):
//!   CONFigure:<f> [<range>|AUTO|MIN|MAX|DEF[,<res>|MIN|MAX|DEF]]   select
//!     the function, then the range (none / AUTO / DEF = autorange, also on
//!     the function already selected), then the resolution if given
//!     (CONT / DIOD: no argument; CAP / FREQ: AUTO / DEF only)
//!   [SENSe:]<f>:RANGe <v>|AUTO|MIN|MAX|DEF / ?   the smallest range that
//!     holds <v>: V 60 mV, 600 mV, 6, 60, 600, 1000 V; A 600 uA, 6000 uA, 60 mA,
//!     600 mA, 6 A, 10 A; ohm 60 ohm .. 60 Mohm (the MF range page's
//!     items; above the top one: "Data out of range"). The query gives the
//!     present range also while autoranging (CAP 6 nF .. 60 mF, FREQ 60 Hz
//!     .. 60 MHz, not settable)
//!   [SENSe:]<f>:RANGe:AUTO ON|OFF|ONCE / ?   ON = the AUTO key: back to
//!     the autorange chosen on the meter (AUTO+ or the mV- / uA-first AUTO;
//!     SCPI does not change that choice); OFF / ONCE keep the present range
//!     (also CAP / FREQ); ? 0 also while REL holds the range
//!   [SENSe:]<f>:RESolution <v>|MIN|MAX|DEF / ?   the FR-05 rate (global,
//!     not per function): <v> at least the range's 60000-count step (or
//!     MAX) = FAST, finer (MIN, DEF) = MED unless SLOW is set (kept); ? the
//!     resolution of the present range at the rate (one decimal more where
//!     the range has the +1 digit: DC V 600 mV .. 600 V, ohm 600 ohm .. 60
//!     kohm, not in AC+DC)
//!   <f> as CONFigure's: VOLTage[:DC] | :AC | :ACDC | :AUTO, CURRent[...],
//!   RESistance (FRESistance), CAPacitance, FREQuency. There is one range
//!   state, the current function's (the V / A button's, whatever the
//!   coupling: the MF range page's). A bare VOLTage / CURRent names that
//!   quantity in any coupling (VOLT:RANG:AUTO? in AC V, AC+DC or AUTO
//!   coupling answers the present state; in the power meter RANG? /
//!   RANG:AUTO? answer its V / I channel's, as POW:VOLT:RANG? /
//!   POW:CURR:RANG?, a set there is "Settings conflict"); an explicit
//!   coupling names that coupling's function. A set naming another
//!   function is "Settings conflict"; a query of another function is no
//!   error (as a 34401A answers any function's): RANG:AUTO? 1 (selecting
//!   a function or a coupling starts it in autorange), RANG? / RES?
//!   +9.91E37 (no range until it is selected). CONT / DIOD / TEMP have
//!   none ("Undefined header"). A set acts as a tap on the MF range
//!   page (REL off); refused while the data logger runs or the Wave view
//!   is open ("Settings conflict"; CONF then only selects the function).
//!
//! *IDN? = "Alientek,DM40A|B|C,<24 hex digits of the chip UID>,opendm40 0.1"
//! (the serial number = the USB one, src/uid.rs).
//! *IDN? compatibility (SYSTem:IDN:EMULation, persisted, default OFF): with
//! 34401A, *IDN? answers "HEWLETT-PACKARD,34401A,0,opendm40 0.1" so that hosts
//! with a 34401A profile only (libsigrok >= 0.5.2 / SmuView without the DM40
//! profile) take the meter; READ? then never waits longer than the quick
//! budget (+9.91E37, read as +inf there, until the first reading after a
//! function change).
//! Caveats: that profile knows DC / AC V and A, RES (and FRES = RES here),
//! CONT, DIOD, FREQ (and PER, which the DM40 lacks): in CAP, TEMP and AC+DC
//! the host stops with "unknown measurement quantity"; that profile has no
//! range or resolution settings (the DM40 profile has); the digits come
//! from CONFigure?. PyVISA scripts written for a real 34401A get its
//! command subset (CONF:<f> <range>,<res>, <f>:RANG, :RES work as above).
//!
//! Temperature / system (opendm40 extensions):
//!   CONFigure:TEMPerature [<sensor>]   MEASure:TEMPerature? [<sensor>]
//!     <sensor> = TCK | PT100 | PT1000 | NTC10K3950 | NTC10K3435 | NTC100K3950
//!     | NTCUSER (dmm.rs T_NAMES; also TC / K for TCK); without it the current
//!     sensor is kept. Reading in the UNIT:TEMP unit (OL = +9.9E37).
//!   UNIT:TEMPerature C | F / ?   global temperature unit (also Settings 2/2):
//!     readings, MEAS:TEMP:CJ? and SYST:TEMP? follow it; offsets stay in C
//!   [SENSe:]TEMPerature:TRANsducer <sensor> / ?   select / query the sensor
//!   [SENSe:]TEMPerature:OFFSet <C> / ?   offset of the CURRENT sensor, -20..20
//!     in 0.1 C steps (thermocouple: applied to the cold junction)
//!   [SENSe:]TEMPerature:NTC <R25_ohm>,<Beta> / ?   the NTCUSER parameters
//!   [SENSe:]TEMPerature:EXCitation?   range,R_ohm,I_A,P_W of a resistive sensor
//!     (I, P estimated from the measured ohm short-circuit current;
//!     TC: error)
//!   MEASure:TEMPerature:CJ?   cold junction (HY3131 internal sensor, C, incl.
//!     the TC offset),age_s. Measured only in thermocouple mode; outside it the
//!     last value with its age (and "Data stale" in SYST:ERR?); never measured:
//!     +9.9E37 and an error.
//!   SYSTem:TEMPerature?   MCU (GD32) internal sensor, C (typical constants)
//!   SYSTem:RAIL AUTO|BOTH / ?   analog rail policy (default AUTO, persisted)
//!   SYSTem:RAIL:STATe?   dmm_rail,scope_rail (1/0),wakes,lost_row,silent (DMM
//!                        rail wakes and stall-watchdog restarts, dmm.rs)
//!   SYSTem:ALERt?   wrong-connection alert in force (sentinel.rs, docs/
//!     power-meter-and-misconnect.md 4): NONE | OVERLOAD (the uA / mA shunt
//!     over range: moved to 10 A, latched) | AJACK (current in the A jack
//!     outside the current functions) | VJACK (voltage on V/ohm, no current,
//!     in DC / AC / AC+DC A) | LIVE (a source on the circuit in ohm /
//!     continuity / diode) | DC (hint: DC on V/ohm in AC V) | AC (hint: AC in
//!     DC V 6 V) | FUSE (hint: an A function at zero for 60 s)
//!   SYSTem:ALERt:ENABle <mask> | ON | OFF / ?   alerts enabled, bit 0 AJACK,
//!     1 VJACK and the DC / AC / FUSE hints, 2 LIVE (Settings 3/3 "Jack
//!     alerts"); default 7 = ON, persisted; bit 3 (the removed "Leads in
//!     A?" reminder) is accepted and ignored;
//!     OVERLOAD (the uA / mA shunt over range: the meter moved to the 10 A
//!     shunt, latched; doc 4.7) is not maskable
//!   SYSTem:ALERt:ALARm LOUD | VOLume | OFF / ?   the danger alarm (doc
//!     4.10, Settings 3/3 "Danger alarm"): A-JACK, A-PROT while its current
//!     flows and a current above the 10 A rating sound a two-tone pattern at
//!     full duty (LOUD, default), at the key / alert volume (VOLUME) or not
//!     at all (OFF); persisted. `?` -> LOUD | VOLUME | OFF
//!   SYSTem:BEEPer:VOLume <0..100> / ?   the key / alert volume in % (Settings
//!     "Key/alert volume"), persisted
//!   DBG:ALARm?   <sounding 0|1>,<silenced 0|1>,<events>,<setting 0..2>
//!   SYSTem:FUSE:TEST   start the fuse check (doc 4.3: one lead from V/ohm to
//!     A, nothing else): ohm 600 ohm for 3 readings, then the function before
//!     (the power meter restarts its energy); non-blocking
//!   SYSTem:FUSE?   IDLE | BUSY | OK,<ohm> (~48 ohm) | OPEN (OL: fuse or lead)
//!     | FAIL,<ohm> (another resistance) | FAIL,NONE (no reading)
//!   FETCh:BURDen?   burden voltage (V) of the latest reading of a current
//!     function: I x R_path of the shunt in use (POW:BURDen; default the bare
//!     shunt 10 mohm / 0.5 ohm / 47.5 ohm), DC signed, AC and AC+DC the RMS,
//!     REL ignored; outside the A functions / no reading: +9.9E37 and "Data
//!     stale"; over range: +9.9E37 (doc 3.15). Works with the readout off.
//!   DISPlay:BURDen ON|OFF / ?   the burden voltage readout on the DMM page
//!     (DC A aux1, AC A aux2 instead of the duty cycle; not in AC+DC);
//!     Settings 3/3 "Burden voltage", persisted, default OFF
//!   [SENSe:]RESistance:NEGative ON|OFF / ?   ohm readings below 0 shown
//!     (shorted leads: the internal offset, a few tens of mohm) instead of
//!     clamped at 0 as the stock; Settings 3/3 "Negative ohm", persisted,
//!     default OFF
//!   MEMory:STATe:RECall:AUTO ON|OFF / ?   Settings 3/3 "Mode Remember"
//!     (the stock's switch; the header is the Keysight 34461A's "recall the
//!     power-down state at power-on"): ON = a power-on continues the
//!     multimeter as it was (app.rs `restore`); persisted, default OFF
//!   POWer:BURDen <ohm>|DEFault / ?   (power meter section, 3.9) the R_path
//!     used by both: in a DMM A function that function's shunt, else the
//!     power meter's current range's
//!   POWer:BURDen:STATe ON|OFF / ?   the power meter's "At load" (V - I R,
//!     P - I^2 R; the "@load" marks): default OFF since doc 3.16 (records
//!     from older builds load OFF too)
//!   POWer:COUPling DC|AC|ACDC / ?   the power meter's coupling; CONF:POW
//!     from another function starts in DC (doc 3.16): set it after CONF:POW.
//!     FETCh:POWer:ALL? "no value" fields are +9.91E37: PF / S / Q / F in AC
//!     below the AC floors, F whenever no frequency is measured (also DC)
//!   STATus:QUEStionable:CONDition?   bit 9 while a red alert (AJACK,
//!     OVERLOAD) is on, bit 1 while OVERLOAD is (SCPI CURRent)
//!   STATus:QUEStionable[:EVENt]?   the same bits latched since the last
//!     read / *CLS (cleared by the read); no enable / transition registers,
//!     *STB? stays 0
//!
//! Statistics / limits of the CURRENT DMM function (FR-01 / 02 / 06; values
//! in SI units of the function, as READ?):
//!   CALCulate:AHOLd ON|OFF / ?            AutoHold (as RUN held 1 s)
//!   CALCulate:AVERage[:AVERage]?  CALCulate:AVERage:MINimum? / MAXimum?
//!     no sample yet: +9.9E37 and "Data stale" in SYST:ERR?
//!   CALCulate:AVERage:COUNt?              samples in the statistics
//!   CALCulate:AVERage:SDEViation?         sample standard deviation (N - 1;
//!     0 after one sample)
//!   CALCulate:AVERage:CLEar               restart the statistics
//!   CALCulate:AVERage:STATe ON|OFF / ?    recording session (no idle
//!     power-off; ON restarts the statistics)
//!   CALCulate:LIMit:LOWer|UPPer <v> / ?   set a limit (turns the check on);
//!     query: +9.9E37 when not set
//!   CALCulate:LIMit:STATe ON|OFF / ?      limit check on / off
//!   CALCulate:LIMit:CLEar                 clear both limits (opendm40; not the
//!     Keysight meaning "clear the fail indicator")
//!   CALCulate:LIMit:FAIL?                 1 = the value is outside the limits
//!
//! Conversion rate / display smoothing (FR-05; persisted):
//!   [SENSe:]RATE SLOW|MEDium|FAST / ?     AD1 conversion rate of DC V / mV /
//!     A and ohm (dmm.rs FR-05 table; other functions keep the stock timing):
//!     FAST 10/s, MED and SLOW 2.5/s steady (10/s while moving; SLOW
//!     averages 16 conversions, 32 on ohm r0..r3);
//!     query: SLOW / MED / FAST
//!   [SENSe:]RATE:PERiod?                  measured interval between readings,
//!     s (0 until two readings were seen)
//!   CALCulate:SMOothing[:STATe] ON|OFF / ?   display smoothing on (LOW) / off
//!   CALCulate:SMOothing:RESPonse SLOW|MEDium|FAST / ?   SLOW = HIGH (long
//!     window), MEDium / FAST = LOW; query SLOW / FAST. The smoothing feeds
//!     the readout, READ? / MEAS? / FETCh?; MIN/MAX/AVG and the CSV log stay
//!     unsmoothed (meter.rs header)

use crate::dmm::Reading;

/// *IDN?: "Alientek,DM40<m>,<serial>,opendm40 <version>", <m> = the
/// calibration model letter (ident.rs), <serial> = the chip UID as 24 hex
/// digits (src/uid.rs; the USB serial number too), <version> =
/// `ident::VERSION`. libsigrok shows it as [S/N: ..], ngscopeclient as the
/// serial; both take any comma-free field.
const IDN_HEAD: &[u8] = b"Alientek,DM40";
const IDN_TAIL: &[u8] = b",opendm40 ";

fn model() -> (u8, u8) {
    crate::ident::model()
}
/// *IDN? while SYST:IDN:EMUL 34401A is on: the HP 34401A identity (vendor
/// as the real meter's, libsigrok maps it to "HP"), our firmware in the
/// version field: "HEWLETT-PACKARD,34401A,0,opendm40 <version>".
pub const IDN_34401A: &[u8] = b"HEWLETT-PACKARD,34401A,0";

/// *IDN? compatibility mode (SYSTem:IDN:EMULation OFF|34401A; persisted, app.rs
/// persist_words p[0] bit 14): 0 = off (default), 1 = HP 34401A. For hosts
/// that pick a driver profile from *IDN? and know no DM40, e.g. libsigrok /
/// SmuView builds without the Alientek DM40 scpi-dmm profile: their "HP
/// 34401A" profile (since libsigrok 0.5.2) sends CONF:<f>, CONF?, INIT,
/// READ?, *OPC?, ABORT. With it on, READ? / MEAS:<f>? never wait for a
/// first reading longer than the quick budget (main.rs: +9.91E37 for the
/// first ~1..3 s after a function change), since such hosts do not wait
/// after CONF and time out after 1 s (USBTMC) / 1.5 s.
static IDN_EMUL: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

pub fn idn_emul() -> u8 {
    IDN_EMUL.load(core::sync::atomic::Ordering::Relaxed)
}

pub fn set_idn_emul(v: u8) {
    IDN_EMUL.store(if v == 1 { 1 } else { 0 }, core::sync::atomic::Ordering::Relaxed);
}

pub enum Out {
    /// Nothing to send (command without a query).
    None,
    /// Reply text is in the buffer.
    Text,
    /// Switch function (CONFigure / FUNCtion set), no reply.
    Configure(u8),
    /// Switch function (if Some) and reply with the next fresh reading.
    Measure(Option<u8>),
    /// Reply with the last reading.
    Fetch,
    /// Scope / generator command; `None` argument = query.
    Instr(Cmd),
    /// Debug command (DBG:...), handled by the main loop.
    Dbg(Dbg),
    /// Data-logger command (LOG:...), handled by the main loop with the logger.
    Log(Log),
    /// USB-disk file command (MMEMory:...), handled by the main loop (csvlog.rs).
    Mmem(Mmem),
    /// Inrush current (INRush:...), handled by the main loop with the app.
    Inr(Inr),
    /// Protocol decoder (DECode:...), handled by the main loop with the app.
    Dcd(Dcd),
}

/// Protocol decoder on the scope page (src/decode.rs, src/app_decode.rs);
/// `None` = query:
///   DECode:PROTocol OFF|UART|MODBus / ?  OFF / UART / MODB (the band)
///   DECode:BAUD <n>|AUTO / ?         the bit rate set, 300 .. 115200 or AUTO
///   DECode:RATE?                     the bit rate of the last decode (0: none)
///   DECode:FORMat 8N1|8E1|8O1|7E1 / ?
///   DECode:POLarity NORMal|INVerted / ?   NORM / INV (RS-232: inverted)
///   DECode:THReshold <V>|AUTO / ?    the level set (AUTO or V)
///   DECode:LEVel?                    the level of the last decode, V
///   DECode:MODE BAND|LOG / ?         the band on the scope page, or the
///                                    frame log (View::Decode, app_log.rs)
///   DECode:COUNt?                    frames,bytes,framing,parity,crc,lost
///   DECode:FRAMes? [<n>]             #4nnnn block of frame lines. Band: from
///                                    frame `n` of the decoded record; Log:
///                                    the unread frames (at most n; 0 all)
///   DECode:CLEar                     Log: the list and counters start again
#[derive(Clone, Copy, PartialEq)]
pub enum Dcd {
    /// decode::P_OFF / P_UART / P_MODBUS.
    Proto(Option<u8>),
    /// Some(0) = AUTO.
    Baud(Option<u32>),
    Rate,
    Fmt(Option<u8>),
    Inv(Option<bool>),
    /// Some(NaN) = AUTO.
    Thr(Option<f32>),
    Level,
    /// ui_decode MODE_BAND / MODE_LOG.
    Mode(Option<u8>),
    Count,
    Frames(u32),
    Clear,
}

/// Inrush current (src/inrush.rs, src/app_inrush.rs; docs/inrush-current.md):
///   INRush[:STATe] ON|OFF / ?    open the Inrush Current page (the A
///                                function, AC row, 10 A range) and arm /
///                                close it (the A function's settings back)
///   INRush:ARM                   (re)arm: a new capture (opens the page)
///   INRush:THReshold <A> / ?     the trigger step, 0.1 .. 50 A (default 1)
///   INRush:CONDition?            OFF | IDLE | ARMING | ARMED | TRIGGERED |
///                                SAVING | DONE
///   INRush:FETCh?                Irms(100 ms),Ipeak,Iidle,lost: A (peak
///                                signed, +9.9E37 = OL), lost samples;
///                                all +9.9E37 + "Data stale" without a capture
///   INRush:FILE?                 "LOGS/FASTnnn.DLG" of the capture, or ""
///   INRush:PREamble?             rate_hz,samples,trigger_index,a,b (A = a + b
///                                * code; samples 0 without a capture)
///   INRush:DATA? [<start>[,<count>]]  #4nnnn block of the trace's codes,
///                                little-endian i16, count <= 3000 (default)
#[derive(Clone, Copy, PartialEq)]
pub enum Inr {
    State(Option<bool>),
    Arm,
    Thr(Option<f32>),
    Cond,
    Fetch,
    File,
    Pre,
    Data(u32, u32),
}

/// File commands on the USB disk's root (csvlog.rs gallery backend):
///   MMEMory:CATalog?        <used>,<free>{,"<name>,<type>,<size>"} (bytes; type
///                           always BIN: S*.BMP, LOG*.DLG, SCOP*.DLG; csvlog::catalog_scpi_on)
///   MMEMory:DELete "<name>" delete one S*.BMP / LOG*.DLG / SCOP*.DLG (USB only:
///                           refused over Bluetooth; refused for the file being recorded)
///   MMEMory:DATA? "<name>"[,<offset>[,<length>]]
///                           definite-length block #4nnnn of file bytes
///                           offset.. (length default and max 6000; short or
///                           empty at the end). "LOGnnn.CSV" / "SCOPnnn.CSV"
///                           = the CSV generated from LOGnnn.DLG / SCOPnnn.DLG
///                           (dlog.rs, the same bytes as LUN 1's virtual
///                           file); any other root file its raw bytes
#[derive(Clone, Copy)]
pub enum Mmem {
    Cat,
    Del([u8; 11]),
    Data([u8; 11], u32, u32),
}

/// MMEM:DATA? block payload at most (fits the 6144-byte USBTMC response).
pub const MMEM_DATA_MAX: u32 = 6000;

/// Data-logger commands (FR-38), executed by the main loop against `Logger`:
///   LOG:STATe ON|OFF        start / stop the fast stream
///   LOG:STATe?              1 / 0
///   LOG:OSR <0..7>          AD1OSR code (OSR = 256 << n, default 3); takes
///                           effect at START. Period OSR * 30 / 4.9152 MHz, x2
///                           chopped: 3 = 25 ms (40/s) .. 0 = 3.125 ms (320/s)
///   LOG:OSR?                current code
///   LOG:CHOP ON|OFF         AD1 chopper (default ON, as every stock row); OFF
///                           doubles the rate (OSR 256: 640/s, the chip's max)
///                           but leaves the converter offset in the samples
///                           and adds low-frequency noise; takes effect at START
///   LOG:CHOP?               1 / 0
///   LOG:SOURce AD1|AD2      converter (default AD1; takes effect at START).
///                           AD1: DC V / mV / A modes, LOG:OSR + LOG:CHOP.
///                           AD2 (FR-38 B): the stock AC rows, AC mV and AC A /
///                           mA / uA DC-coupled, AC V AC-coupled (~0.5 Hz);
///                           rate LOG:SRATe, LOG:CHOP = ENCHOPAD2 (logger.rs)
///                           ("Settings conflict" while a session runs)
///   LOG:SOURce?             AD1 / AD2
///   LOG:SRATe <S/s>         requested AD2 rate; the nearest rung of 1200,
///                           2400, 4800, 9600, 19200, 38400, 76800 is used (chop on or off: the
///                           chopper does not slow AD2) (default 2400, 4800
///                           with the ad2-isr stream ISR); 76800 needs the DMA
///                           frames (the default; DBG:AD2:DMA 0 or a forced /16
///                           / /32 divider cap it at 38400 / 19200, the rung
///                           used is in LOG:PREamble?); takes effect at START
///   LOG:SRATe?              the requested rate
///   LOG:RATE?               measured mean sample rate, Hz (0 until 2 samples)
///   LOG:COUNt?              buffered,captured,overwritten,missed,lost (lost =
///                           conversions not captured, from the span and the
///                           nominal period; missed = the per-gap estimate;
///                           overwritten = captured but dropped, ring full:
///                           the newest are dropped, seqs stay consecutive)
///   LOG:GAPs?               bad,max_gap_us,g2,g3_4,g5_16,g17_128,g129 (bad =
///                           AD2 reads dropped as torn; gN = gaps of N periods)
///   LOG:PREamble?           mode,range,osr,nominal_us,measured_hz,chop,source,
///                           req_hz,nominal_mhz,r26,coupling (source 0 AD1 / 1
///                           AD2; osr = AD1OSR or AD2OSR code; r26 = AD2 R26 low
///                           bits; coupling 0 DC / 1 AC). Scaling: AD1 raw
///                           counts, 1.389 V / 8388607 cts * range decade; AD2
///                           +-262143 codes = +-1.25 V at the AD2 input (IG
///                           1.0), times the stock range factor (mV: /x10 or
///                           x1 op gain; A: / (shunt * op gain); AC V: the
///                           divider); calibration gain/offset NOT applied
///   LOG:SCALe?              a,b: the AD2 session's value = a + b * raw in V
///                           or A, the scale the device uses (the wave view,
///                           the fast logger's file): the stock calibration
///                           (mV, A: LPF gain / offset) or, in DC V, the
///                           nominal gain and the entry self-cal's offset
///                           (feasibility E7); 0,0 for AD1 / no AD2 row
///   LOG:DATA?               definite-length #4nnnn block, oldest first: ROWS =
///                           12-byte LE records {u32 seq, u32 ts_us, i32 raw};
///                           PACKed / DLG = a compact reply (logger.rs
///                           "Compact LOG:DATA?"); an empty ring: #40000
///   LOG:FORMat ROWS|PACKed|DLG / ?  the LOG:DATA? layout (default ROWS;
///                           any time, each reply describes itself)
///   LOG:CSV ON|OFF          CSV recording of the DMM readings to the USB disk
///                           (csvlog.rs; as the DMM page swipe down). ON is
///                           USB only: refused over Bluetooth ("Command
///                           protected", main.rs; OFF and LOG:CSV? work). ON fails:
///                           "Mass storage busy" while a PC has the disk mounted,
///                           "Settings conflict: Wave view" / ": inrush page"
///                           (their AD2 stream pauses the DMM: no rows), else
///                           "Settings conflict" (no reading yet, already on)
///   LOG:CSV?                state,file,rows,bytes,committed,dropped (state 0 idle,
///                           1 recording, 2 stopping, 3 done, 4 full, 5 no disk,
///                           6 error; file -1 until the first commit)
///   LOG:CSV:INTerval <s> / ?  row interval of the next recording (Record page,
///                           decision 2026-10-22 #1): 0 = every reading
///                           (default), 1, 10, 60 = one row per interval with
///                           the mean of its readings; other values: "Illegal
///                           parameter value"; while recording: "Settings
///                           conflict" (the running one keeps its settings)
///   LOG:CSV:DURation <s> / ?  length of the next recording: 0 = until stopped
///                           (default), 600, 3600, 28800, 86400; as INTerval
///   LOG:CSV:FREE?           free bytes on the USB disk ("Mass storage busy"
///                           while a commit / screenshot / upload holds it)
///   LOG:DEST DISK|HOST      where LOG:STATe ON sends the samples (default
///                           HOST = LOG:DATA?). DISK: the fast logger
///                           (fastlog.rs) writes LOGS/FASTnnn.DLG (served as
///                           FASTnnn.CSV): LOG:STATe ON pre-allocates and
///                           pre-erases LOG:DURation's worth (bounded by the
///                           free space; LOG:DISK? shows the progress), then
///                           starts the session; it ends at LOG:STATe OFF, at
///                           the duration or when the file is full. LOG:DATA?
///                           answers an empty block meanwhile. USB only (as
///                           LOG:CSV ON: "Command protected" over Bluetooth)
///   LOG:DEST?               DISK / HOST
///   LOG:DURation <s> / ?    length of a DISK session in device time, 0 =
///                           until stopped / the disk is full (default 60)
///   LOG:DISK?               state,file,alloc,bytes,rows,prep_pct,fifo_hw,programs,
///                           tpp_us,tpp_max_us (state 0 idle, 1 preparing, 2
///                           ready, 3 running, 4 stopping, 5 done, 6 full, 7 no
///                           disk, 8 error; file -1 until the entry exists;
///                           bytes = file bytes written; fifo_hw = the most
///                           bytes coded but not yet programmed, of 8192: the
///                           flash's margin; programs = page / header / partial
///                           programs started; tpp = the page-program probe of
///                           a session above 38400 S/s, mean / max us, 0 = none:
///                           over 800 / 1500 us the session runs at 38400),
///                           gate,gate_isr_permille,gate_fill (the writer
///                           trial of such a session, fastlog.rs `Gate`: 0
///                           none, 1 kept the rung, 2 fell back to 38400; the
///                           stream ISR's CPU share and the ring's high water,
///                           of 2048, in its 1 s window), writer_permille
///                           (the Run passes' share of the main loop's CPU,
///                           the stream ISR's time out of both; 1000 minus
///                           it = the headroom at this rate)
/// Bluetooth (an untrusted ~11.5 kB/s link): LOG:STATe ON is refused above
/// BT_MAX_HZ ("Rate too high for Bluetooth", every AD2 rate; AD1 up to
/// 640 S/s works) and with LOG:DEST DISK ("Command protected"); LOG:DATA?
/// from Bluetooth of a faster session (started over USB) is refused too.
#[derive(Clone, Copy)]
pub enum Log {
    State(Option<bool>),
    /// LOG:SOURce AD1 (false) | AD2 (true).
    Source(Option<bool>),
    /// LOG:SRATe <S/s> (AD2).
    SRate(Option<u32>),
    Gaps,
    Osr(Option<u8>),
    Chop(Option<bool>),
    Rate,
    Count,
    Preamble,
    /// LOG:SCALe? (main.rs `log_scale`).
    Scale,
    Data,
    /// LOG:FORMat: Some(logger::FMT_*) to set, None = query.
    Format(Option<u8>),
    Csv(Option<bool>),
    /// LOG:CSV:INTerval / DURation / CHANge: Some(index into
    /// csvlog::INTERVALS_S (csvlog::ON_CHANGE: CHANge) / DURATIONS_S /
    /// CHANGES) to set, None = query.
    CsvInt(Option<u8>),
    CsvDur(Option<u8>),
    CsvChg(Option<u8>),
    CsvFree,
    /// LOG:DEST DISK (true) | HOST (false) | ? (None).
    Dest(Option<bool>),
    /// LOG:DURation <s> | ?.
    Dur(Option<u32>),
    /// LOG:DISK?
    Disk,
}

/// DBG:PING? field 3: ms until the watchdog feeding stops, saturated at
/// u32::MAX. The boot hold (main.rs INITIAL_HOLD_MS, u64::MAX / 4) has no
/// limit and reads u32::MAX; a plain `as u32` wrapped it (issue #97).
pub fn keepalive_left(deadline: u64, now: u64) -> u32 {
    deadline.saturating_sub(now).min(u32::MAX as u64) as u32
}

/// Debug commands over USBTMC (development aids, not SCPI standard):
///   DBG:PING? [ms]          uptime_ms,flags,keepalive_left_ms (extends keepalive;
///                           keepalive_left 4294967295 = no limit, see
///                           keepalive_left)
///   DBG:OHM6 g,off,skip,keep,leak | DBG:OHM6?  600 Mohm range tuning (RAM
///                           only, dmm::Ohm600): Rref gain, offset uV subtracted
///                           from Vfb - Vx, conversions dropped / kept per phase,
///                           the learned leak nS | g,off,skip,keep,leak,Vx,Vfb
///                           (V, the last phase medians)
///   DBG:IDLE <ms> | DBG:IDLE?  the idle shutdown after <ms> instead of the
///                           Shutdown setting (0 = the setting; RAM only, for
///                           tests) | ms since the last touch / key, ms since
///                           the last input or host activity, shutdown ms
///   DBG:TAP x,y  DBG:SWIPE L|R  inject a touch tap / view swipe
///   DBG:KEY n[,ms]          side key n (0 MENU, 1 RUN, 2 AUTO, 3 MF): a full
///                           click (key-down, key-up); with ms > 0 the key-up
///                           follows ms later (long press: RUN >= 1000 ms in a
///                           DMM view = AutoHold on/off); max 60000
///   DBG:SWIPE D,n           synthetic n-finger swipe down (3 = screenshot, 2 = power off)
///   DBG:READout?            the DMM readout as drawn (App::shown_reading):
///                           kind,unit,value_si,hold,range (kind P placeholder, G
///                           grey provisional, F final, N none; value NAN for
///                           dashes, +9.9E37 OL; hold Meter::hold_state; range
///                           AUTO / AUTO+ / MANU; in manual the status bar
///                           shows the range, ui::range_text)
///   DBG:GRAB <ms> | ?       scope drag hold-to-grab time (0 = direct drag, default)
///   DBG:PINCH 0|1|2 | ?     scope pinch mode (RAM only): 0 one step per 2.15x
///                           of finger span, 1 one step per 1.25x (default), 2 live
///                           stretch, snapped to the nearest step on release
///   DBG:GEST?               last scope drags, oldest first: n then per drag
///                           ";target,axis,delta_px,ms" (target 1 level, 2 trigger
///                           position / pan, 3 offset, 4/5 X/Y cursor a, 14/15 b;
///                           axis H|V), then "|POS hpos,trig_row,pos" (the
///                           trigger position px, trigger row, CH1 position now),
///                           then "|AUTO..." (the last AUTO-set)
///   DBG:SENTinel?           misconnect sentinel (sentinel.rs): kind,alert,mask,
///                           ctsta,pkhmin,pkhmax,streak,hit_windows,samples,
///                           probe_mA,aprot_ms (A-jack protection, doc 4.8),
///                           vside (V_GATE: -1 / 0 free / 1 driven), soft (1 =
///                           A-JACK here is badge only, no danger alarm, doc
///                           4.10; ctsta -1 in kinds without the comparator),
///                           jack (the jack check, sentinel.rs JACK CHECK: -1
///                           none, 0 free, 1 driven, 2 driven by the
///                           comparator's both-bit reads), jack_mV (its peak;
///                           the present hit run's, else -1 / 0)
///   DBG:SENTinel:LOG?       the last 8 alert assertions since power-on:
///                           n then per one ";ALERT,KIND,ms" (oldest first;
///                           n counts all, emu/hy3131/switchtest.py)
///   DBG:SENTinel:VARiant <0..127> / ?
///                           device-check row variants, RAM only (sentinel.rs
///                           VAR_*): +1 AC V keeps OP1's chopper (f80524f row),
///                           +2 no AC V sentinel, +4 comparator on DC V 60 V+,
///                           +8 none of the later sentinels (AC+DC, AC A,
///                           continuity, diode, cap r0..4, frequency), +16 no
///                           DC? / AC? input alternation, +32 AC A's AD1 on
///                           SENSE - PB5 (the 40728b8 row), +64 AC A AD1 at IG
///                           0.9 (where SENSE sits; diagnostic)
///   DBG:SENTinel:REG?       HY3131 R20..R37 read back (48 hex digits), relay PA
///                           (0 V/A, 1 ohm network, 255 unknown), mode, range,
///                           a_blind (1 = no temperature row since DC V 60 V+ /
///                           an ohm-network row / a chip restart)
///   DBG:SRC <vref>,<path>[,<sense>][,FORCE] | OFF   ohm-source map row (src/dbgsrc.rs):
///                           OP3 VREF = SFUVR tap <vref> on the stock ohm / diode row <path>
///   DBG:SRC?                0 | 1,cycles,vref,path,sense,V_in,V_far,I,V_vref,V_drop,V_com,
///                           VDDA-AGND,AGND-VSS;10 AD1 codes;R20..R35 hex
///   DBG:SENTinel:PRECond <ms> / ?  RAM only, default 100, 0 = off: entering
///                           DC A / mA / uA while a_blind (V-JACK? enabled),
///                           run the temperature row for <ms> (<= 2000, blocking)
///                           first (device 40728b8: 24 V on V-COM was seen in DC
///                           A only after TEMP; d2aff22: 100 ms is enough)
///   DBG:ACDC:METH A|B|C / ? AC+DC method (acdcm.rs; RAM only, default A =
///                           the stock alternation; a change restarts AC+DC).
///                           METH / STAT / ZERO / R0: feature acdc-meth, else
///                           undefined header
///   DBG:ACDC:STAT?          meth,step,mode,range,ac,dc,n_ac,n_dc,n_rd,hz,corr,
///                           c_per_code,c_offset,lpf_codes,sqrt_rms_codes,
///                           pk_pp_codes,ac_src,gain_span_codes,gain_src,
///                           gain_dis_codes,lpf_slope (step 0 the stock DC
///                           row, 1 meas, 2 run, 3 C V's zero; C V: the
///                           anchor's per_code / offset, AD2's peak p-p, the
///                           AC part from 0 the RMS / 1 the peak bound / 2
///                           none, the gain's span, 0 = the table's; its
///                           source 0 table / 1 internal zero / 2 two refs /
///                           3 two refs over a biased zero; the last two-ref
///                           gain vs the one in use over its span, codes; the
///                           steady window's LPF slope, codes / s)
///   DBG:ACDC:ZERO k,v|AUTO / ?  fix slot k's correction (0..3 V ranges, 4..9 A
///                           unit*2+range; B zero, C A offset) / measure again;
///                           ? = valid,fixed;c0..c9
///   DBG:ACDC:R0 1|2 / ?     B 6 V: (i) AD1 SENSE - PB5 / (ii) OP1O - RLU
///   DBG:ACDC:TRACE 0|1|2    AC+DC event trace (acdctr.rs; feature acdc-trace, else
///                           undefined header): off / on / on with
///                           the row read back per DC conversion; re-arms
///   DBG:ACDC:TRACE? [from]  n,held,trig,on then ";" + 56 hex digits per entry
///                           (oldest = 0, from `from`); frozen 32 events after
///                           an accepted DC part > 20 % off the one before
///   DBG:ACDC:RANG <item>|AUTO  the MF range item (V 0 = 600 mV, 1..4 = 6 ..
///                           1000 V; A 0..5 = 600 uA .. 10 A; manual) / AUTO+
///   DBG:ACDC?               AC+DC phase diagnostic (Dmm::acdc_diag_text): phases
///                           started, then per recent AC phase (oldest first)
///                           ";late_ms:sq/lpf/dcp,..."; read-only
///   DBG:DRAG x0,y0,x1,y1,ms[,hold[,jitter]]
///                           synthetic one-finger drag through the gesture
///                           tracker (axis lock, capture): down at x0,y0, held
///                           still hold ms, moved to x1,y1 in ms ms (0 = 300),
///                           released; runs synchronously; jitter px: a
///                           sideways sawtooth on the path (+j, +j/2, 0 per 3
///                           points: a jump out, a slow drift back)
///   DBG:HWTRIG?             comparator trigger (hwtrig.rs): TIM2 SR,CNT,CCR1,
///                           PA15<<16|DOR2,enabled,available,map rising lo,hi,
///                           map falling lo,hi,post_ticks (available = enabled,
///                           not suspended by 3 misses, map present; a suspension
///                           ends at the next range / timebase / coupling / edge
///                           / mode change)
///   DBG:HWTRIG:ENABLE 0|1   use the comparator trigger (scope default on,
///                           scope.rs Scope::new; 1 resets the miss count and
///                           a suspension)
///   DBG:HWTRIG:CAL?         wire-free calibration (scope rail on, MCX open):
///                           flip00,flip01,flip10,flip11,map0lo,map0hi,map1lo,
///                           map1hi,hyst,slope,code0,code1 or the error name
///   DBG:HWTRIG:STAT?        hits,misses,delta_last,delta_min,delta_max,mean
///                           (samples, refined crossing - hardware estimate)
///   DBG:HWTRIG:STAT:RESET   clear the statistics
///   DBG:LCD? x,y,w,h,n      n raw GRAM words after RAMRD (#4 block)
///   DBG:TE?                 panel TE probe (lcd.rs te_probe, ~0.3 s)
///   DBG:CHG?                charger pins: PB10,PB11,mode,pud,omode,octl,usb,adc16
///   DBG:CHG:PIN 10|11,<m>   m IN UP DOWN LOW HIGH OD DEF; DBG:CHG:OUT <m> = PB10
///   DBG:CHG:LOG?            n,ms:pins,... the last 8 charge pin changes
///   DBG:DIAG?               diagnostic block (#3); byte 61: bit 0 PB10, bit 1
///                           PB11 (charge status pins), bit 2 USB configured
///   DBG:TOUCH? | DBG:TOUCH 0  touch log (frames, events, gestures; touch.rs log_text) / clear
///                           (feature dbg-touch, else undefined header)
///   DBG:TOUCH:TEST ON|OFF / ?  the touch test page (app_touchtest.rs) / 1 while shown
///   DBG:TOUCH:CAL? | RESET | a,b,c,d,e,f  the touch calibration x' = a x +
///                           b y + c, y' = d x + e y + f (touchcal.rs; RESET =
///                           none, 1,0,0,0,1,0; a set outside the limits is
///                           "Data out of range")
///   DBG:PROF?, DBG:PROF:RESET main-loop profile table (src/prof.rs) / clear it
///                           (feature dbg-prof, else undefined header)
///   DBG:HYSPI:SELF? n[,div[,cpha]]  HY3131 bus self-test (src/hyspi.rs; cpha 1 =
///                           the data phase sampled with CKPH = 1)
///   DBG:HYSPI:WR? mode[,div] SPI2 write experiment (src/hyspi.rs write_test):
///                           mode 0 hy3131::write, 1 SPI2 one byte per frame + a cut
///                           dummy, 2 SPI2 continuous + a cut dummy, 3 SPI2 as
///                           0d0fd76 (byte by byte, no dummy)
///   DBG:ISR?                AD2 stream ISR (logger.rs `isr`): feature,active,ticks,
///                           captured,late,probes,dropped,bad,timeouts,overruns,
///                           max_isr_ns,max_lat_ns,rate_mhz,forced,locked,ppm_x10,
///                           slips,max_run,edges_after_b,div,cpu_permille,lat_pc,dma
///                           (feature 0: no ISR in the build; timeouts = all
///                           HY3131 frames since boot; rate_mhz = the locked
///                           conversion rate in MCU time, ppm_x10 = its period vs
///                           the nominal rung, signed; lat_pc = the PC (0x hex)
///                           the worst-latency entry interrupted; AD1 sessions
///                           unchopped at 160 .. 640 S/s run on it too; in a main-loop
///                           AD1 session `late` = samples read blind after an
///                           AD1F lost to the INTF read-clear race)
///   DBG:FLASH?              SPI disk: jedec_id_hex,blocks,failed, then the last
///                           screenshot (screenshot.rs `stats`): frozen_ms,
///                           total_ms,slices_frozen,blocks_erased, then the
///                           idle-time preparation (`prep_stats`): state (0
///                           waiting, 1 working, 2 ready, 3 off),
///                           clusters_ready,erases_since_boot
///   DBG:RELEASE             stop feeding the watchdog: the WDT reset (~20 s) counts
///                           as a crash (DBG:BOOT? crash word); opendm40 reboots
///   DBG:RESET               reboot opendm40 (SW reset)
///   DBG:UPDATE              DESTRUCTIVE: clear the previous-mode word, zero the 12-byte loader
///                           marker, reset -> vendor update screen + its USB disk until
///                           a dm40*.bin is installed. "Flash error" = marker did not
///                           read back as zero (no reset). Sync + eject a copied file first.
///   DBG:LOADER, DBG:RECOVER reset into the vendor update screen through PA2 (boot.rs
///                           loader_pa2): marker untouched; a staged dm40*.bin is
///                           installed; a power cycle boots opendm40 again
///   DBG:BOOT?               boot info (src/boot.rs put_info)
#[derive(Clone, Copy)]
pub enum Dbg {
    Ping(u32),
    Input(u8, u16, u16),
    /// x0, y0, x1, y1, ms, hold ms, jitter px.
    Drag([u16; 7]),
    Lcd(u16, u16, u16, u16, u32),
    #[cfg_attr(not(feature = "dbg-diag"), allow(dead_code))]
    Diag,
    /// DBG:TOUCH? (false) / DBG:TOUCH 0 (true = clear): touch log
    /// (feature `dbg-touch`).
    #[cfg_attr(not(feature = "dbg-touch"), allow(dead_code))]
    Touch(bool),
    Flash,
    Release,
    Reset,
    /// DBG:PROF? (false) / DBG:PROF:RESET (true) (feature `dbg-prof`).
    #[cfg_attr(not(feature = "dbg-prof"), allow(dead_code))]
    Prof(bool),
    /// DBG:AD2:TORN? r26,n[,SYNC] / NOISE? r26,n[,op1chop] / DRAW 0|1|2 /
    /// DIV 0|8|16|32[,cpha] (the stream ISR's read divider, 0 = automatic:
    /// /8 + CKPH = 1 data phase for CPU frames, /16 for DMA frames) / DMA 0|1
    /// (stream frames by the CPU / by DMA, the default: decision 2026-10-22 #7;
    /// DMA? = Ad2(4, 255): the setting, 0|1)
    /// (src/ad2exp.rs): 0 torn (a = n, b = sync), 1 noise (b = op1chop, 255
    /// = the row's), 2 draw (r26 = level), 5 DCV? range,r26,n[,flags] (the
    /// DC-coupled V row, device test 6.5: n | range << 8, flags).
    #[cfg_attr(not(feature = "ad2-exp"), allow(dead_code))]
    Ad2(u8, u8, u16, u8),
    /// DBG:HYSPI:SELF? n[,div[,cpha]] (src/hyspi.rs): (n, div | cpha << 7;
    /// div 0 = hy3131::DIV).
    #[cfg_attr(not(feature = "dbg-hyspi"), allow(dead_code))]
    HySpi(u16, u8),
    /// DBG:HYSPI:WR? mode[,div] (src/hyspi.rs `write_test`).
    #[cfg_attr(not(feature = "dbg-hyspi"), allow(dead_code))]
    HySpiWr(u8, u8),
    /// DBG:ISR? (logger.rs `isr`).
    Isr,
    /// DBG:FAST?: FR-05 FAST's offset refreshes used, rejected, the VOS
    /// (AD1 codes) of the current path and the AD1 conversion period in ms
    /// (dmm.rs `fast_info`).
    Fast,
    /// Firmware update file upload (upload.rs): 0 BEGin size,crc32 | 1
    /// END? | 2 ABORt | 3 DBG:UPLoad? (DATA bypasses the parser, main.rs).
    Upload(u8, u32, u32),
    /// DBG:IDLE <ms> (Some: the idle shutdown time for a test, 0 = the
    /// setting) / DBG:IDLE? (None: idle times, App::dbg_idle).
    Idle(Option<u32>),
    /// DBG:OHM6 gain,off_uV,skip,keep,leak_nS (Some; a missing field = NaN, kept) /
    /// DBG:OHM6? (None): the 600 Mohm range's RAM-only tuning
    /// (dmm::Ohm600, Dmm::ohm600_dbg).
    #[cfg_attr(not(feature = "dbg-ohm6"), allow(dead_code))]
    Ohm6(Option<[f32; 5]>),
}

/// DBG:AD2:DCV? range,r26,n[,flags] -> Dbg::Ad2(5, r26, n | range << 8,
/// flags) (main.rs `dbg_dcv`).
#[cfg(feature = "ad2-exp")]
#[inline(never)] // .cold (link-native.x)
fn dcv_arg(a: &[u8]) -> Dbg {
    let w = nums::<4>(a);
    Dbg::Ad2(5, (w[1] & 0x3F) as u8, (w[2].clamp(1, 16) | w[0].min(3) << 8) as u16, (w[3] & 0x0F) as u8)
}

/// WAVeform:<sub> (scope_wav.rs): Ok(None) = WAV:SOURce (CHANnel1 only;
/// the query's reply is put by the caller).
#[inline(never)] // region B: polled while the Wave view streams
fn wav_cmd(sub: &[u8], a: &[u8], query: bool) -> Result<Option<Cmd>, &'static str> {
    const BAD: &str = "Illegal parameter value";
    if kw(sub, "PREamble") && query {
        Ok(Some(Cmd::WavPre))
    } else if (kw(sub, "DATA") || sub.is_empty()) && query {
        Ok(Some(Cmd::WavData))
    } else if kw(sub, "POINts") {
        if query {
            return Ok(Some(Cmd::WavPoints(None)));
        }
        let n = if kw(a, "MAXimum") || kw(a, "RAW") {
            u32::MAX
        } else if kw(a, "SCReen") || kw(a, "NORMal") || kw(a, "DEFault") {
            0
        } else {
            match parse_num(a) {
                Some(v) if v >= 1.0 => v.min(u32::MAX as f32 - 1.0) as u32,
                _ => return Err(BAD),
            }
        };
        Ok(Some(Cmd::WavPoints(Some(n))))
    } else if kw(sub, "FORMat") {
        if query {
            Ok(Some(Cmd::WavFormat(None)))
        } else if kw(a, "WORD") {
            Ok(Some(Cmd::WavFormat(Some(true))))
        } else if kw(a, "BYTE") {
            Ok(Some(Cmd::WavFormat(Some(false))))
        } else {
            Err(BAD)
        }
    } else if kw(sub, "SOURce") {
        if query || kw(a.strip_suffix(b"1").unwrap_or(&[]), "CHANnel") {
            Ok(None)
        } else {
            Err(BAD)
        }
    } else {
        Err("Undefined header")
    }
}

/// DBG:AD2:N7? item,state,n -> Dbg::Ad2(6, state, n, item) (main.rs).
#[cfg(feature = "ad2-exp")]
#[inline(never)] // .cold (link-native.x)
fn n7_arg(a: &[u8]) -> Dbg {
    let w = nums::<3>(a);
    Dbg::Ad2(6, w[1].min(2) as u8, w[2].clamp(1, 40) as u16, w[0].min(4) as u8)
}

/// DBG:TOUCH:CAL a,b,c,d,e,f: six numbers into touchcal.rs (false: not
/// six numbers).
#[inline(never)] // .cold (link-native.x)
fn tcal_arg(a: &[u8]) -> bool {
    let mut v = [0f32; 6];
    let mut n = 0;
    for part in a.split(|&b| b == b',') {
        match parse_num(part) {
            Some(x) if n < 6 => v[n] = x,
            _ => return false,
        }
        n += 1;
    }
    n == 6 && crate::touchcal::scpi_put(&v)
}

/// LOG:CSV:INTerval / DURation / CHANge [?], LOG:CSV:FREE? (the Record
/// page's settings): the command, or the error. Values: exactly one of
/// csvlog::INTERVALS_S (or CHANge) / DURATIONS_S (s) / CHANGES (%); another
/// number is -222, anything else -224.
#[inline(never)] // .cold (link-native.x)
fn csv_arg(s2: &[u8], a: &[u8], query: bool) -> Result<Log, &'static str> {
    use crate::csvlog::{CHANGES, DURATIONS_S, INTERVALS_S, ON_CHANGE};
    const BAD: &str = "Illegal parameter value";
    if kw(s2, "FREE") && query {
        return Ok(Log::CsvFree);
    }
    let n = parse_num(a);
    let pick = |i: Option<usize>| match (query, i) {
        (true, _) => Ok(None),
        (false, Some(i)) => Ok(Some(i as u8)),
        (false, None) => Err(if n.is_some() { RANGE } else { BAD }),
    };
    let secs = |set: &[u32]| n.and_then(|v| set.iter().position(|&x| x as f32 == v));
    if kw(s2, "INTerval") {
        let i = if kw(a, "CHANge") { Some(ON_CHANGE as usize) } else { secs(&INTERVALS_S) };
        Ok(Log::CsvInt(pick(i)?))
    } else if kw(s2, "DURation") {
        Ok(Log::CsvDur(pick(secs(&DURATIONS_S))?))
    } else if kw(s2, "CHANge") {
        Ok(Log::CsvChg(pick(n.and_then(|v| CHANGES.iter().position(|c| parse_num(c.as_bytes()) == Some(v))))?))
    } else {
        Err("Undefined header")
    }
}

fn nums<const N: usize>(a: &[u8]) -> [u32; N] {
    let mut out = [0u32; N];
    for (i, part) in a.split(|&b| b == b',').take(N).enumerate() {
        out[i] = parse_num(part).unwrap_or(0.0) as u32;
    }
    out
}

/// Scope / generator commands (handled by the main loop with the app):
///   SOURce:FUNCtion SIN|SQU|TRI|RAMP   SOURce:FREQuency <Hz>
///   SOURce:VOLTage <Vpp>   SOURce:VOLTage:OFFSet <V>|AUTO (FR-28: centre level;
///   AUTO = stock 0..Vpp; kept in [Vpp/2, 3 - Vpp/2])
///   SOURce:FUNCtion:SQUare:DCYCle <%>   OUTPut ON|OFF
///   Queries: FREQ? = the actual (clamped + quantised) frequency, DCYC? the
///   effective duty, VOLT? / OFFS? the set values (OFFS? with AUTO: Vpp/2)
///   TIMebase[:MAIN]:SCALe <s/div>   CHANnel1:SCALe <V/div>   CHANnel1:COUPling AC|DC
///   TRIGger[:EDGE]:LEVel <V>   TRIGger[:EDGE]:SLOPe POS|NEG   TRIGger:SWEep AUTO|NORM|SING
///   TRIGger[:EDGE]:SOURce CHANnel1|GENerator (GEN: the generator's output
///   computed from its timers, docs/scope-gen-channel.md)
///   CHANnel1:OFFSet <V> (the vertical position as a voltage added to the
///   signal: positive moves the trace up)   ACQuire:SRATe? (S/s; stopped:
///   the stored record's rate)
///   RUN  STOP  SINGle   DIGitize (one acquisition with the current sweep,
///   then STOP)   TRIGger:STATus? (STOP|WAIT|AUTO|TD|RUN)
///   SCOPe:MEASure:VPP|VAVG|VRMS|FREQ?
///   WAVeform:POINts SCReen|MAXimum|<n> (default SCReen: 400 screen columns;
///   MAX: the whole acquisition record; n: the record decimated to at most n
///   points)   WAVeform:FORMat BYTE|WORD (code >> 2 | the 10-bit ADC code as
///   16-bit little endian)   WAVeform:SOURce CHANnel1
///   WAVeform:DATA? (SCReen + BYTE: #3400 + 400 bytes, as before; else a
///   block from src/scope_wav.rs)   WAVeform:PREamble? (Keysight / Rigol
///   layout: format,type,points,count,xinc,xorig,xref,yinc,yorig,yref)
///   Volts (CHAN1:SCAL, TRIG:LEV, measurements, WAV:PRE) are at the probe
///   tip: the probe factor of the scope settings window (x0.1..x1000) is
///   applied; there is no SCPI probe command.
///   A set with no value is -109, with a value that is no number / keyword
///   of the command -224, with a number outside its domain (FREQ, SCAL,
///   VOLT, TIM below 0 or 0, DCYC outside 0..100) -222; the setting stays.
///   Numbers take the suffixes of `parse_num_u` (e.g. FREQ 1KHZ, 2.5MHZ;
///   TIM 100US; VOLT 500MV).
#[derive(Clone, Copy)]
pub enum Cmd {
    GenFunc(Option<u8>),
    GenFreq(Option<f32>),
    GenVolt(Option<f32>),
    /// SOUR:VOLT:OFFS: Some(NaN) = AUTO.
    GenOffs(Option<f32>),
    GenDuty(Option<f32>),
    GenOut(Option<bool>),
    TimScale(Option<f32>),
    ChScale(Option<f32>),
    ChCoup(Option<bool>),
    TrigLevel(Option<f32>),
    TrigSlope(Option<bool>),
    TrigSweep(Option<u8>),
    /// TRIG:SOURce: Some(scope::TSRC_*); None = ?
    TrigSource(Option<u8>),
    /// DBG:GENTRIG:LAG <ns>: the GEN channel's lag (scope::set_gen_lag).
    DbgGenLag(Option<f32>),
    /// DBG:GENTRIG:STAT?: the generator phase readings (gen::stats).
    DbgGenStat,
    /// DBG:CH2:LAG <us>: the DMM CH2's conversion lag (scope_ch2).
    DbgCh2Lag(Option<f32>),
    Run,
    Stop,
    Single,
    Meas(u8),
    WavData,
    WavPre,
    /// WAV:POINts: Some(0) SCReen, Some(u32::MAX) MAXimum, Some(n); None = ?
    WavPoints(Option<u32>),
    /// WAV:FORMat: Some(true) WORD, Some(false) BYTE; None = ?
    WavFormat(Option<bool>),
    TrigStatus,
    Digitize,
    ChOffs(Option<f32>),
    AcqSrate,
    /// Debug: sample the DAC output pin PA4 with the MCU ADC.
    DiagDac,
    /// Debug: count PB14 (PWM output) rising edges for 2 ms.
    DiagPwm,
    /// DMM / system commands (not scope commands, see App::scpi).
    TempSensor(Option<u8>),
    TempOffs(Option<f32>),
    TempNtc(Option<(f32, f32)>),
    TempExc,
    TempCj,
    /// UNIT:TEMPerature C|F (true = F) / ?
    TempUnit(Option<bool>),
    SysTemp,
    /// CONFigure?: "<func> <range>,<resolution>" as the Agilent / HP meters
    /// (App::scpi_dmm; "CONT" / "DIOD" bare).
    ConfQuery,
    SysRail(Option<bool>),
    SysRailState,
    /// CALCulate (FR-01 / 02 / 06; App::set_ahold etc.).
    CalcAhold(Option<bool>),
    /// 0 MIN, 1 MAX, 2 AVER, 3 COUNt, 4 SDEViation.
    CalcAver(u8),
    CalcAverClear,
    CalcAverState(Option<bool>),
    /// 0 LOWer, 1 UPPer; None = query.
    CalcLim(u8, Option<f32>),
    CalcLimClear,
    CalcLimState(Option<bool>),
    CalcLimFail,
    /// FR-05: [SENSe:]RATE (0 SLOW, 1 MED, 2 FAST; None = query),
    /// RATE:PERiod?, CALC:SMOothing[:STATe], CALC:SMOothing:RESPonse
    /// (1 LOW = FAST / MED, 2 HIGH = SLOW).
    SensRate(Option<u8>),
    SensRatePer,
    /// [SENSe:]VOLTage:COUPling? (false) / CURRent:COUPling? (true).
    Coup(bool),
    CalcSmoState(Option<bool>),
    CalcSmoResp(Option<u8>),
    /// DBG:GRAB <ms> / ?: hold-to-grab time of scope drags (0 = direct).
    DbgGrab(Option<u16>),
    /// DBG:PINCH 0|1|2 / ?: scope pinch mode (scope_page::set_pinch_mode).
    DbgPinch(Option<u8>),
    /// DBG:GEST?: log of the last scope drags.
    DbgGest,
    /// DBG:VIEW?: the page shown (App::scpi).
    DbgView,
    /// DBG:TOUCH:TEST ON|OFF (Some) / ? (None): the touch test page
    /// (app_touchtest.rs).
    DbgTouchTest(Option<bool>),
    /// DBG:TOUCH:CAL? (0) / RESET (1) / a,b,c,d,e,f (2, the values in
    /// touchcal.rs `scpi_take`): the touch calibration (app_touchtest.rs).
    DbgTouchCal(u8),
    /// DBG:READout?: the DMM readout as drawn (App::scpi).
    DbgReadout,
    /// DBG:ACDC?: AC+DC phase diagnostic (Dmm::acdc_diag_text).
    DbgAcdc,
    /// DBG:ACDC:METH / STAT / ZERO / R0 (acdcm.rs; feature `acdc-meth`): op 0 METH <m>, 1 METH?,
    /// 2 STAT?, 3 ZERO <slot>,<v> (NaN: measure again), 4 ZERO?, 5 R0 <1|2>,
    /// 6 R0?, 7 RANG <item> (255 = AUTO+), 8 TRACE? <from>, 9 TRACE <0|1|2>.
    DbgAm(u8, u8, f32),
    /// SYSTem:ALERt?: the wrong-connection alert in force (sentinel.rs).
    SysAlert,
    /// STATus:QUEStionable:CONDition? (sentinel::ques_bits of the alert).
    StatQuesCond,
    /// SYSTem:FUSE:TEST (true) / SYSTem:FUSE? (false).
    SysFuse(bool),
    /// SYSTem:ALERt:ENABle <mask> | ON | OFF / ?: alerts enabled (bit 0
    /// A-JACK, 1 V-JACK?, 2 LIVE?; bit 3 ignored).
    SysAlertEnable(Option<u8>),
    /// SYSTem:ALERt:ALARm LOUD | VOLume | OFF / ?: the danger alarm setting
    /// (danger::LOUD / VOLUME / OFF).
    SysAlarm(Option<u8>),
    /// SYSTem:BEEPer:VOLume <0..100> / ?: the key / alert volume (%).
    SysBeepVol(Option<u8>),
    /// DBG:ALARm?: the danger alarm's state.
    DbgAlarm,
    /// DBG:SENTinel?: sentinel raw state (Sentinel::diag_text).
    #[cfg_attr(not(feature = "dbg-sent"), allow(dead_code))]
    DbgSent,
    /// DBG:SENTinel:LOG?: the alert assertions (Sentinel::log).
    DbgSentLog,
    /// DBG:SENTinel:VARiant <0..127> / ?: device-check row variants
    /// (sentinel::VAR_*, RAM only).
    DbgSentVar(Option<u8>),
    /// DBG:SENTinel:REG?: R20..R37 read back + relay / mode (Dmm::reg_text).
    #[cfg_attr(not(feature = "dbg-sent"), allow(dead_code))]
    DbgSentReg,
    /// DBG:SRC <vref>,<path>[,<sense>][,FORCE] | OFF (Some) / DBG:SRC? (None):
    /// the ohm-source map row (src/dbgsrc.rs).
    #[cfg_attr(not(feature = "dbg-src"), allow(dead_code))]
    DbgSrc(Option<crate::dbgsrc::Req>),
    /// DBG:SENTinel:PRECond <ms> / ?: the DC A precondition experiment
    /// (Dmm::precond_ms, RAM only, 0 = off).
    DbgSentPrec(Option<u16>),
    /// DBG:HWTRIG...: 0 query, 1 ENABLE (bool), 2 CAL?, 3 STAT?, 4 STAT:RESET.
    DbgHwTrig(u8, bool),
    /// Power meter (dmm::pw): CONF:POW, FETC:POW:ALL?, [SENS:]POW:..., DBG:POW:...
    Pow(Pow),
    /// FETCh:BURDen?: burden voltage of the latest A-function reading (doc 3.15).
    FetchBurden,
    /// DISPlay:BURDen ON|OFF / ?: the burden voltage readout (Settings 3/3).
    DispBurden(Option<bool>),
    /// [SENSe:]RESistance:NEGative ON|OFF / ?: ohm readings below 0
    /// (Settings 3/3 "Negative ohm", Dmm::ohm_neg).
    ResNeg(Option<bool>),
    /// MEMory:STATe:RECall:AUTO ON|OFF / ?: Settings 3/3 "Mode Remember".
    ModeRemember(Option<bool>),
    /// [SENSe:]<f>:RANGe[:AUTO] / <f>:RESolution (`range_cmd`): the
    /// function the header names (`parse_func`) and the operation
    /// (App::scpi_range).
    Range(u8, Rng),
    /// CONFigure:<f> [<range>|AUTO|MIN|MAX|DEF[,<res>|MIN|MAX|DEF]]
    /// (`conf_cmd`): select the function, then the range (Rng::Auto without
    /// one) and, when given, the resolution (Rng::Res's value).
    Conf(u8, Rng, Option<f32>),
}

/// Range / resolution operation of a DMM function (Cmd::Range / Conf).
/// One range state exists (the current function's): a header naming
/// another function is "Settings conflict" (App::scpi_range).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Rng {
    /// <f>:RANGe? (the present range, also while autoranging)
    Query,
    /// <f>:RANGe:AUTO? (1 = autorange, 0 = a fixed range: manual or REL)
    AutoQuery,
    /// RANG:AUTO ON, RANG AUTO|DEF, CONF:<f> [AUTO|DEF]: back to the
    /// meter's autorange, the flavour chosen on the meter (V / A: AUTO+ or
    /// the mV- / uA-first AUTO; Dmm::auto_key).
    Auto,
    /// RANG:AUTO OFF|ONCE: keep the present range (as REL does).
    Hold,
    /// RANG <v> | MIN (0) | MAX (inf): the smallest range that holds v
    /// (34401A rule; the MF page's items).
    Value(f32),
    /// <f>:RESolution?
    ResQuery,
    /// RES <v> | MIN (0) | MAX (inf) | DEF (NaN): FAST (60000 counts) when
    /// v is at least the range's 60000-count step, else MED (the +1 digit
    /// where the range has it; SLOW is kept).
    Res(f32),
}

impl Rng {
    pub fn is_query(self) -> bool {
        matches!(self, Rng::Query | Rng::AutoQuery | Rng::ResQuery)
    }
}

/// Range value (SI) of a stock mode / range index, the full scale the
/// meter labels it with (60000-count ranges 6 / 60 / 600..., 1000 V, 10 A,
/// 60 mV = DC / AC mV r0).
/// `r` of frequency (14) is the counter range (Dmm::counter_range). None: no
/// range (continuity, diode, temperature).
pub fn range_si(mode: u8, r: i8) -> Option<f32> {
    let r = r.max(0) as usize;
    const P10: [f32; 8] = [1.0, 10.0, 100.0, 1e3, 1e4, 1e5, 1e6, 1e7];
    let p = P10[r.min(7)];
    Some(match mode {
        0 | 1 if r >= 3 => 1000.0,
        0 | 1 => 6.0 * p,
        2 | 3 => [0.06, 0.6][r.min(1)],
        4..=9 => [[6.0, 10.0], [0.06, 0.6], [6e-4, 6e-3]][((mode - 4) % 3) as usize][r.min(1)],
        10 => 60.0 * p,
        13 => 6e-9 * p,
        14 => 60.0 * p,
        _ => return None,
    })
}

/// Decimal exponent of the 60000-count step of range `v` (the 6000-count
/// step for capacitance, `cap`): floor(log10 v) - 4 (6 V -> -4, 1000 V ->
/// -1, 10 A -> -3, 60 ohm -> -3, 6 nF -> -12), as the readout's decimals.
pub fn step_exp(v: f32, cap: bool) -> i32 {
    let mut e = -4 + cap as i32;
    let mut x = v * 1.0001;
    while x >= 10.0 {
        x /= 10.0;
        e += 1;
    }
    while x < 1.0 && e > -40 {
        x *= 10.0;
        e -= 1;
    }
    e
}

/// 10^e, the f32 nearest it for e in -16..=8 (a table: VOLT:RES? prints
/// it as nr3, and an f32 divided by 10 five times gave +9.999999E-06,
/// device 1a11eda). Outside the table: repeated steps (not used).
pub fn pow10f(e: i32) -> f32 {
    const T: [f32; 25] = [
        1e-16, 1e-15, 1e-14, 1e-13, 1e-12, 1e-11, 1e-10, 1e-9, 1e-8, 1e-7, 1e-6, 1e-5, 1e-4, 1e-3, 1e-2, 1e-1, 1.0,
        1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8,
    ];
    if let Some(&v) = usize::try_from(e + 16).ok().and_then(|i| T.get(i)) {
        return v;
    }
    let mut v = 1.0f32;
    for _ in 0..e.unsigned_abs() {
        v = if e < 0 { v / 10.0 } else { v * 10.0 };
    }
    v
}

/// The settable ranges, ascending (the MF page's items, Dmm::set_range
/// through `range_item_code`): V 60 mV .. 1000 V, A 600 uA .. 10 A, ohm
/// 60 ohm .. 60 Mohm and the experimental 600 Mohm.
pub fn range_items(func: u8) -> &'static [f32] {
    match func {
        0..=3 | 16 | crate::dmm::FUNC_V_AUTO => &[0.06, 0.6, 6.0, 60.0, 600.0, 1000.0],
        4..=9 | 17 | crate::dmm::FUNC_A_AUTO => &[6e-4, 6e-3, 0.06, 0.6, 6.0, 10.0],
        // 600 Mohm: the experimental r7 (dmm::OHM600); MAX stays 60 Mohm.
        10 => &[60.0, 600.0, 6e3, 60e3, 600e3, 6e6, 60e6, 600e6],
        _ => &[],
    }
}

/// The Dmm::set_range item of `range_items(func)[i]`: V 60 mV is item 5
/// (after 600 mV .. 1000 V = 0..4); the other lists are the items in order.
pub fn range_item_code(func: u8, i: usize) -> u8 {
    if quantity(func & !ANY_CPL) == 1 {
        if i == 0 { 5 } else { i as u8 - 1 }
    } else {
        i as u8
    }
}

/// The unit suffix a value of function `f` (a `Dmm::func` code) may carry.
fn unit_of(f: u8) -> &'static [u8] {
    match quantity(f & !ANY_CPL) {
        1 => b"V",
        8 => b"A",
        10 => b"OHM",
        13 => b"F",
        14 => b"HZ",
        _ => b"",
    }
}

/// RANG / CONF range argument, a value in `unit`.
fn range_arg(a: &[u8], unit: &[u8]) -> Option<Rng> {
    if kw(a, "AUTO") || kw(a, "DEFault") {
        Some(Rng::Auto)
    } else if kw(a, "MINimum") {
        Some(Rng::Value(0.0))
    } else if kw(a, "MAXimum") {
        Some(Rng::Value(f32::INFINITY))
    } else {
        parse_num_u(a, unit).filter(|v| *v >= 0.0).map(Rng::Value)
    }
}

/// RES / CONF resolution argument (MIN 0, MAX inf, DEF NaN), in `unit`.
fn res_arg(a: &[u8], unit: &[u8]) -> Option<f32> {
    if kw(a, "DEFault") {
        Some(f32::NAN)
    } else if kw(a, "MINimum") {
        Some(0.0)
    } else if kw(a, "MAXimum") {
        Some(f32::INFINITY)
    } else {
        parse_num_u(a, unit).filter(|v| *v > 0.0)
    }
}

/// [SENSe:]<f>:RANGe <v>|AUTO|MIN|MAX|DEF / ?, [SENSe:]<f>:RANGe:AUTO
/// ON|OFF|ONCE / ?, [SENSe:]<f>:RESolution <v>|MIN|MAX|DEF / ? with <f> as
/// CONFigure's (VOLTage[:DC] / :AC / :ACDC / :AUTO, CURRent..., RESistance,
/// CAPacitance, FREQuency...). None: not such a header.
#[inline(never)] // .cold (link-native.x)
fn range_cmd(parts: &[&[u8]], arg: &[u8], query: bool) -> Option<Result<Cmd, &'static str>> {
    let r = if parts.first().is_some_and(|p| kw(p, "SENSe")) { &parts[1..] } else { parts };
    // From 1: "RES" is also RESistance's short form.
    let k = 1 + r.iter().skip(1).position(|p| kw(p, "RANGe") || kw(p, "RESolution"))?;
    let f = parse_func(&r[..k])?;
    let res = kw(r[k], "RESolution");
    let auto = match r.len() - k {
        1 => false,
        2 if !res && kw(r[k + 1], "AUTO") => true,
        _ => return None,
    };
    if k > 2 || f == crate::dmm::pw::FUNC {
        return None;
    }
    let a = trim(arg);
    const BAD: &str = "Illegal parameter value";
    let op = match (res, auto, query) {
        (false, false, true) => Some(Rng::Query),
        (false, true, true) => Some(Rng::AutoQuery),
        (true, _, true) => Some(Rng::ResQuery),
        (false, true, false) if kw(a, "ONCE") => Some(Rng::Hold),
        (false, true, false) => on_off(a).map(|on| if on { Rng::Auto } else { Rng::Hold }),
        (false, false, false) => range_arg(a, unit_of(f)),
        (true, _, false) => res_arg(a, unit_of(f)).map(Rng::Res),
    };
    // A bare VOLTage / CURRent: the quantity in any coupling (ANY_CPL).
    let f = if k == 1 && matches!(f, 1 | 8) { f | ANY_CPL } else { f };
    Some(op.map(|o| Cmd::Range(f, o)).ok_or(BAD))
}

/// Cmd::Range's function flag: a bare VOLTage / CURRent header (no :DC /
/// :AC / :ACDC / :AUTO), i.e. that quantity whatever the coupling (one
/// range state per V / A button; App::scpi_range).
pub const ANY_CPL: u8 = 0x80;

/// The quantity of a `Dmm::func` code for ANY_CPL: V in any coupling 1, A
/// 8, other functions as they are.
pub fn quantity(f: u8) -> u8 {
    match f {
        0..=3 | 16 | crate::dmm::FUNC_V_AUTO => 1,
        4..=9 | 17 | crate::dmm::FUNC_A_AUTO => 8,
        m => m,
    }
}

/// CONFigure:<f> <range>[,<res>] (`f` not temperature / power): Cmd::Conf.
/// Continuity and diode take no argument; capacitance and frequency
/// autorange only (AUTO / DEF).
#[inline(never)] // .cold (link-native.x)
fn conf_cmd(f: u8, arg: &[u8]) -> Result<Cmd, &'static str> {
    const BAD: &str = "Illegal parameter value";
    let a = trim(arg);
    let mut it = a.splitn(2, |&b| b == b',').map(trim);
    let ra = it.next().unwrap_or(&[]);
    let rs = it.next();
    if matches!(f, 11 | 12) && !a.is_empty() {
        return Err(BAD);
    }
    let rng = if ra.is_empty() { Rng::Auto } else { range_arg(ra, unit_of(f)).ok_or(BAD)? };
    let res = match rs {
        Some(s) => Some(res_arg(s, unit_of(f)).ok_or(BAD)?),
        None => None,
    };
    if matches!(f, 13 | 14) && (rng != Rng::Auto || res.is_some_and(|v| !v.is_nan())) {
        return Err(BAD);
    }
    Ok(Cmd::Conf(f, rng, res))
}

/// ON | 1 -> true, OFF | 0 -> false.
/// Power-meter commands (`parse_pow`). Ranges: None = query, Some(None) =
/// AUTO, Some(Some(r)) = range index (V 0..3, I 0..5).
#[derive(Clone, Copy)]
pub enum Pow {
    /// CONFigure:POWer [<V>|AUTO[,<A>|AUTO]].
    Conf(Option<u8>, Option<u8>),
    /// FETCh:POWer:ALL?
    All,
    /// MEASure:POWer:APParent? (0) / :PFACtor? (1) / :ENERgy? (2) /
    /// :REACtive? (3) / :VOLTage? (4) / :CURRent? (5): the latest value.
    Get(u8),
    VRange(Option<Option<u8>>),
    IRange(Option<Option<u8>>),
    EnergyReset,
    EnergyState(Option<bool>),
    /// :BURDen <ohm>|DEFault (NaN) / ?, :BURDen:STATe ON|OFF / ? ("At
    /// load", default OFF, doc 3.16)
    Burden(Option<f32>),
    BurdenState(Option<bool>),
    Phase(Option<i8>),
    /// Some(true) REACtive, Some(false) RESistive (refused), None = the query.
    PhaseCal(Option<bool>),
    Cycle,
    Method(Option<bool>),
    /// :LIMit <A>,<s> / ? (continuous current, countdown; s 0 = none).
    Limit(Option<(f32, u16)>),
    /// :COUPling DC|AC|ACDC / ? (pw::COUP_*; AUTO = ACDC, deprecated).
    Coupling(Option<u8>),
    /// :ZERO (0: start the self-zero, leads open), :ZERO CLEar (1), :ZERO? (2).
    Zero(u8),
    /// :VOLTage:CALibrate (0: start the V self-cal on the current V range),
    /// :VOLTage:CALibrate DEFault (1: the factory-derived gain again),
    /// :VOLTage:CALibrate? (2). :VOLTage:GAIN? (3): the 4 gains in force.
    VCal(u8),
    DbgRow,
    DbgPoke(u8, u8),
    DbgStat(u8),
    DbgRaw,
    DbgDisc(Option<u8>),
}

/// The message is a power-meter command (`parse_pow`).
fn is_pow(parts: &[&[u8]], query: bool) -> bool {
    let p0 = parts[0];
    let p1 = parts.get(1).is_some_and(|p| kw(p, "POWer"));
    kw(p0, "POWer")
        || (p1 && (kw(p0, "SENSe") || p0 == b"DBG"))
        || (p1 && kw(p0, "CONFigure") && !query)
        || (p1 && kw(p0, "FETCh") && query)
        || (p1 && kw(p0, "MEASure") && query && parts.len() > 2)
}

/// V range index for a value in V (the smallest that holds it).
fn pow_vrange(a: &[u8]) -> Option<Option<u8>> {
    if kw(a, "AUTO") {
        return Some(None);
    }
    let v = parse_num_u(a, b"V")?;
    crate::dmm::pw::V_LIM.iter().position(|&l| v <= l * 1.0001).map(|k| Some(k as u8))
}

/// I range index for a value in A.
fn pow_irange(a: &[u8]) -> Option<Option<u8>> {
    if kw(a, "AUTO") {
        return Some(None);
    }
    let v = parse_num_u(a, b"A")?;
    crate::dmm::pw::I_LIM.iter().position(|&l| v <= l * 1.0001).map(|k| Some(k as u8))
}

/// Parse a power-meter command (docs/power-meter-and-misconnect.md 3.9).
fn parse_pow(parts: &[&[u8]], arg: &[u8], query: bool) -> Result<Pow, &'static str> {
    const BAD: &str = "Illegal parameter value";
    const HDR: &str = "Undefined header";
    let a = trim(arg);
    let p0 = parts[0];
    let mut args = a.split(|&b| b == b',').map(trim);
    if kw(p0, "CONFigure") {
        let v = match args.next() {
            None | Some(b"") => None,
            Some(x) => Some(pow_vrange(x).ok_or(BAD)?.unwrap_or(255)),
        };
        let i = match args.next() {
            None => None,
            Some(x) => Some(pow_irange(x).ok_or(BAD)?.unwrap_or(255)),
        };
        return Ok(Pow::Conf(v, i));
    }
    if kw(p0, "FETCh") {
        return if parts.get(2).is_some_and(|p| kw(p, "ALL")) { Ok(Pow::All) } else { Err(HDR) };
    }
    if kw(p0, "MEASure") {
        let l = parts[2];
        let k = if kw(l, "APParent") {
            0
        } else if kw(l, "PFACtor") {
            1
        } else if kw(l, "ENERgy") {
            2
        } else if kw(l, "REACtive") {
            3
        } else if kw(l, "VOLTage") {
            4
        } else if kw(l, "CURRent") {
            5
        } else {
            return Err(HDR);
        };
        return Ok(Pow::Get(k));
    }
    // [SENSe:]POWer:..., DBG:POWer:...
    let dbg = p0 == b"DBG";
    let rest = if kw(p0, "POWer") { &parts[1..] } else { &parts[2..] };
    let l0 = rest.first().copied().unwrap_or(b"");
    let l1 = rest.get(1).copied();
    let state = |q: bool| if q { Ok(None) } else { on_off(a).map(Some).ok_or(BAD) };
    if dbg {
        return match l0 {
            b"ROW" if query => Ok(Pow::DbgRow),
            b"RAW" if query => Ok(Pow::DbgRaw),
            b"POKE" if !query => {
                if kw(a, "CLEar") {
                    return Ok(Pow::DbgPoke(255, 0));
                }
                let v = nums::<2>(a);
                if v[0] < 22 && v[1] < 256 { Ok(Pow::DbgPoke(v[0] as u8, v[1] as u8)) } else { Err(BAD) }
            }
            b"STAT" if !query => match a {
                b"XY" => Ok(Pow::DbgStat(crate::dmm::pw::R34_XY)),
                b"XX" => Ok(Pow::DbgStat(crate::dmm::pw::R34_XX)),
                b"YY" => Ok(Pow::DbgStat(crate::dmm::pw::R34_YY)),
                b"AUTO" => Ok(Pow::DbgStat(0)),
                _ => Err(BAD),
            },
            b"DISC" if query => Ok(Pow::DbgDisc(None)),
            b"DISC" => match parse_num(a) {
                Some(v) if (0.0..=8.0).contains(&v) => Ok(Pow::DbgDisc(Some(v as u8))),
                _ => Err(BAD),
            },
            _ => Err(HDR),
        };
    }
    if kw(l0, "VOLTage") && l1.is_some_and(|p| kw(p, "CALibrate")) {
        return if query {
            Ok(Pow::VCal(2))
        } else if a.is_empty() {
            Ok(Pow::VCal(0))
        } else if kw(a, "DEFault") {
            Ok(Pow::VCal(1))
        } else {
            Err(BAD)
        };
    }
    if kw(l0, "VOLTage") && l1.is_some_and(|p| kw(p, "GAIN")) && query {
        return Ok(Pow::VCal(3));
    }
    if kw(l0, "COUPling") && l1.is_none() {
        return if query {
            Ok(Pow::Coupling(None))
        } else if a == b"ACDC" || kw(a, "AUTO") {
            // AUTO (deprecated, doc 3.14): the former auto choice is AC+DC.
            Ok(Pow::Coupling(Some(crate::dmm::pw::COUP_ACDC)))
        } else if a == b"AC" {
            Ok(Pow::Coupling(Some(crate::dmm::pw::COUP_AC)))
        } else if a == b"DC" {
            Ok(Pow::Coupling(Some(crate::dmm::pw::COUP_DC)))
        } else {
            Err(BAD)
        };
    }
    if kw(l0, "ZERO") && l1.is_none() {
        return if query {
            Ok(Pow::Zero(2))
        } else if a.is_empty() {
            Ok(Pow::Zero(0))
        } else if kw(a, "CLEar") {
            Ok(Pow::Zero(1))
        } else {
            Err(BAD)
        };
    }
    if kw(l0, "VOLTage") && l1.is_some_and(|p| kw(p, "RANGe")) {
        return if query { Ok(Pow::VRange(None)) } else { pow_vrange(a).map(|r| Pow::VRange(Some(r))).ok_or(BAD) };
    }
    if kw(l0, "CURRent") && l1.is_some_and(|p| kw(p, "RANGe")) {
        return if query { Ok(Pow::IRange(None)) } else { pow_irange(a).map(|r| Pow::IRange(Some(r))).ok_or(BAD) };
    }
    if kw(l0, "ENERgy") {
        return match l1 {
            Some(p) if kw(p, "RESet") && !query => Ok(Pow::EnergyReset),
            Some(p) if kw(p, "STATe") => state(query).map(Pow::EnergyState),
            _ => Err(HDR),
        };
    }
    if kw(l0, "BURDen") {
        return match l1 {
            Some(p) if kw(p, "STATe") => state(query).map(Pow::BurdenState),
            None if query => Ok(Pow::Burden(None)),
            None if kw(a, "DEFault") => Ok(Pow::Burden(Some(f32::NAN))),
            None => match parse_num_u(a, b"OHM") {
                Some(v) if (0.0..=1000.0).contains(&v) => Ok(Pow::Burden(Some(v))),
                Some(_) => Err(RANGE),
                None => Err(BAD),
            },
            _ => Err(HDR),
        };
    }
    if kw(l0, "PHASe") {
        return match l1 {
            Some(p) if kw(p, "CALibrate") && query => Ok(Pow::PhaseCal(None)),
            Some(p) if kw(p, "CALibrate") => {
                if a.is_empty() || kw(a, "REACtive") {
                    Ok(Pow::PhaseCal(Some(true)))
                } else if kw(a, "RESistive") {
                    Ok(Pow::PhaseCal(Some(false)))
                } else {
                    Err(BAD)
                }
            }
            None if query => Ok(Pow::Phase(None)),
            None => match parse_num(a) {
                Some(v) if (-127.0..=127.0).contains(&v) => Ok(Pow::Phase(Some(v as i8))),
                Some(_) => Err(RANGE),
                None => Err(BAD),
            },
            _ => Err(HDR),
        };
    }
    if kw(l0, "CYCle") && query {
        return Ok(Pow::Cycle);
    }
    if kw(l0, "METHod") {
        return if query {
            Ok(Pow::Method(None))
        } else if kw(a, "SIMultaneous") || a == b"P" {
            Ok(Pow::Method(Some(false)))
        } else if kw(a, "ALTernate") || a == b"VA" {
            Ok(Pow::Method(Some(true)))
        } else {
            Err(BAD)
        };
    }
    if kw(l0, "LIMit") {
        if query {
            return Ok(Pow::Limit(None));
        }
        let i = args.next().and_then(|x| parse_num_u(x, b"A")).ok_or(BAD)?;
        let t = args.next().map_or(Some(crate::dmm::pw::T_LIM_DEFAULT as f32), |x| parse_num_u(x, b"S")).ok_or(BAD)?;
        return if (0.0..=10.0).contains(&i) && (0.0..=3600.0).contains(&t) { Ok(Pow::Limit(Some((i, t as u16)))) } else { Err(RANGE) };
    }
    Err(HDR)
}

/// SOURce / OUTPut / TIMebase / CHANnel1 / TRIGger (generator and scope).
/// A query gives the query Cmd (None inside); a set gives Some(value) or an
/// error (BAD: no such value, RANGE: outside the setting's domain); the
/// App clamps a value in the domain to the nearest setting (the generator
/// 1 Hz .. 10 MHz, the scope's timebase and V/div lists).
#[inline(never)] // region B: the scope's commands are polled while it streams
fn gen_scope_cmd(parts: &[&[u8]], arg: &[u8], query: bool) -> Result<Cmd, &'static str> {
    let p0 = parts[0];
    let sub = parts.get(1).copied().unwrap_or(&[]);
    let a = trim(arg);
    // A number in `unit` with `ok` its domain.
    let num = |unit: &[u8], ok: fn(f32) -> bool| -> Result<Option<f32>, &'static str> {
        if query {
            return Ok(None);
        }
        match parse_num_u(a, unit) {
            Some(v) if ok(v) => Ok(Some(v)),
            Some(_) => Err(RANGE),
            None => Err(BAD),
        }
    };
    let any = |_: f32| true;
    let pos = |v: f32| v > 0.0;
    // A keyword from `set` (index), or the query.
    let pick = |set: &[&str]| -> Result<Option<u8>, &'static str> {
        if query {
            return Ok(None);
        }
        set.iter().position(|m| kw(a, m)).map(|i| Some(i as u8)).ok_or(BAD)
    };
    if kw(p0, "OUTPut") {
        return match (query, on_off(a)) {
            (true, _) => Ok(Cmd::GenOut(None)),
            (false, Some(b)) => Ok(Cmd::GenOut(Some(b))),
            _ => Err(BAD),
        };
    }
    if kw(p0, "SOURce") {
        return if kw(sub, "FUNCtion") && parts.len() >= 4 {
            // SOUR:FUNC:SQU:DCYC <percent>
            Ok(Cmd::GenDuty(num(b"PCT", |v| (0.0..=100.0).contains(&v))?))
        } else if kw(sub, "FUNCtion") {
            Ok(Cmd::GenFunc(pick(&["SINusoid", "SQUare", "TRIangle", "RAMP"])?))
        } else if kw(sub, "FREQuency") {
            Ok(Cmd::GenFreq(num(b"HZ", pos)?))
        } else if kw(sub, "VOLTage") && parts.len() >= 3 && kw(parts[2], "OFFSet") {
            if !query && kw(a, "AUTO") {
                Ok(Cmd::GenOffs(Some(f32::NAN)))
            } else {
                Ok(Cmd::GenOffs(num(b"V", any)?))
            }
        } else if kw(sub, "VOLTage") {
            Ok(Cmd::GenVolt(num(b"V", |v| v >= 0.0)?))
        } else {
            Err("Undefined header")
        };
    }
    if kw(p0, "TIMebase") {
        // TIMebase[:MAIN][:SCALe]
        return if parts[1..].iter().all(|p| kw(p, "MAIN") || kw(p, "SCALe")) {
            Ok(Cmd::TimScale(num(b"S", pos)?))
        } else {
            Err("Undefined header")
        };
    }
    if kw(p0, "TRIGger") {
        // TRIGger:EDGE:LEVel / :SLOPe = TRIGger:LEVel / :SLOPe (Rigol's
        // spelling; edge is the trigger type the scope page selects).
        let sub = if kw(sub, "EDGE") { parts.get(2).copied().unwrap_or(&[]) } else { sub };
        return if kw(sub, "STATus") && query {
            Ok(Cmd::TrigStatus)
        } else if kw(sub, "SLOPe") {
            Ok(Cmd::TrigSlope(pick(&["POSitive", "NEGative"])?.map(|i| i == 0)))
        } else if kw(sub, "SWEep") {
            Ok(Cmd::TrigSweep(pick(&["AUTO", "NORMal", "SINGle"])?))
        } else if kw(sub, "SOURce") {
            Ok(Cmd::TrigSource(pick(&["CHANnel1", "GENerator"])?.map(|i| i as u8)))
        } else if sub.is_empty() || kw(sub, "LEVel") {
            Ok(Cmd::TrigLevel(num(b"V", any)?))
        } else {
            Err("Undefined header")
        };
    }
    // CHANnel[1]
    if kw(sub, "COUPling") {
        Ok(Cmd::ChCoup(pick(&["DC", "AC"])?.map(|i| i == 1)))
    } else if kw(sub, "OFFSet") {
        Ok(Cmd::ChOffs(num(b"V", any)?))
    } else if sub.is_empty() || kw(sub, "SCALe") {
        Ok(Cmd::ChScale(num(b"V", pos)?))
    } else {
        Err("Undefined header")
    }
}

fn on_off(a: &[u8]) -> Option<bool> {
    match a {
        b"ON" | b"1" => Some(true),
        b"OFF" | b"0" => Some(false),
        _ => None,
    }
}

/// Parse a number like "1.5", "-2e-3", "10k", "5m", "2.5 MHz" (no unit
/// allowed: `parse_num_u` with an empty unit).
pub fn parse_num(a: &[u8]) -> Option<f32> {
    parse_num_u(a, b"")
}

/// IEEE 488.2 suffix multipliers (7.7.3, case-insensitive): MA = mega,
/// M = milli.
const MULT: [(&[u8], f32); 8] =
    [(b"T", 1e12), (b"G", 1e9), (b"MA", 1e6), (b"K", 1e3), (b"M", 1e-3), (b"U", 1e-6), (b"N", 1e-9), (b"P", 1e-12)];

/// The multiplier of suffix `s` (upper case) for a parameter in `unit`
/// (upper case, e.g. b"HZ"; empty: none allowed): [<multiplier>][<unit>],
/// IEEE 488.2 7.7.3 / SCPI-99 7.1.2. MHZ and MOHM are mega (the two
/// exceptions where M is not milli). "MA" with the unit A is milliampere
/// (M + A); mega-ampere is "MAA". None: not a valid suffix here.
fn suffix_mult(s: &[u8], unit: &[u8]) -> Option<f32> {
    if s.is_empty() {
        return Some(1.0);
    }
    if !unit.is_empty() && s.len() == unit.len() + 1 && s[0] == b'M' && &s[1..] == unit && (unit == b"HZ" || unit == b"OHM") {
        return Some(1e6);
    }
    let m = if !unit.is_empty() && s.ends_with(unit) { &s[..s.len() - unit.len()] } else { s };
    if m.is_empty() {
        return Some(1.0);
    }
    MULT.iter().find(|(k, _)| *k == m).map(|&(_, v)| v)
}

/// `parse_num` with an optional unit suffix `unit` (upper case: V, A,
/// HZ, OHM, S, PCT): "1.5", "-2E-3", "10K", "5M" (milli), "1MA" (mega
/// without a unit; with the unit A: milliampere), "2.5MHZ", "100 MV".
/// Case-insensitive: the parser upper-cases the message first.
pub fn parse_num_u(a: &[u8], unit: &[u8]) -> Option<f32> {
    let a = trim(a);
    if a.is_empty() {
        return None;
    }
    // The suffix: the trailing letters, after an optional space (an
    // exponent "E-3" ends in a digit, so it is never taken for one).
    let mut end = a.len();
    while end > 0 && a[end - 1].is_ascii_alphabetic() {
        end -= 1;
    }
    let mut sfx = [0u8; 8];
    let sl = a.len() - end;
    if sl > sfx.len() {
        return None;
    }
    for (d, c) in sfx.iter_mut().zip(&a[end..]) {
        *d = c.to_ascii_uppercase();
    }
    let mult = suffix_mult(&sfx[..sl], unit)?;
    let t = trim(&a[..end]);
    let mut i = 0;
    let neg = t.first() == Some(&b'-');
    if neg || t.first() == Some(&b'+') {
        i += 1;
    }
    let mut v = 0.0f32;
    let mut any = false;
    while i < t.len() && t[i].is_ascii_digit() {
        v = v * 10.0 + (t[i] - b'0') as f32;
        i += 1;
        any = true;
    }
    if i < t.len() && t[i] == b'.' {
        i += 1;
        let mut k = 0.1f32;
        while i < t.len() && t[i].is_ascii_digit() {
            v += (t[i] - b'0') as f32 * k;
            k *= 0.1;
            i += 1;
            any = true;
        }
    }
    if !any {
        return None;
    }
    if i < t.len() && (t[i] == b'e' || t[i] == b'E') {
        i += 1;
        let eneg = t.get(i) == Some(&b'-');
        if eneg || t.get(i) == Some(&b'+') {
            i += 1;
        }
        let mut e = 0i32;
        while i < t.len() && t[i].is_ascii_digit() {
            e = (e * 10 + (t[i] - b'0') as i32).min(99);
            i += 1;
        }
        for _ in 0..e {
            v = if eneg { v / 10.0 } else { v * 10.0 };
        }
    }
    if i != t.len() {
        return None;
    }
    Some(if neg { -v } else { v } * mult).filter(|v| v.is_finite())
}

pub struct Scpi {
    pub buf: [u8; 512],
    pub len: usize,
    /// The error queue (SCPI-99 21.8): FIFO, `ERRQ` entries, the code from
    /// the message (`err_code`). Full: the newest entry becomes "Queue
    /// overflow" (-350). SYSTem:ERRor? pops, *CLS clears.
    errq: [&'static str; ERRQ],
    errn: u8,
    /// `func_id` of the function the last FUNCtion? / CONFigure? named, until
    /// the next READ? / FETCh? takes it (`take_asked`); NO_FUNC = none.
    asked: u8,
}

const NO_FUNC: u8 = 0xFF;

/// Error queue depth (SCPI-99: at least 2; opendm40 keeps 8).
pub const ERRQ: usize = 8;

/// The parameter-check messages of the parser. "Missing parameter" is
/// pushed instead of BAD when a set command had no argument.
const BAD: &str = "Illegal parameter value";
const RANGE: &str = "Data out of range";
const MISSING: &str = "Missing parameter";
const NOT_FOUND: &str = "File name not found";
const NOT_ALLOWED: &str = "Parameter not allowed";
const SYNTAX: &str = "Syntax error";
const QUEUE_OVERFLOW: &str = "Queue overflow";

/// The SCPI error code of an error message (SCPI-99 21.8, IEEE 488.2):
/// the standard message, or its text before "; " (a message may carry
/// details). Anything else is an execution error (-200) with its own text.
pub fn err_code(e: &str) -> i16 {
    let head = match e.find(';') {
        Some(i) => &e[..i],
        None => e,
    };
    match head {
        "Command error" => -100,
        "Syntax error" => -102,
        "Parameter not allowed" => -108,
        "Missing parameter" => -109,
        "Undefined header" => -113,
        "Execution error" => -200,
        "Command protected" => -203,
        "Settings conflict" => -221,
        "Data out of range" => -222,
        "Too much data" => -223,
        "Illegal parameter value" => -224,
        "Out of memory" => -225,
        "Data stale" => -230,
        "Hardware error" | "Flash error" => -240,
        "Mass storage error" | "Mass storage busy" => -250,
        "Mass storage full" => -254,
        "File name not found" => -256,
        "Queue overflow" => -350,
        "Input buffer overrun" => -363,
        "Query error" => -400,
        "Query INTERRUPTED" => -410,
        _ => -200,
    }
}

/// Match `tok` (upper case) against a mnemonic written as e.g. "MEASure":
/// accepted are the short form (upper-case letters) and the full word. A
/// numeric suffix ("CHANnel1") also goes with the short form ("CHAN1");
/// the short form without it ("CHAN") stays accepted.
fn kw(tok: &[u8], mnem: &str) -> bool {
    let m = mnem.as_bytes();
    let short: usize = m.iter().take_while(|b| b.is_ascii_uppercase() || **b == b'*').count();
    let full = m.len();
    let digits = m.iter().rev().take_while(|b| b.is_ascii_digit()).count();
    let eq = |n: usize| tok.len() == n && tok.iter().zip(m).all(|(a, b)| *a == b.to_ascii_uppercase());
    let suffixed = digits > 0
        && short + digits < full
        && tok.len() == short + digits
        && tok[..short].iter().zip(m).all(|(a, b)| *a == b.to_ascii_uppercase())
        && tok[short..] == m[full - digits..];
    eq(short) || eq(full) || suffixed
}

/// [SENSe:]VOLTage:COUPling (Some(false)) / [SENSe:]CURRent:COUPling
/// (Some(true)); None for any other header.
fn coup_cmd(parts: &[&[u8]]) -> Option<bool> {
    let r = if parts.first().is_some_and(|p| kw(p, "SENSe")) { &parts[1..] } else { parts };
    if r.len() != 2 || !kw(r[1], "COUPling") {
        return None;
    }
    if kw(r[0], "VOLTage") {
        Some(false)
    } else if kw(r[0], "CURRent") {
        Some(true)
    } else {
        None
    }
}

fn parse_func(parts: &[&[u8]]) -> Option<u8> {
    let p0 = *parts.first()?;
    let ac = parts.get(1).map(|p| kw(p, "AC")).unwrap_or(false);
    let acdc = parts.get(1).map(|p| kw(p, "ACDC")).unwrap_or(false);
    let auto = parts.get(1).map(|p| kw(p, "AUTO")).unwrap_or(false);
    let dc_or_none = parts.get(1).map(|p| kw(p, "DC")).unwrap_or(true);
    if kw(p0, "VOLTage") {
        if ac { Some(0) } else if acdc { Some(16) } else if auto { Some(crate::dmm::FUNC_V_AUTO) } else if dc_or_none { Some(1) } else { None }
    } else if kw(p0, "CURRent") {
        if ac { Some(5) } else if acdc { Some(17) } else if auto { Some(crate::dmm::FUNC_A_AUTO) } else if dc_or_none { Some(8) } else { None }
    } else if kw(p0, "RESistance") || kw(p0, "FRESistance") {
        Some(10)
    } else if kw(p0, "CONTinuity") {
        Some(11)
    } else if kw(p0, "DIODe") {
        Some(12)
    } else if kw(p0, "CAPacitance") {
        Some(13)
    } else if kw(p0, "FREQuency") {
        Some(14)
    } else if kw(p0, "TEMPerature") {
        Some(15)
    } else if kw(p0, "POWer") && parts.len() == 1 {
        Some(crate::dmm::pw::FUNC)
    } else {
        None
    }
}

/// Temperature sensor by name (dmm.rs T_NAMES; TC / K = TCK).
fn parse_sensor(a: &[u8]) -> Option<u8> {
    let a = trim(a);
    if a == b"TC" || a == b"K" {
        return Some(0);
    }
    crate::dmm::T_NAMES.iter().position(|n| n.as_bytes() == a).map(|k| k as u8)
}

/// Function with an optional temperature sensor argument: TEMP <sensor> ->
/// 32 + sensor (Dmm::select).
fn parse_func_arg(parts: &[&[u8]], arg: &[u8]) -> Option<u8> {
    let f = parse_func(parts)?;
    if f == 15 && !trim(arg).is_empty() {
        return parse_sensor(arg).map(|k| 32 + k);
    }
    Some(f)
}

/// SCPI "not a number" (SCPI-99 7.2.1.5): no reading to give (READ? /
/// MEAS? / FETCh? "Data stale", FETC:POW:ALL? parts). +9.9E37 is the
/// overload.
pub const NAN: &[u8] = b"+9.91000000E+37";

/// The FUNCtion? identity of a `Dmm::func` code: the modes one function
/// autoranges through (V DC 1 / mV 3, V AC 0 / 2, A AC 4..6, A DC 7..9)
/// are one function (`func_name` gives them one name).
pub fn func_id(mode: u8) -> u8 {
    match mode {
        2 => 0,
        3 => 1,
        5 | 6 => 4,
        8 | 9 => 7,
        m => m,
    }
}

/// READ? / FETCh?: the function the host's last FUNCtion? / CONFigure?
/// named, if it asked one since its last READ? / FETCh? (libsigrok asks
/// FUNCtion? before every READ?). main.rs answers +9.91E37 when the function
/// changed on the meter in between (`forget_asked` when this host changed
/// it): the reading would be filed under the old function. Once per
/// question, so a host that asked FUNCtion? once and then only reads gets
/// one NaN per change at most.
pub fn take_asked(s: &mut Scpi) -> Option<u8> {
    let a = core::mem::replace(&mut s.asked, NO_FUNC);
    (a != NO_FUNC).then_some(a)
}

/// The host changed the function itself (CONF, MEAS:<f>?, FUNC, INR:STAT,
/// ...) after its FUNCtion? / CONFigure?: it knows, no NaN for its READ?.
pub fn forget_asked(s: &mut Scpi) {
    s.asked = NO_FUNC;
}

fn func_name(mode: u8) -> &'static str {
    match mode {
        0 | 2 => "\"VOLT:AC\"",
        1 | 3 => "\"VOLT\"",
        4..=6 => "\"CURR:AC\"",
        7..=9 => "\"CURR\"",
        16 => "\"VOLT:ACDC\"",
        17 => "\"CURR:ACDC\"",
        crate::dmm::FUNC_V_AUTO => "\"VOLT:AUTO\"",
        crate::dmm::FUNC_A_AUTO => "\"CURR:AUTO\"",
        crate::dmm::pw::FUNC => "\"POW\"",
        10 => "\"RES\"",
        11 => "\"CONT\"",
        12 => "\"DIOD\"",
        13 => "\"CAP\"",
        14 => "\"FREQ\"",
        _ => "\"TEMP\"",
    }
}

impl Scpi {
    pub const fn new() -> Self {
        Scpi { buf: [0; 512], len: 0, errq: [""; ERRQ], errn: 0, asked: NO_FUNC }
    }

    fn put(&mut self, s: &[u8]) {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s[..n]);
        self.len += n;
    }

    /// Decimal `v`.
    pub fn put_u32(&mut self, mut v: u32) {
        let mut t = [0u8; 10];
        let mut n = t.len();
        loop {
            n -= 1;
            t[n] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        self.put(&t[n..]);
    }

    /// Process one message. Returns what the caller must do; text replies are
    /// accumulated in `buf` (cleared at the start).
    pub fn handle(&mut self, msg: &[u8], cur: u8) -> Out {
        self.len = 0;
        let mut out = Out::None;
        for cmd in msg.split(|&b| b == b';' || b == b'\n' || b == b'\r') {
            let cmd = trim(cmd);
            if cmd.is_empty() {
                continue;
            }
            let mut up = [0u8; 64];
            if cmd.len() > up.len() {
                // Cut off, it could read as another command.
                self.set_err(SYNTAX);
                continue;
            }
            let n = cmd.len();
            for (d, s) in up.iter_mut().zip(&cmd[..n]) {
                *d = s.to_ascii_uppercase();
            }
            let cmd = &up[..n];
            let (head, arg) = match cmd.iter().position(|&b| b == b' ' || b == b'\t') {
                Some(i) => (&cmd[..i], trim(&cmd[i + 1..])),
                None => (cmd, &cmd[..0]),
            };
            let query = head.last() == Some(&b'?');
            let head = if query { &head[..head.len() - 1] } else { head };
            let no_arg = arg.is_empty() && !query;
            // A header is letters, digits, '*' (common commands), ':' and
            // '_' (none uses it; kept for hosts' spellings); a second '?' or
            // any other character is a syntax error (-102).
            if !head.iter().all(|&b| b.is_ascii_alphanumeric() || b == b':' || b == b'*' || b == b'_') {
                self.set_err(SYNTAX);
                continue;
            }
            let mut parts_buf: [&[u8]; 6] = [&[]; 6];
            let mut np = 0;
            for p in head.split(|&b| b == b':').filter(|p| !p.is_empty()) {
                if np < 6 {
                    parts_buf[np] = p;
                    np += 1;
                }
            }
            let parts = &parts_buf[..np];
            let Some(&p0) = parts.first() else { continue };
            // Commands without parameters given one (-108).
            if !arg.is_empty()
                && (p0.first() == Some(&b'*')
                    || (parts.len() == 1 && ["READ", "FETCh", "RUN", "STOP", "SINGle", "DIGitize", "INITiate", "ABORt"].iter().any(|m| kw(p0, m)))
                    || (parts.len() >= 2 && kw(p0, "SYSTem") && kw(parts[1], "ERRor")))
            {
                self.set_err(NOT_ALLOWED);
                continue;
            }

            if self.len > 0 && query {
                self.put(b";");
            }
            match (p0, query) {
                (b"*IDN", true) if idn_emul() == 1 => {
                    self.put(IDN_34401A);
                    self.put(IDN_TAIL);
                    self.put(crate::ident::VERSION.as_bytes());
                }
                (b"*IDN", true) => {
                    self.put(IDN_HEAD);
                    self.put(&[b'A' + model().0, b',']);
                    self.put(crate::uid::bytes());
                    self.put(IDN_TAIL);
                    self.put(crate::ident::VERSION.as_bytes());
                }
                // SYSTem:MODel?: "DM40A|B|C,<raw byte>,<counts>": the model the
                // calibration record names (stock rule: record +0x168, a record
                // without version 1 counts as DM40A) and the count limits
                // opendm40 uses on every model (60000, dmm.rs COUNTS_MODEL).
                _ if kw(p0, "SYSTem") && parts.len() == 2 && kw(parts[1], "MODel") && query => {
                    let (i, raw) = model();
                    self.put(b"DM40");
                    self.put(&[b'A' + i, b',']);
                    let mut nb = [0u8; 3];
                    let mut n = 0;
                    let mut v = raw;
                    loop {
                        nb[2 - n] = b'0' + v % 10;
                        n += 1;
                        v /= 10;
                        if v == 0 {
                            break;
                        }
                    }
                    self.put(&nb[3 - n..]);
                    self.put(b",60000");
                }
                _ if kw(p0, "SYSTem") && parts.len() > 2 && kw(parts[1], "IDN") && kw(parts[2], "EMULation") => {
                    if query {
                        self.put(if idn_emul() == 1 { b"34401A" } else { b"OFF" });
                    } else {
                        match trim(arg) {
                            b"OFF" | b"0" => set_idn_emul(0),
                            b"34401A" | b"HP34401A" => set_idn_emul(1),
                            _ => self.err_arg(no_arg, "Illegal parameter value"),
                        }
                    }
                }
                // Accepted for Agilent / HP-style hosts (libsigrok's 34401A
                // profile sends INIT at the start, ABORT at the end); the
                // meter measures continuously, READ? takes the next reading.
                _ if (kw(p0, "INITiate") || kw(p0, "ABORt")) && !query => {}
                _ if kw(p0, "SYSTem") && parts.len() == 2 && (kw(parts[1], "LOCal") || kw(parts[1], "REMote")) && !query => {}
                // *CLS: the error queue and the event register cleared
                // (IEEE 488.2 10.3); *RST leaves both (10.32) and is
                // accepted (no settings to reset through SCPI).
                (b"*CLS", false) => {
                    self.errn = 0;
                    crate::sentinel::QUES_EVENT.store(0, core::sync::atomic::Ordering::Relaxed);
                }
                (b"*RST", false) => {}
                _ if kw(p0, "STATus") && parts.len() > 1 && kw(parts[1], "QUEStionable") && query => {
                    if parts.len() == 3 && kw(parts[2], "CONDition") {
                        out = Out::Instr(Cmd::StatQuesCond);
                    } else if parts.len() == 2 || (parts.len() == 3 && kw(parts[2], "EVENt")) {
                        let v = crate::sentinel::QUES_EVENT.swap(0, core::sync::atomic::Ordering::Relaxed);
                        self.put_u32(v as u32);
                    } else {
                        self.err_arg(no_arg, "Undefined header");
                    }
                }
                (b"*OPC", true) => self.put(b"1"),
                (b"*ESR", true) | (b"*STB", true) => self.put(b"0"),
                _ if kw(p0, "SYSTem") && parts.len() > 1 && kw(parts[1], "TEMPerature") && query => {
                    out = Out::Instr(Cmd::SysTemp)
                }
                _ if kw(p0, "SYSTem") && parts.len() > 1 && kw(parts[1], "RAIL") => {
                    let a = trim(arg);
                    out = Out::Instr(if parts.len() > 2 && query {
                        Cmd::SysRailState
                    } else if query {
                        Cmd::SysRail(None)
                    } else if kw(a, "BOTH") {
                        Cmd::SysRail(Some(true))
                    } else if kw(a, "AUTO") {
                        Cmd::SysRail(Some(false))
                    } else {
                        self.err_arg(no_arg, "Illegal parameter value");
                        continue;
                    });
                }
                _ if kw(p0, "SYSTem") && parts.len() > 1 && kw(parts[1], "FUSE") => {
                    out = Out::Instr(match (parts.len(), query) {
                        (2, true) => Cmd::SysFuse(false),
                        (3, false) if kw(parts[2], "TEST") => Cmd::SysFuse(true),
                        _ => {
                            self.err_arg(no_arg, "Undefined header");
                            continue;
                        }
                    });
                }
                _ if kw(p0, "SYSTem") && parts.len() == 3 && kw(parts[1], "BEEPer") && kw(parts[2], "VOLume") => {
                    out = Out::Instr(if query {
                        Cmd::SysBeepVol(None)
                    } else {
                        match parse_num(trim(arg)) {
                            Some(v) if (0.0..=100.0).contains(&v) => Cmd::SysBeepVol(Some(v as u8)),
                            v => {
                                self.err_arg(no_arg, if v.is_some() { "Data out of range" } else { "Illegal parameter value" });
                                continue;
                            }
                        }
                    });
                }
                _ if p0 == b"DBG" && query && parts.len() == 2 && kw(parts[1], "ALARm") => out = Out::Instr(Cmd::DbgAlarm),
                _ if kw(p0, "SYSTem") && parts.len() > 1 && kw(parts[1], "ALERt") => {
                    let a = trim(arg);
                    out = Out::Instr(if parts.len() == 2 && query {
                        Cmd::SysAlert
                    } else if parts.len() == 3 && kw(parts[2], "ALARm") && query {
                        Cmd::SysAlarm(None)
                    } else if parts.len() == 3 && kw(parts[2], "ALARm") {
                        if kw(a, "LOUD") {
                            Cmd::SysAlarm(Some(crate::danger::LOUD))
                        } else if kw(a, "VOLume") {
                            Cmd::SysAlarm(Some(crate::danger::VOLUME))
                        } else if kw(a, "OFF") {
                            Cmd::SysAlarm(Some(crate::danger::OFF))
                        } else {
                            self.err_arg(no_arg, "Illegal parameter value");
                            continue;
                        }
                    } else if parts.len() == 3 && kw(parts[2], "ENABle") && query {
                        Cmd::SysAlertEnable(None)
                    } else if parts.len() == 3 && kw(parts[2], "ENABle") {
                        match on_off(a) {
                            Some(on) => Cmd::SysAlertEnable(Some(if on { crate::sentinel::M_ALL } else { 0 })),
                            None => match parse_num(a) {
                                Some(v) if (0.0..=15.0).contains(&v) && v == (v as u8) as f32 => Cmd::SysAlertEnable(Some(v as u8)),
                                v => {
                                    // A whole number outside 0..15: -222.
                                    self.err_arg(no_arg, if v.is_some_and(|v| v == (v as i32) as f32) { "Data out of range" } else { "Illegal parameter value" });
                                    continue;
                                }
                            },
                        }
                    } else {
                        self.err_arg(no_arg, "Undefined header");
                        continue;
                    });
                }
                _ if kw(p0, "UNIT") && parts.len() > 1 && kw(parts[1], "TEMPerature") => {
                    let a = trim(arg);
                    out = Out::Instr(if query {
                        Cmd::TempUnit(None)
                    } else if kw(a, "C") || kw(a, "CEL") {
                        Cmd::TempUnit(Some(false))
                    } else if kw(a, "F") || kw(a, "FAR") {
                        Cmd::TempUnit(Some(true))
                    } else {
                        self.err_arg(no_arg, "Illegal parameter value");
                        continue;
                    });
                }
                _ if kw(p0, "MEASure") && query && parts.len() > 2 && kw(parts[1], "TEMPerature") && kw(parts[2], "CJ") => {
                    out = Out::Instr(Cmd::TempCj)
                }
                _ if kw(p0, "SYSTem") && parts.len() > 1 && kw(parts[1], "ERRor") && query => {
                    if parts.len() > 2 && kw(parts[2], "COUNt") {
                        self.put_u32(self.errn as u32);
                        continue;
                    }
                    match self.pop_err() {
                        Some(e) => {
                            let c = err_code(e);
                            self.put(b"-");
                            self.put_u32(c.unsigned_abs() as u32);
                            self.put(b",\"");
                            self.put(e.as_bytes());
                            self.put(b"\"");
                        }
                        None => self.put(b"0,\"No error\""),
                    }
                }
                _ if is_pow(parts, query) => match parse_pow(parts, arg, query) {
                    Ok(c) => out = Out::Instr(Cmd::Pow(c)),
                    Err(e) => self.err_arg(no_arg, e),
                },
                _ if kw(p0, "MEASure") && query => match parse_func_arg(&parts[1..], arg) {
                    Some(f) => out = Out::Measure(Some(f)),
                    None => self.err_arg(no_arg, "Command error"),
                },
                _ if kw(p0, "CONFigure") && query && parts.len() == 1 => out = Out::Instr(Cmd::ConfQuery),
                _ if kw(p0, "CONFigure") && !query => match parse_func_arg(&parts[1..], arg) {
                    // Temperature (its argument is the sensor).
                    Some(f) if f == 15 || f >= 32 => out = Out::Configure(f),
                    Some(f) => match conf_cmd(f, arg) {
                        Ok(c) => out = Out::Instr(c),
                        Err(e) => self.err_arg(no_arg, e),
                    },
                    None => self.err_arg(no_arg, "Command error"),
                },
                // [SENSe:]<f>:RANGe / RANGe:AUTO / RESolution (opendm40).
                // range_cmd is .cold (region C): only for a header that names
                // RANGe / RESolution. Every other header below (LOG:DATA?,
                // LOG:DISK?, DBG:ISR?, MMEM...) reached it, and a fetch from C
                // while the AD2 stream runs delays the ISR (device 825080c:
                // max_lat 1.6-20 us at range_cmd's entry, overruns / slips,
                // one 76800 writer trial fell back).
                _ if parts.len() > 1
                    && parts[1..].iter().any(|p| kw(p, "RANGe") || kw(p, "RESolution"))
                    && range_cmd(parts, arg, query).is_some() =>
                {
                    match range_cmd(parts, arg, query) {
                        Some(Ok(c)) => out = Out::Instr(c),
                        Some(Err(e)) => self.err_arg(no_arg, e),
                        None => {}
                    }
                }
                (b"READ", true) => out = Out::Measure(None),
                // Generator and scope settings: a set with a value that does
                // not parse is -224 (-109 without one), one outside the
                // setting's domain -222; the setting is left as it was.
                _ if kw(p0, "SOURce") || kw(p0, "OUTPut") || kw(p0, "TIMebase") || kw(p0.strip_suffix(b"1").unwrap_or(p0), "CHANnel") || kw(p0, "TRIGger") => {
                    match gen_scope_cmd(parts, arg, query) {
                        Ok(c) => out = Out::Instr(c),
                        Err(e) => self.err_arg(no_arg, e),
                    }
                }
                (b"RUN", false) => out = Out::Instr(Cmd::Run),
                (b"STOP", false) => out = Out::Instr(Cmd::Stop),
                _ if kw(p0, "SINGle") && !query => out = Out::Instr(Cmd::Single),
                _ if kw(p0, "DIGitize") && !query => out = Out::Instr(Cmd::Digitize),
                _ if kw(p0, "ACQuire") && query && parts.len() == 2 && kw(parts[1], "SRATe") => out = Out::Instr(Cmd::AcqSrate),
                _ if kw(p0, "SCOPe") && query => {
                    let m = parts.last().copied().unwrap_or(&[]);
                    let k = if kw(m, "VPP") {
                        0
                    } else if kw(m, "VAVG") {
                        1
                    } else if kw(m, "VRMS") {
                        2
                    } else {
                        3
                    };
                    out = Out::Instr(Cmd::Meas(k));
                }
                // Scope drag A/B aids (App::scpi): DBG:GRAB <ms> (0 = direct
                // drag) / DBG:GRAB?, DBG:GEST? (last drags).
                _ if p0 == b"DBG" && parts.get(1) == Some(&&b"PINCH"[..]) => {
                    out = Out::Instr(Cmd::DbgPinch(if query { None } else { parse_num(arg).map(|v| v.clamp(0.0, 2.0) as u8) }))
                }
                _ if p0 == b"DBG" && parts.get(1) == Some(&&b"GRAB"[..]) => {
                    match (query, parse_num(arg)) {
                        (true, _) => out = Out::Instr(Cmd::DbgGrab(None)),
                        (false, Some(v)) => out = Out::Instr(Cmd::DbgGrab(Some(v.clamp(0.0, 5000.0) as u16))),
                        _ => self.err_arg(no_arg, BAD),
                    }
                }
                _ if p0 == b"DBG" && parts.get(1) == Some(&&b"GEST"[..]) => out = Out::Instr(Cmd::DbgGest),
                _ if p0 == b"DBG" && query && parts.get(1) == Some(&&b"VIEW"[..]) => out = Out::Instr(Cmd::DbgView),
                _ if p0 == b"DBG" && parts.len() == 3 && parts[1] == b"TOUCH" && kw(parts[2], "CAL") => {
                    let a = trim(arg);
                    if query {
                        out = Out::Instr(Cmd::DbgTouchCal(0));
                    } else if kw(a, "RESet") {
                        out = Out::Instr(Cmd::DbgTouchCal(1));
                    } else if tcal_arg(a) {
                        out = Out::Instr(Cmd::DbgTouchCal(2));
                    } else {
                        self.err_arg(no_arg, BAD);
                    }
                }
                _ if p0 == b"DBG" && parts.len() == 3 && parts[1] == b"TOUCH" && kw(parts[2], "TEST") => {
                    match (query, on_off(trim(arg))) {
                        (true, _) => out = Out::Instr(Cmd::DbgTouchTest(None)),
                        (false, Some(on)) => out = Out::Instr(Cmd::DbgTouchTest(Some(on))),
                        _ => self.err_arg(no_arg, BAD),
                    }
                }
                _ if p0 == b"DBG" && query && parts.len() == 2 && kw(parts[1], "READout") => out = Out::Instr(Cmd::DbgReadout),
                // Without the feature `acdc-trace`: no such header (Cargo.toml).
                #[cfg(not(feature = "acdc-trace"))]
                _ if p0 == b"DBG" && parts.len() == 3 && parts[1] == b"ACDC" && parts[2] == b"TRACE" => self.err_arg(no_arg, "Undefined header"),
                // Without the feature `acdc-meth`: no method headers (Cargo.toml).
                #[cfg(not(feature = "acdc-meth"))]
                _ if p0 == b"DBG" && parts.len() == 3 && parts[1] == b"ACDC" && matches!(&parts[2][..], b"METH" | b"STAT" | b"ZERO" | b"R0") => {
                    self.err_arg(no_arg, "Undefined header")
                }
                _ if p0 == b"DBG" && parts.len() == 3 && parts[1] == b"ACDC" => {
                    let a = trim(arg);
                    let mut it = a.split(|&c| c == b',');
                    let a0 = trim(it.next().unwrap_or(&[]));
                    #[cfg_attr(not(feature = "acdc-meth"), allow(unused_variables))]
                    let a1 = it.next().map(trim);
                    let c = match (&parts[2][..], query) {
                        #[cfg(feature = "acdc-meth")]
                        (b"METH", true) => Some(Cmd::DbgAm(1, 0, 0.0)),
                        #[cfg(feature = "acdc-meth")]
                        (b"METH", false) => match a0 {
                            b"A" | b"B" | b"C" => Some(Cmd::DbgAm(0, a0[0] - b'A', 0.0)),
                            _ => None,
                        },
                        #[cfg(feature = "acdc-meth")]
                        (b"STAT", true) => Some(Cmd::DbgAm(2, 0, 0.0)),
                        #[cfg(feature = "acdc-meth")]
                        (b"ZERO", true) => Some(Cmd::DbgAm(4, 0, 0.0)),
                        #[cfg(feature = "acdc-meth")]
                        (b"ZERO", false) => match (parse_num(a0), a1) {
                            (Some(k), Some(v)) if (0.0..10.0).contains(&k) => {
                                Some(Cmd::DbgAm(3, k as u8, if kw(v, "AUTO") { f32::NAN } else { parse_num(v).unwrap_or(f32::NAN) }))
                            }
                            _ => None,
                        },
                        (b"RANG", false) => match parse_num(a0) {
                            _ if kw(a0, "AUTO") => Some(Cmd::DbgAm(7, 255, 0.0)),
                            Some(v) if (0.0..=5.0).contains(&v) => Some(Cmd::DbgAm(7, v as u8, 0.0)),
                            _ => None,
                        },
                        #[cfg(feature = "acdc-trace")]
                        (b"TRACE", true) => Some(Cmd::DbgAm(8, parse_num(a0).unwrap_or(0.0).clamp(0.0, 255.0) as u8, 0.0)),
                        #[cfg(feature = "acdc-trace")]
                        (b"TRACE", false) => match parse_num(a0) {
                            Some(v) if (0.0..=2.0).contains(&v) => Some(Cmd::DbgAm(9, v as u8, 0.0)),
                            _ => None,
                        },
                        #[cfg(feature = "acdc-meth")]
                        (b"R0", true) => Some(Cmd::DbgAm(6, 0, 0.0)),
                        #[cfg(feature = "acdc-meth")]
                        (b"R0", false) => match a0 {
                            b"1" | b"2" => Some(Cmd::DbgAm(5, a0[0] - b'0', 0.0)),
                            _ => None,
                        },
                        _ => None,
                    };
                    match c {
                        Some(c) => out = Out::Instr(c),
                        None => self.err_arg(no_arg, "Illegal parameter value"),
                    }
                }
                // DBG:ACDC? only with --features dbg-acdc (region B audit #9).
                _ if cfg!(feature = "dbg-acdc") && p0 == b"DBG" && parts.get(1) == Some(&&b"ACDC"[..]) => {
                    out = Out::Instr(Cmd::DbgAcdc)
                }
                _ if p0 == b"DBG" && parts.get(1).is_some_and(|p| kw(p, "SENTinel")) && parts.get(2).is_some_and(|p| kw(p, "VARiant")) => {
                    match parse_num(arg) {
                        _ if query => out = Out::Instr(Cmd::DbgSentVar(None)),
                        Some(v) if (0.0..=127.0).contains(&v) && v == (v as u8) as f32 => out = Out::Instr(Cmd::DbgSentVar(Some(v as u8))),
                        _ => self.err_arg(no_arg, "Illegal parameter value"),
                    }
                }
                _ if p0 == b"DBG" && query && parts.get(1).is_some_and(|p| kw(p, "SENTinel")) && parts.get(2).is_some_and(|p| kw(p, "LOG")) => out = Out::Instr(Cmd::DbgSentLog),
                #[cfg(feature = "dbg-sent")]
                _ if p0 == b"DBG" && query && parts.get(1).is_some_and(|p| kw(p, "SENTinel")) && parts.get(2).is_some_and(|p| kw(p, "REG")) => out = Out::Instr(Cmd::DbgSentReg),
                #[cfg(feature = "dbg-src")]
                _ if p0 == b"DBG" && parts.len() == 2 && parts[1] == b"SRC" => {
                    if query {
                        out = Out::Instr(Cmd::DbgSrc(None));
                    } else {
                        match crate::dbgsrc::parse(arg) {
                            Some(r) => out = Out::Instr(Cmd::DbgSrc(Some(r))),
                            None => self.set_err("Illegal parameter value"),
                        }
                    }
                }
                _ if p0 == b"DBG" && parts.get(1).is_some_and(|p| kw(p, "SENTinel")) && parts.get(2).is_some_and(|p| kw(p, "PRECond")) => {
                    match parse_num(arg) {
                        _ if query => out = Out::Instr(Cmd::DbgSentPrec(None)),
                        Some(v) if (0.0..=2000.0).contains(&v) => out = Out::Instr(Cmd::DbgSentPrec(Some(v as u16))),
                        _ => self.err_arg(no_arg, "Illegal parameter value"),
                    }
                }
                // DBG:SENTinel? / :REG? only with the feature `dbg-sent`.
                #[cfg(feature = "dbg-sent")]
                _ if p0 == b"DBG" && query && parts.get(1).is_some_and(|p| kw(p, "SENTinel")) => out = Out::Instr(Cmd::DbgSent),
                _ if p0 == b"DBG" && parts.get(1) == Some(&&b"CH2"[..]) && parts.get(2) == Some(&&b"LAG"[..]) => {
                    match parse_num(arg) {
                        _ if query => out = Out::Instr(Cmd::DbgCh2Lag(None)),
                        Some(v) if v.abs() <= 1e5 => out = Out::Instr(Cmd::DbgCh2Lag(Some(v))),
                        _ => self.err_arg(no_arg, "Illegal parameter value"),
                    }
                }
                _ if p0 == b"DBG" && query && parts.get(1) == Some(&&b"GENTRIG"[..]) && parts.get(2) == Some(&&b"STAT"[..]) => {
                    out = Out::Instr(Cmd::DbgGenStat)
                }
                _ if p0 == b"DBG" && parts.get(1) == Some(&&b"GENTRIG"[..]) && parts.get(2) == Some(&&b"LAG"[..]) => {
                    match parse_num(arg) {
                        _ if query => out = Out::Instr(Cmd::DbgGenLag(None)),
                        Some(v) if v.abs() <= 1e6 => out = Out::Instr(Cmd::DbgGenLag(Some(v))),
                        _ => self.err_arg(no_arg, "Illegal parameter value"),
                    }
                }
                _ if p0 == b"DBG" && parts.get(1) == Some(&&b"HWTRIG"[..]) => {
                    let c = match (parts.get(2).copied(), parts.get(3).copied(), query) {
                        (None, _, true) => Some(Cmd::DbgHwTrig(0, false)),
                        (Some(b"ENABLE"), None, false) => on_off(trim(arg)).map(|on| Cmd::DbgHwTrig(1, on)),
                        (Some(b"CAL"), None, true) => Some(Cmd::DbgHwTrig(2, false)),
                        (Some(b"STAT"), None, true) => Some(Cmd::DbgHwTrig(3, false)),
                        (Some(b"STAT"), Some(b"RESET"), false) => Some(Cmd::DbgHwTrig(4, false)),
                        _ => None,
                    };
                    match c {
                        Some(c) => out = Out::Instr(c),
                        None => self.err_arg(no_arg, "Undefined header"),
                    }
                }
                // HY3131 bus self-test (src/hyspi.rs, main.rs).
                // Only with --features dbg-hyspi (region B audit #9): else
                // an undefined header.
                _ if cfg!(feature = "dbg-hyspi") && p0 == b"DBG" && parts.get(1) == Some(&&b"HYSPI"[..]) => {
                    let a = trim(arg);
                    let mut it = a.split(|&b| b == b',').map(trim);
                    let first = it.next().unwrap_or(&[]);
                    let div = it.next().and_then(parse_num).map_or(0, |d| d as u32);
                    let div = if matches!(div, 8 | 16 | 32) { div as u8 } else { 0 };
                    let cpha = it.next().and_then(parse_num).is_some_and(|c| c != 0.0);
                    let d = match (parts.get(2).copied(), query) {
                        (Some(b"SELF"), true) => {
                            let n = parse_num(first).map_or(100, |v| v as u32).clamp(1, crate::hyspi::N_MAX);
                            Some(Dbg::HySpi(n as u16, div | (cpha as u8) << 7))
                        }
                        (Some(b"WR"), true) => Some(Dbg::HySpiWr(parse_num(first).map_or(1, |v| v as u32).min(3) as u8, div)),
                        _ => None,
                    };
                    match d {
                        Some(d) => out = Out::Dbg(d),
                        None => self.err_arg(no_arg, "Undefined header"),
                    }
                }
                // AD2 device experiments (src/ad2exp.rs, main.rs).
                _ if p0 == b"DBG" && parts.get(1) == Some(&&b"AD2"[..]) => {
                    let a = trim(arg);
                    let v = nums::<2>(a);
                    let third = a.split(|&b| b == b',').nth(2).map(trim);
                    let n = v[1].clamp(1, 2000) as u16; // ad2exp::N_MAX
                    let r26 = (v[0] & 0x3F) as u8;
                    #[cfg(not(feature = "ad2-exp"))]
                    let _ = (third, n, r26);
                    let d = match (parts.get(2).copied(), query) {
                        #[cfg(feature = "ad2-exp")]
                        (Some(b"TORN"), true) => Some(Dbg::Ad2(0, r26, n, third.is_some_and(|t| kw(t, "SYNChronous")) as u8)),
                        #[cfg(feature = "ad2-exp")]
                        (Some(b"NOISE"), true) => {
                            Some(Dbg::Ad2(1, r26, n, third.and_then(parse_num).map_or(255, |c| (c as u8).min(3))))
                        }
                        (Some(b"DRAW"), false) => Some(Dbg::Ad2(2, (v[0].min(2)) as u8, 0, 0)),
                        (Some(b"DIV"), false) => Some(Dbg::Ad2(3, (v[0].min(255)) as u8, 0, (v[1] != 0) as u8)),
                        (Some(b"DMA"), false) => Some(Dbg::Ad2(4, (v[0].min(1)) as u8, 0, 0)),
                        (Some(b"DMA"), true) => Some(Dbg::Ad2(4, 255, 0, 0)),
                        // DCV? range,r26,n[,flags]: (5, r26, n | range << 8, flags).
                        #[cfg(feature = "ad2-exp")]
                        (Some(b"DCV"), true) => Some(dcv_arg(a)),
                        // N7? item,state,n: (6, state, n, item).
                        #[cfg(feature = "ad2-exp")]
                        (Some(b"N7"), true) => Some(n7_arg(a)),
                        _ => None,
                    };
                    match d {
                        Some(d) => out = Out::Dbg(d),
                        None => self.err_arg(no_arg, "Undefined header"),
                    }
                }
                _ if p0 == b"DBG" => {
                    let sub = parts.get(1).copied().unwrap_or(&[]);
                    let a = trim(arg);
                    out = Out::Dbg(match sub {
                        b"PING" => Dbg::Ping(nums::<1>(a)[0]),
                        b"TAP" => {
                            let v = nums::<2>(a);
                            Dbg::Input(0, v[0] as u16, v[1] as u16)
                        }
                        b"SWIPE" => match a.first() {
                            Some(&b'R') => Dbg::Input(2, 0, 0),
                            // D,n: synthetic n-finger swipe down (gesture.rs synth_swipe_down).
                            Some(&b'D') => {
                                let n = a.split(|&b| b == b',').nth(1).and_then(parse_num).unwrap_or(1.0);
                                Dbg::Input(4, (n as u16).clamp(1, 5), 0)
                            }
                            _ => Dbg::Input(1, 0, 0),
                        },
                        b"DRAG" => {
                            let v = nums::<7>(a);
                            let ms = if v[4] == 0 { 300 } else { v[4].min(10_000) };
                            Dbg::Drag([v[0].min(479) as u16, v[1].min(319) as u16, v[2].min(479) as u16, v[3].min(319) as u16, ms as u16, v[5].min(10_000) as u16, v[6].min(50) as u16])
                        }
                        b"KEY" => { let v = nums::<2>(a); Dbg::Input(3, v[0] as u16, v[1].min(60_000) as u16) } // n[,hold_ms]
                        b"LCD" => {
                            let v = nums::<5>(a);
                            Dbg::Lcd(v[0] as u16, v[1] as u16, v[2].max(1) as u16, v[3].max(1) as u16, v[4])
                        }
                        // Without the feature: no such header (Cargo.toml).
                        #[cfg(feature = "dbg-diag")]
                        b"DIAG" => Dbg::Diag,
                        // Without the feature: no such header (Cargo.toml).
                        #[cfg(feature = "dbg-touch")]
                        b"TOUCH" => Dbg::Touch(!query),
                        _ if kw(sub, "UPLoad") => {
                            let op = parts.get(2).copied().unwrap_or(b"");
                            let mut v = [0u32; 2];
                            for (d, t) in v.iter_mut().zip(a.split(|&b| b == b',')) {
                                *d = core::str::from_utf8(trim(t)).ok().and_then(|t| t.parse().ok()).unwrap_or(0);
                            }
                            if op.is_empty() && query {
                                Dbg::Upload(3, 0, 0)
                            } else if kw(op, "BEGin") {
                                Dbg::Upload(0, v[0], v[1])
                            } else if kw(op, "END") && query {
                                Dbg::Upload(1, 0, 0)
                            } else if kw(op, "ABORt") {
                                Dbg::Upload(2, 0, 0)
                            } else {
                                self.err_arg(no_arg, "Undefined header");
                                continue;
                            }
                        }
                        // DBG:CHG? / DBG:CHG:PIN n,<mode> / DBG:CHG:OUT <mode> (PB10):
                        // the charger status pins (power.rs dbg_state / dbg_pin).
                        b"CHG" => {
                            self.len = 0;
                            let op = parts.get(2).copied().unwrap_or(b"");
                            if kw(op, "LOG") && query {
                                // n, then the last <= 8 changes oldest first:
                                // ms:pins (bit 0 PB10, 1 PB11, 2 USB).
                                let l = unsafe { &*core::ptr::addr_of!(crate::power::CHG_LOG) };
                                self.put_u32(l.n);
                                for k in l.n.saturating_sub(8)..l.n {
                                    let e = l.ev[k as usize % 8];
                                    self.put(b",");
                                    self.put_u32(e >> 4);
                                    self.put(b":");
                                    self.put_u32(e & 15);
                                }
                            } else if op.is_empty() && query {
                                for (i, v) in crate::power::dbg_state().iter().enumerate() {
                                    if i > 0 {
                                        self.put(b",");
                                    }
                                    self.put_u32(*v);
                                }
                            } else {
                                let mut it = a.split(|&b| b == b',');
                                let pin = if kw(op, "OUT") { Some(10) } else if kw(op, "PIN") { core::str::from_utf8(trim(it.next().unwrap_or(b""))).ok().and_then(|t| t.parse().ok()) } else { None };
                                let m = trim(it.next().unwrap_or(b""));
                                let mode = [&b"IN"[..], b"UP", b"DOWN", b"LOW", b"HIGH", b"OD", b"DEF"].iter().position(|k| m.eq_ignore_ascii_case(k));
                                match (pin, mode) {
                                    (Some(p), Some(md)) if crate::power::dbg_pin(p, md as u32) => {}
                                    _ => self.err_arg(no_arg, "Illegal parameter value"),
                                }
                            }
                            continue;
                        }
                        b"IDLE" => Dbg::Idle(if query { None } else { Some(nums::<1>(a)[0]) }),
                        #[cfg(feature = "dbg-ohm6")]
                        b"OHM6" => Dbg::Ohm6((!query).then(|| {
                            let mut v = [f32::NAN; 5];
                            for (x, f) in v.iter_mut().zip(a.split(|&b| b == b',')) {
                                *x = parse_num(trim(f)).unwrap_or(f32::NAN);
                            }
                            v
                        })),
                        #[cfg(feature = "dbg-prof")]
                        b"PROF" => Dbg::Prof(parts.get(2) == Some(&&b"RESET"[..])),
                        b"FLASH" => Dbg::Flash,
                        b"ISR" => Dbg::Isr,
                        b"FAST" if query => Dbg::Fast,
                        b"RELEASE" => Dbg::Release,
                        b"RESET" => Dbg::Reset,
                        // DBG:UPDATE = DBG:RECOVER: zero the 12-byte loader
                        // marker at 0x08008000, then the deferred SYSRESETREQ ->
                        // vendor update screen + its USB disk (docs/native-boot.md).
                        // DBG:BOOT? = boot info.
                        // DBG:TE? = the panel TE probe (lcd.rs te_probe,
                        // ~0.3 s): per phase (TE off; on, mode 0; on, mode
                        // 1), ";" between: <samples>, then per pin PA3,
                        // PB13, PC1, PC15 <level changes>,<samples high>.
                        #[cfg(target_os = "none")]
                        b"TE" if query => {
                            self.len = 0;
                            for (k, (n, o)) in crate::lcd::te_probe().iter().enumerate() {
                                if k > 0 {
                                    self.put_bytes(b";");
                                }
                                self.put_u32(*n);
                                for &(e, h) in o {
                                    self.put_bytes(b",");
                                    self.put_u32(e);
                                    self.put_bytes(b",");
                                    self.put_u32(h);
                                }
                            }
                            continue;
                        }
                        b"BOOT" => {
                            crate::boot::put_info(self);
                            continue;
                        }
                        // The generated USB volume: EJECT n ("no medium" for n
                        // commands, then UNIT ATTENTION) | EJECT? -> <LUN 0 ejected by
                        // the host>,<START STOP UNITs since boot>,<CB[4] of the last,
                        // 256 = ignored> | STAT? -> <files>,<next free
                        // cluster>,<files left out> | RO 0|1 (write-protect it: hosts
                        // mount it read-only, live updates while mounted; read at
                        // the host's next mount), RO? | WR? -> host writes (vwrite.rs):
                        // <accepted>,<refused>,<busy>,<last refused LBA>,<SPI files
                        // deleted>,<firmware files committed>,<rejected>,<root
                        // records>,<FAT pairs>,<staged clusters>,<frozen>,<staging>,
                        // <firmware state 0 none 1 ask 2 asked 3 declined 4 install 5 bad>.
                        #[cfg(feature = "vdisk")]
                        b"VDISK" => { self.len = 0; let v = nums::<1>(a); match parts.get(2).copied().unwrap_or(b"") { b"EJECT" => { use core::sync::atomic::Ordering::Relaxed; if query { self.put_u32(crate::msc::ejected(0) as u32); self.put(b","); self.put_u32(crate::msc::SSU_COUNT.load(Relaxed)); self.put(b","); self.put_u32(crate::msc::SSU_LAST.load(Relaxed)) } else { crate::msc::DBG_EJECT.store(v[0].clamp(1, 1000) as u32, Relaxed); self.put(b"OK") } } b"RD" => { use core::sync::atomic::Ordering::Relaxed; let r = &crate::vdisk::RD; let f = &crate::dlog::FR; for (i, x) in [r[0].load(Relaxed), (r[1].load(Relaxed) as u64 * 1024 / (crate::clock::HCLK_HZ as u64 / 1000)) as u32, r[2].load(Relaxed) / (crate::clock::HCLK_HZ / 1_000_000), r[3].load(Relaxed), f[0].load(Relaxed), f[1].load(Relaxed), f[2].load(Relaxed), f[3].load(Relaxed), f[4].load(Relaxed)].iter().enumerate() { if i > 0 { self.put(b","); } self.put_u32(*x); } } b"STAT" => { let (n, hw, out) = crate::vdisk::stats(); for (i, x) in [n as u32, hw, out as u32].iter().enumerate() { if i > 0 { self.put(b","); } self.put_u32(*x); } } b"RO" => { if query { self.put_u32(crate::vwrite::read_only() as u32); self.put(b","); self.put_u32(crate::vwrite::read_only_pending() as u32) } else { crate::vwrite::set_read_only(v[0] != 0); self.put(b"OK") } } b"MIR" => { let (ok, st, wr, ms) = crate::vdisk::mirror_info(); let pc = &crate::vdisk::MIR_PACE; use core::sync::atomic::Ordering::Relaxed; for (i, x) in [ok as u32, st as u32, wr as u32, ms, pc[0].load(Relaxed), pc[1].load(Relaxed), pc[2].load(Relaxed)].iter().enumerate() { if i > 0 { self.put(b","); } self.put_u32(*x); } } b"WR" => { let n = crate::vwrite::stats(); let (rec, pairs, st, fz, stg, fw) = crate::vwrite::state(); for (i, x) in [n.ok, n.refused, n.busy, n.last_refused, n.deleted as u32, n.fw_ok as u32, n.fw_bad as u32, rec as u32, pairs as u32, st as u32, fz as u32, stg as u32, fw as u32].iter().enumerate() { if i > 0 { self.put(b","); } self.put_u32(*x); } } _ => self.err_arg(no_arg, "Undefined header") } continue; }
                        b"UPDATE" => { if !crate::boot::enter_loader_update() { self.err_arg(no_arg, "Flash error"); continue; } Dbg::Reset }
                        b"LOADER" | b"RECOVER" => { crate::boot::request_loader(); Dbg::Reset }
                        _ => {
                            self.err_arg(no_arg, "Undefined header");
                            continue;
                        }
                    });
                }
                _ if kw(p0, "DIAG") && query => {
                    let sub = parts.get(1).copied().unwrap_or(&[]);
                    out = Out::Instr(if kw(sub, "PWM") { Cmd::DiagPwm } else { Cmd::DiagDac });
                }
                _ if kw(p0, "INRush") => {
                    let sub = parts.get(1).copied().unwrap_or(&[]);
                    let a = trim(arg);
                    let c = if sub.is_empty() || kw(sub, "STATe") {
                        if query { Some(Inr::State(None)) } else { on_off(a).map(|b| Inr::State(Some(b))) }
                    } else if kw(sub, "ARM") && !query {
                        Some(Inr::Arm)
                    } else if kw(sub, "THReshold") {
                        if query { Some(Inr::Thr(None)) } else { parse_num_u(a, b"A").map(|v| Inr::Thr(Some(v))) }
                    } else if kw(sub, "CONDition") && query {
                        Some(Inr::Cond)
                    } else if kw(sub, "FETCh") && query {
                        Some(Inr::Fetch)
                    } else if kw(sub, "FILE") && query {
                        Some(Inr::File)
                    } else if kw(sub, "PREamble") && query {
                        Some(Inr::Pre)
                    } else if kw(sub, "DATA") && query {
                        let mut v = [0u32, 3000];
                        for (i, part) in a.split(|&b| b == b',').take(2).enumerate() {
                            if let Some(x) = parse_num(part) {
                                v[i] = x.max(0.0) as u32;
                            }
                        }
                        Some(Inr::Data(v[0], v[1]))
                    } else {
                        None
                    };
                    match c {
                        Some(c) => out = Out::Inr(c),
                        None => {
                            self.err_arg(no_arg, if sub.is_empty() || kw(sub, "STATe") || kw(sub, "THReshold") { "Illegal parameter value" } else { "Undefined header" });
                            continue;
                        }
                    }
                }
                _ if kw(p0, "DECode") => {
                    let sub = parts.get(1).copied().unwrap_or(&[]);
                    let a = trim(arg);
                    let auto = kw(a, "AUTO");
                    let c = if kw(sub, "PROTocol") {
                        if query {
                            Some(Dcd::Proto(None))
                        } else if kw(a, "OFF") {
                            Some(Dcd::Proto(Some(crate::decode::P_OFF)))
                        } else if kw(a, "UART") {
                            Some(Dcd::Proto(Some(crate::decode::P_UART)))
                        } else if kw(a, "MODBus") {
                            Some(Dcd::Proto(Some(crate::decode::P_MODBUS)))
                        } else {
                            None
                        }
                    } else if sub == b"BAUD" {
                        if query {
                            Some(Dcd::Baud(None))
                        } else if auto {
                            Some(Dcd::Baud(Some(0)))
                        } else {
                            parse_num(a).filter(|v| (crate::decode::MIN_BAUD as f32..=crate::decode::MAX_BAUD as f32).contains(v)).map(|v| Dcd::Baud(Some(v as u32)))
                        }
                    } else if kw(sub, "RATE") && query {
                        Some(Dcd::Rate)
                    } else if kw(sub, "FORMat") {
                        if query { Some(Dcd::Fmt(None)) } else { crate::decode::FMT_NAMES.iter().position(|n| a.eq_ignore_ascii_case(n.as_bytes())).map(|i| Dcd::Fmt(Some(i as u8))) }
                    } else if kw(sub, "POLarity") {
                        if query {
                            Some(Dcd::Inv(None))
                        } else if kw(a, "NORMal") {
                            Some(Dcd::Inv(Some(false)))
                        } else if kw(a, "INVerted") {
                            Some(Dcd::Inv(Some(true)))
                        } else {
                            None
                        }
                    } else if kw(sub, "THReshold") {
                        if query {
                            Some(Dcd::Thr(None))
                        } else if auto {
                            Some(Dcd::Thr(Some(f32::NAN)))
                        } else {
                            parse_num_u(a, b"V").filter(|v| v.abs() <= 100.0).map(|v| Dcd::Thr(Some(v)))
                        }
                    } else if kw(sub, "LEVel") && query {
                        Some(Dcd::Level)
                    } else if kw(sub, "MODE") {
                        if query {
                            Some(Dcd::Mode(None))
                        } else if kw(a, "BAND") {
                            Some(Dcd::Mode(Some(0)))
                        } else if kw(a, "LOG") {
                            Some(Dcd::Mode(Some(1)))
                        } else {
                            None
                        }
                    } else if kw(sub, "CLEar") && !query {
                        Some(Dcd::Clear)
                    } else if kw(sub, "COUNt") && query {
                        Some(Dcd::Count)
                    } else if kw(sub, "FRAMes") && query {
                        if a.is_empty() { Some(Dcd::Frames(0)) } else { parse_num(a).filter(|v| *v >= 0.0).map(|v| Dcd::Frames(v as u32)) }
                    } else {
                        None
                    };
                    match c {
                        Some(c) => out = Out::Dcd(c),
                        None => {
                            let known = ["PROTocol", "FORMat", "POLarity", "THReshold", "FRAMes", "MODE"].iter().any(|m| kw(sub, m)) || sub == b"BAUD";

                            self.err_arg(no_arg, if known { "Illegal parameter value" } else { "Undefined header" });
                            continue;
                        }
                    }
                }
                _ if kw(p0, "LOG") => {
                    let sub = parts.get(1).copied().unwrap_or(&[]);
                    let a = trim(arg);
                    // ON|OFF set or the query (Some(None)); None: a bad value.
                    let state = if query { Some(None) } else { on_off(a).map(Some) };
                    out = if kw(sub, "STATe") {
                        match state {
                            Some(v) => Out::Log(Log::State(v)),
                            None => {
                                self.err_arg(no_arg, BAD);
                                continue;
                            }
                        }
                    } else if kw(sub, "SOURce") {
                        if query {
                            Out::Log(Log::Source(None))
                        } else if a.eq_ignore_ascii_case(b"AD2") || a.eq_ignore_ascii_case(b"AD1") {
                            Out::Log(Log::Source(Some(a.eq_ignore_ascii_case(b"AD2"))))
                        } else {
                            self.err_arg(no_arg, "Illegal parameter value");
                            continue;
                        }
                    } else if kw(sub, "SRATe") {
                        match (query, parse_num(a)) {
                            (true, _) => Out::Log(Log::SRate(None)),
                            (false, Some(v)) if v >= 1.0 && v <= 1.0e6 => Out::Log(Log::SRate(Some(v as u32))),
                            (_, v) => {
                                self.err_arg(no_arg, if v.is_some() { "Data out of range" } else { "Illegal parameter value" });
                                continue;
                            }
                        }
                    } else if kw(sub, "GAPs") && query {
                        Out::Log(Log::Gaps)
                    } else if kw(sub, "OSR") {
                        match (query, parse_num(a)) {
                            (true, _) => Out::Log(Log::Osr(None)),
                            (false, Some(v)) if (0.0..=7.0).contains(&v) && v == (v as u8) as f32 => Out::Log(Log::Osr(Some(v as u8))),
                            (_, v) => {
                                self.err_arg(no_arg, if v.is_some_and(|v| v == (v as i32) as f32) { "Data out of range" } else { BAD });
                                continue;
                            }
                        }
                    } else if kw(sub, "CHOP") {
                        match state {
                            Some(v) => Out::Log(Log::Chop(v)),
                            None => {
                                self.err_arg(no_arg, BAD);
                                continue;
                            }
                        }
                    } else if kw(sub, "RATE") && query {
                        Out::Log(Log::Rate)
                    } else if kw(sub, "COUNt") && query {
                        Out::Log(Log::Count)
                    } else if kw(sub, "PREamble") && query {
                        Out::Log(Log::Preamble)
                    } else if kw(sub, "SCALe") && query {
                        Out::Log(Log::Scale)
                    } else if kw(sub, "DATA") && query {
                        Out::Log(Log::Data)
                    } else if kw(sub, "FORMat") {
                        // Argument keywords in any case, short or long form.
                        let am = |m: &str| {
                            let n = m.bytes().take_while(|b| b.is_ascii_uppercase()).count();
                            a.eq_ignore_ascii_case(&m.as_bytes()[..n]) || a.eq_ignore_ascii_case(m.as_bytes())
                        };
                        match query {
                            true => Out::Log(Log::Format(None)),
                            false if am("ROWS") => Out::Log(Log::Format(Some(crate::logger::FMT_ROWS))),
                            false if am("PACKed") => Out::Log(Log::Format(Some(crate::logger::FMT_PACKED))),
                            false if am("DLG") => Out::Log(Log::Format(Some(crate::logger::FMT_DLG))),
                            _ => {
                                self.set_err("Illegal parameter value");
                                continue;
                            }
                        }
                    } else if kw(sub, "CSV") && parts.len() > 2 {
                        match csv_arg(parts[2], a, query) {
                            Ok(l) => Out::Log(l),
                            Err(e) => {
                                self.err_arg(no_arg, e);
                                continue;
                            }
                        }
                    } else if kw(sub, "CSV") {
                        match state {
                            Some(v) => Out::Log(Log::Csv(v)),
                            None => {
                                self.err_arg(no_arg, BAD);
                                continue;
                            }
                        }
                    } else if kw(sub, "DEST") {
                        match (query, a) {
                            (true, _) => Out::Log(Log::Dest(None)),
                            (false, x) if x.eq_ignore_ascii_case(b"DISK") => Out::Log(Log::Dest(Some(true))),
                            (false, x) if x.eq_ignore_ascii_case(b"HOST") => Out::Log(Log::Dest(Some(false))),
                            _ => {
                                self.err_arg(no_arg, "Illegal parameter value");
                                continue;
                            }
                        }
                    } else if kw(sub, "DURation") {
                        match (query, parse_num(a)) {
                            (true, _) => Out::Log(Log::Dur(None)),
                            (false, Some(v)) if (0.0..=1.0e6).contains(&v) => Out::Log(Log::Dur(Some(v as u32))),
                            (_, v) => {
                                self.err_arg(no_arg, if v.is_some() { "Data out of range" } else { "Illegal parameter value" });
                                continue;
                            }
                        }
                    } else if kw(sub, "DISK") && query {
                        Out::Log(Log::Disk)
                    } else {
                        self.err_arg(no_arg, "Undefined header");
                        continue;
                    };
                }
                _ if kw(p0, "MMEMory") => {
                    let sub = parts.get(1).copied().unwrap_or(&[]);
                    out = if kw(sub, "CATalog") && query {
                        Out::Mmem(Mmem::Cat)
                    } else if kw(sub, "DATA") && query {
                        // "<name>"[,<offset>[,<length>]]
                        let mut it = arg.split(|&c| c == b',');
                        let name = it.next().and_then(|n| crate::csvlog::parse_path(trim(n)));
                        let mut num = |d: u32| -> Option<u32> {
                            match it.next() {
                                None => Some(d),
                                Some(t) => core::str::from_utf8(trim(t)).ok()?.parse().ok(),
                            }
                        };
                        match (name, num(0), num(MMEM_DATA_MAX)) {
                            // Another directory's prefix (csvlog::parse_path).
                            (Some((_, false)), Some(_), Some(_)) => {
                                self.set_err(NOT_FOUND);
                                continue;
                            }
                            (Some((n, true)), Some(off), Some(len)) => Out::Mmem(Mmem::Data(n, off, len.min(MMEM_DATA_MAX))),
                            _ => {
                                // A query: `no_arg` is false; no name is -109.
                                self.err_arg(trim(arg).is_empty(), "Illegal parameter value");
                                continue;
                            }
                        }
                    } else if kw(sub, "DELete") && !query {
                        match crate::csvlog::parse_path(trim(arg)) {
                            Some((n, true)) => Out::Mmem(Mmem::Del(n)),
                            // Not the file of the same name in another directory.
                            Some((_, false)) => {
                                self.set_err(NOT_FOUND);
                                continue;
                            }
                            None => {
                                self.err_arg(no_arg, "Illegal parameter value");
                                continue;
                            }
                        }
                    } else {
                        self.err_arg(no_arg, "Undefined header");
                        continue;
                    };
                }
                _ if kw(p0, "WAVeform") => match wav_cmd(parts.get(1).copied().unwrap_or(&[]), trim(arg), query) {
                    Ok(Some(c)) => out = Out::Instr(c),
                    Ok(None) => {
                        if query {
                            self.put(b"CHAN1");
                        }
                    }
                    Err(e) => self.err_arg(no_arg, e),
                },
                _ if kw(p0, "FETCh") && query && parts.len() == 2 && kw(parts[1], "BURDen") => {
                    out = Out::Instr(Cmd::FetchBurden)
                }
                // DISPlay:BURDen, [SENSe:]RESistance:NEGative,
                // MEMory:STATe:RECall:AUTO: ON|OFF / ?.
                _ if (kw(p0, "DISPlay") && parts.len() == 2 && kw(parts[1], "BURDen"))
                    || (kw(p0, "MEMory") && parts.len() == 4 && kw(parts[1], "STATe") && kw(parts[2], "RECall") && kw(parts[3], "AUTO"))
                    || {
                        let r = if kw(p0, "SENSe") { &parts[1..] } else { parts };
                        r.len() == 2 && kw(r[0], "RESistance") && kw(r[1], "NEGative")
                    } =>
                {
                    let v = if query {
                        None
                    } else {
                        match on_off(trim(arg)) {
                            Some(b) => Some(b),
                            None => {
                                self.err_arg(no_arg, "Illegal parameter value");
                                continue;
                            }
                        }
                    };
                    out = Out::Instr(if kw(p0, "DISPlay") {
                        Cmd::DispBurden(v)
                    } else if kw(p0, "MEMory") {
                        Cmd::ModeRemember(v)
                    } else {
                        Cmd::ResNeg(v)
                    });
                }
                _ if kw(p0, "FETCh") && query => out = Out::Fetch,
                // [SENSe:]VOLTage:COUPling / CURRent:COUPling (opendm40).
                _ if coup_cmd(parts).is_some() => {
                    let amp = coup_cmd(parts) == Some(true);
                    if query {
                        out = Out::Instr(Cmd::Coup(amp));
                        continue;
                    }
                    let a = trim(arg);
                    let f = |v, i| if amp { i } else { v };
                    let m = if a == b"DC" {
                        Some(f(1, 8))
                    } else if a == b"AC" {
                        Some(f(0, 5))
                    } else if a == b"ACDC" {
                        Some(f(16, 17))
                    } else if kw(a, "AUTO") {
                        Some(f(crate::dmm::FUNC_V_AUTO, crate::dmm::FUNC_A_AUTO))
                    } else {
                        None
                    };
                    match m {
                        Some(m) => out = Out::Configure(m),
                        None => self.err_arg(no_arg, "Illegal parameter value"),
                    }
                }
                _ if kw(p0, "SENSe") || kw(p0, "FUNCtion") || kw(p0, "TEMPerature") || kw(p0, "RATE") => {
                    let rest = if kw(p0, "SENSe") { &parts[1..] } else { parts };
                    if rest.first().map(|p| kw(p, "RATE")).unwrap_or(false) {
                        let a = trim(arg);
                        let c = match rest.get(1) {
                            Some(l) if kw(l, "PERiod") && query => Some(Cmd::SensRatePer),
                            None if query => Some(Cmd::SensRate(None)),
                            None if kw(a, "SLOW") => Some(Cmd::SensRate(Some(0))),
                            None if kw(a, "MEDium") => Some(Cmd::SensRate(Some(1))),
                            None if kw(a, "FAST") => Some(Cmd::SensRate(Some(2))),
                            _ => None,
                        };
                        match c {
                            Some(c) => out = Out::Instr(c),
                            None => self.err_arg(no_arg, "Illegal parameter value"),
                        }
                        continue;
                    }
                    if rest.first().map(|p| kw(p, "TEMPerature")).unwrap_or(false) {
                        let sub = rest.get(1).copied().unwrap_or(&[]);
                        let a = trim(arg);
                        let c = if kw(sub, "TRANsducer") {
                            match (query, parse_sensor(a)) {
                                (true, _) => Some(Cmd::TempSensor(None)),
                                (false, Some(k)) => Some(Cmd::TempSensor(Some(k))),
                                _ => None,
                            }
                        } else if kw(sub, "OFFSet") {
                            if query { Some(Cmd::TempOffs(None)) } else { parse_num(a).map(|v| Cmd::TempOffs(Some(v))) }
                        } else if kw(sub, "NTC") {
                            let mut it = a.split(|&b| b == b',');
                            match (query, it.next().and_then(parse_num), it.next().and_then(parse_num)) {
                                (true, _, _) => Some(Cmd::TempNtc(None)),
                                (false, Some(r), Some(b)) => Some(Cmd::TempNtc(Some((r, b)))),
                                _ => None,
                            }
                        } else if kw(sub, "EXCitation") && query {
                            Some(Cmd::TempExc)
                        } else {
                            None
                        };
                        match c {
                            Some(c) => out = Out::Instr(c),
                            None => self.err_arg(no_arg, "Illegal parameter value"),
                        }
                        continue;
                    }
                    if rest.first().map(|p| kw(p, "FUNCtion")).unwrap_or(false) {
                        if query {
                            self.put(func_name(cur).as_bytes());
                            self.asked = func_id(cur);
                        } else {
                            let a = trim(arg);
                            let a = a.strip_prefix(b"\"").unwrap_or(a);
                            let a = a.strip_suffix(b"\"").unwrap_or(a);
                            let mut ap: [&[u8]; 3] = [&[]; 3];
                            let mut k = 0;
                            for p in a.split(|&b| b == b':').filter(|p| !p.is_empty()) {
                                if k < 3 {
                                    ap[k] = p;
                                    k += 1;
                                }
                            }
                            match parse_func(&ap[..k]) {
                                Some(f) => out = Out::Configure(f),
                                None => self.err_arg(no_arg, "Illegal parameter value"),
                            }
                        }
                    } else {
                        self.err_arg(no_arg, "Undefined header");
                    }
                }
                _ if kw(p0, "CALCulate") => {
                    let sub = parts.get(1).copied().unwrap_or(&[]);
                    let leaf = parts.get(2).copied();
                    let a = trim(arg);
                    // ON|OFF set or the query: Some(None) = query.
                    let state = if query { Some(None) } else { on_off(a).map(Some) };
                    let c = if kw(sub, "AHOLd") && leaf.is_none() {
                        state.map(Cmd::CalcAhold)
                    } else if kw(sub, "AVERage") {
                        match leaf {
                            None if query => Some(Cmd::CalcAver(2)),
                            Some(l) if query && kw(l, "MINimum") => Some(Cmd::CalcAver(0)),
                            Some(l) if query && kw(l, "MAXimum") => Some(Cmd::CalcAver(1)),
                            Some(l) if query && kw(l, "AVERage") => Some(Cmd::CalcAver(2)),
                            Some(l) if query && kw(l, "COUNt") => Some(Cmd::CalcAver(3)),
                            Some(l) if query && kw(l, "SDEViation") => Some(Cmd::CalcAver(4)),
                            Some(l) if !query && kw(l, "CLEar") => Some(Cmd::CalcAverClear),
                            Some(l) if kw(l, "STATe") => state.map(Cmd::CalcAverState),
                            _ => None,
                        }
                    } else if kw(sub, "SMOothing") {
                        match leaf {
                            Some(l) if kw(l, "RESPonse") => {
                                if query {
                                    Some(Cmd::CalcSmoResp(None))
                                } else if kw(a, "SLOW") {
                                    Some(Cmd::CalcSmoResp(Some(2)))
                                } else if kw(a, "MEDium") || kw(a, "FAST") {
                                    Some(Cmd::CalcSmoResp(Some(1)))
                                } else {
                                    None
                                }
                            }
                            Some(l) if kw(l, "STATe") => state.map(Cmd::CalcSmoState),
                            None => state.map(Cmd::CalcSmoState),
                            _ => None,
                        }
                    } else if kw(sub, "LIMit") {
                        match leaf {
                            Some(l) if kw(l, "LOWer") || kw(l, "UPPer") => {
                                let k = kw(l, "UPPer") as u8;
                                if query { Some(Cmd::CalcLim(k, None)) } else { parse_num(a).map(|v| Cmd::CalcLim(k, Some(v))) }
                            }
                            Some(l) if kw(l, "STATe") => state.map(Cmd::CalcLimState),
                            Some(l) if !query && kw(l, "CLEar") => Some(Cmd::CalcLimClear),
                            Some(l) if query && kw(l, "FAIL") => Some(Cmd::CalcLimFail),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    match c {
                        Some(c) => out = Out::Instr(c),
                        None if !query && !a.is_empty() => self.err_arg(no_arg, "Illegal parameter value"),
                        None => self.err_arg(no_arg, "Undefined header"),
                    }
                }
                _ => self.err_arg(no_arg, "Undefined header"),
            }
        }
        match out {
            Out::None if self.len > 0 => Out::Text,
            o => o,
        }
    }

    /// Append raw bytes to the reply.
    pub fn put_bytes(&mut self, b: &[u8]) {
        self.put(b);
    }

    /// Queue an error for SYSTem:ERRor? (FIFO; a full queue keeps its
    /// oldest entries and its newest becomes "Queue overflow").
    pub fn set_err(&mut self, e: &'static str) {
        let n = self.errn as usize;
        if n < ERRQ {
            self.errq[n] = e;
            self.errn += 1;
        } else {
            self.errq[ERRQ - 1] = QUEUE_OVERFLOW;
        }
    }

    /// `set_err` in the parser: a parameter error of a set command that had
    /// no argument is "Missing parameter" (-109).
    fn err_arg(&mut self, no_arg: bool, e: &'static str) {
        self.set_err(if no_arg && (e == BAD || e == RANGE) { MISSING } else { e });
    }

    /// The oldest queued error, removed.
    fn pop_err(&mut self) -> Option<&'static str> {
        if self.errn == 0 {
            return None;
        }
        let e = self.errq[0];
        self.errq.copy_within(1.., 0);
        self.errn -= 1;
        Some(e)
    }


    /// Append a number in NR3.
    pub fn put_f32(&mut self, v: f32) {
        // No value (a scope measurement without a result): SCPI NaN, the
        // reference's "no value" (nr3 printed the NaN as +0.000000E-03).
        if v.is_nan() {
            self.put(NAN);
            return;
        }
        let mut b = [0u8; 20];
        let n = nr3(&mut b, v);
        self.put(&b[..n]);
    }

    /// CONFigure? reply for stock mode `mode` (Scpi::handle `cur`), its
    /// range (`range_si`; None: from the readout, 60000 counts of it) and
    /// the latest reading of it: "\"VOLT +6.000000E+00,+1.000000E-04\"".
    /// The resolution is an exact power of ten (hosts take its exponent as
    /// the digits): the readout's, one decimal more with the +1 digit.
    pub fn put_conf(&mut self, mode: u8, range: Option<f32>, r: Option<&Reading>) {
        self.put_conf_fmt(mode, range, r.map(|r| (r.main.unit, r.main.dec, r.main.ext)));
    }

    /// The function `mode` was named to the host (FUNCtion? / CONFigure?):
    /// `take_asked`.
    pub fn note_asked(&mut self, mode: u8) {
        self.asked = func_id(mode);
    }

    /// `put_conf` from the readout format (unit, decimals, extra digit)
    /// instead of a reading: the Wave view, where the DMM poll is paused
    /// and the last reading is the one from before it (another range).
    pub fn put_conf_fmt(&mut self, mode: u8, range: Option<f32>, f: Option<(&str, u8, bool)>) {
        self.asked = func_id(mode);
        let name = func_name(mode).as_bytes();
        self.put(&name[..name.len() - 1]);
        if matches!(mode, 11 | 12) {
            self.put(b"\"");
            return;
        }
        // Resolution exponent of the readout: -decimals + the unit prefix.
        let (exp, ext) = match f {
            Some((unit, dec, ext)) => {
                let pfx = match unit.chars().next() {
                    Some('m') => -3,
                    Some('\u{b5}') | Some('u') => -6,
                    Some('n') => -9,
                    Some('k') => 3,
                    Some('M') => 6,
                    _ => 0,
                };
                let pfx = if unit.chars().nth(1).is_some() { pfx } else { 0 };
                (pfx - dec as i32, ext as i32)
            }
            None => (range.map_or(-4, |v| step_exp(v, mode == 13)), 0),
        };
        fn pow10(s: &mut Scpi, m: u8, e: i32) {
            s.put(&[b'+', m, b'.', b'0', b'0', b'0', b'0', b'0', b'0', b'E', if e < 0 { b'-' } else { b'+' }]);
            let a = e.unsigned_abs();
            s.put(&[b'0' + (a / 10 % 10) as u8, b'0' + (a % 10) as u8]);
        }
        self.put(b" ");
        match range {
            Some(v) => self.put_f32(v),
            None => pow10(self, b'6', exp + 4),
        }
        self.put(b",");
        pow10(self, b'1', exp - ext);
        self.put(b"\"");
    }

    /// Format a reading as SCPI NR3 (+1.234567E+00), or the overload value.
    pub fn put_reading(&mut self, r: &Reading) {
        if r.main.ol {
            self.put(b"+9.90000000E+37");
            return;
        }
        let mut b = [0u8; 20];
        let n = nr3(&mut b, r.si);
        self.put(&b[..n]);
    }
}

fn trim(s: &[u8]) -> &[u8] {
    let a = s.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(s.len());
    let e = s.iter().rposition(|b| !b.is_ascii_whitespace()).map(|i| i + 1).unwrap_or(a);
    &s[a..e.max(a)]
}

/// SCPI NR3 with 7 significant digits: "+1.234567E-03".
fn nr3(b: &mut [u8; 20], v: f32) -> usize {
    let mut i = 0;
    b[i] = if v < 0.0 { b'-' } else { b'+' };
    i += 1;
    let x = if v < 0.0 { -v } else { v };
    // The exponent first (approximate), then the 7 digits from `x` scaled
    // by 10^(6 - e) in f64 (exact for an f32 times 10^n, n <= 22) with one
    // rounding, so an exact value such as 76 800 prints exactly
    // (+7.680000E+04). The former repeated f32 `x /= 10.0` rounded at
    // every step (+7.680001E+04, device 2f56ff9).
    let mut e: i32 = 0;
    let mut m = 0u32;
    if x != 0.0 {
        let mut y = x;
        while y >= 10.0 && e < 40 {
            y /= 10.0;
            e += 1;
        }
        while y < 1.0 && e > -46 {
            y *= 10.0;
            e -= 1;
        }
        // The estimate can be one off near a power of ten.
        for _ in 0..3 {
            m = nr3_scale(x, 6 - e);
            if m >= 10_000_000 {
                e += 1;
            } else if m < 1_000_000 {
                e -= 1;
            } else {
                break;
            }
        }
        // 9 999 999 estimated as E+07 rounds up to 1.000000: one more digit.
        if m == 1_000_000 {
            let m2 = nr3_scale(x, 7 - e);
            if m2 < 10_000_000 {
                m = m2;
                e -= 1;
            }
        }
        // Not finite (no reading gives one; an overload is put_reading's
        // 9.9E+37): digits within 0..9 whatever the bits.
        m = m.min(9_999_999);
    }
    let digits = [m / 1_000_000, m / 100_000 % 10, m / 10_000 % 10, m / 1000 % 10, m / 100 % 10, m / 10 % 10, m % 10];
    b[i] = b'0' + digits[0] as u8;
    i += 1;
    b[i] = b'.';
    i += 1;
    for d in &digits[1..] {
        b[i] = b'0' + *d as u8;
        i += 1;
    }
    b[i] = b'E';
    i += 1;
    b[i] = if e < 0 { b'-' } else { b'+' };
    i += 1;
    let ea = e.unsigned_abs();
    b[i] = b'0' + (ea / 10 % 10) as u8;
    b[i + 1] = b'0' + (ea % 10) as u8;
    i + 2
}

/// `x * 10^n` rounded to an integer, with one rounding: in f64, where
/// 10^n by repeated multiplication is exact for |n| <= 22 (a negative `n`
/// divides by 10^-n). For nr3's 7 digits. The rounding adds 2^52 and takes
/// the low mantissa bits (no f64 compare / convert helpers: region B).
fn nr3_scale(x: f32, n: i32) -> u32 {
    let mut p = 1.0f64;
    for _ in 0..n.unsigned_abs().min(60) {
        p *= 10.0;
    }
    let x = x as f64;
    let y = if n >= 0 { x * p } else { x / p };
    (y + 4_503_599_627_370_496.0).to_bits() as u32
}
