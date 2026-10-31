//! Host test of src/scpi.rs (Scpi::handle and parse_num_u). The crate
//! modules scpi.rs refers to are stubs here; the vdisk DBG block is left out
//! (no `vdisk` feature). run.sh builds and runs it.

#[path = "../../src/ident.rs"]
mod ident;
#[allow(dead_code)]
#[path = "../../src/pack10.rs"]
#[allow(dead_code)]
mod pack10;
#[allow(dead_code)]
#[path = "../../src/decode.rs"]
mod decode;
#[path = "../../src/scpi.rs"]
mod scpi;
mod hy3131 {
    pub const CFG_LEN: usize = 22;
}
#[path = "../../src/hy3131_tables.rs"]
mod hy3131_tables;
#[path = "../../src/dbgsrc.rs"]
mod dbgsrc;
#[path = "../../src/touchcal.rs"]
mod touchcal;

mod boot {
    pub fn put_info(_s: &mut crate::scpi::Scpi) {}
    pub fn enter_loader_update() -> bool {
        false
    }
    pub fn request_loader() {}
}
mod clock {
    pub const HCLK_HZ: u32 = 200_000_000;
}
mod power {
    pub struct ChgLog {
        pub n: u32,
        pub ev: [u32; 8],
    }
    pub static mut CHG_LOG: ChgLog = ChgLog { n: 0, ev: [0; 8] };
    pub fn dbg_state() -> [u32; 8] {
        [0; 8]
    }
    pub fn dbg_pin(_pin: u32, _mode: u32) -> bool {
        true
    }
}
mod csvlog {
    pub const INTERVALS_S: [u32; 4] = [0, 1, 10, 60];
    pub const DURATIONS_S: [u32; 5] = [0, 600, 3600, 8 * 3600, 24 * 3600];
    pub const ON_CHANGE: u8 = 4;
    pub const CHANGES: [&str; 3] = ["0.1", "1", "5"];
    pub fn parse_path(s: &[u8]) -> Option<([u8; 11], bool)> {
        // "WRONG/..." stands for a prefix of another directory.
        if s.is_empty() { None } else { Some(([b' '; 11], !s.starts_with(b"\"WRONG/"))) }
    }
}
mod danger {
    pub const LOUD: u8 = 0;
    pub const VOLUME: u8 = 1;
    pub const OFF: u8 = 2;
}
mod dmm {
    pub const FUNC_V_AUTO: u8 = 19;
    pub const FUNC_A_AUTO: u8 = 20;
    pub const T_NAMES: [&str; 7] = ["TCK", "PT100", "PT1000", "NTC10K3950", "NTC10K3435", "NTC100K3950", "NTCUSER"];
    pub struct Val {
        pub dec: u8,
        pub unit: &'static str,
        pub ol: bool,
        pub ext: bool,
    }
    pub struct Reading {
        pub main: Val,
        pub si: f32,
    }
    pub mod pw {
        pub const FUNC: u8 = 18;
        pub const V_LIM: [f32; 4] = [6.0, 60.0, 600.0, 1000.0];
        pub const I_LIM: [f32; 6] = [600e-6, 6e-3, 0.06, 0.6, 6.0, 10.0];
        pub const R34_XY: u8 = 0x88;
        pub const R34_XX: u8 = 0x80;
        pub const R34_YY: u8 = 0x8C;
        pub const T_LIM_DEFAULT: u16 = 30;
        pub const COUP_ACDC: u8 = 0;
        pub const COUP_DC: u8 = 1;
        pub const COUP_AC: u8 = 3;
    }
}
mod logger {
    pub const FMT_ROWS: u8 = 0;
    pub const FMT_PACKED: u8 = 1;
    pub const FMT_DLG: u8 = 2;
}
mod hyspi {
    pub const N_MAX: u32 = 2000;
}
mod sentinel {
    pub const M_ALL: u8 = 15;
    pub static QUES_EVENT: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(0);
}
mod uid {
    pub fn bytes() -> &'static [u8; 24] {
        b"000102030405060708090A0B"
    }
}

use scpi::{parse_num, parse_num_u, Cmd, Log, Mmem, Out, Rng, Scpi};

static mut FAILS: u32 = 0;

fn check(ok: bool, what: &str) {
    if ok {
        println!("ok   {what}");
    } else {
        println!("FAIL {what}");
        unsafe { FAILS += 1 };
    }
}

fn close(a: Option<f32>, b: f32) -> bool {
    a.is_some_and(|a| (a - b).abs() <= b.abs() * 1e-6)
}

/// The reply text of `msg` ("" when none).
fn reply(s: &mut Scpi, msg: &str) -> String {
    let _ = s.handle(msg.as_bytes(), 1);
    String::from_utf8_lossy(&s.buf[..s.len]).into_owned()
}

fn err(s: &mut Scpi) -> String {
    reply(s, "SYST:ERR?")
}

fn suffixes() {
    let cases: &[(&str, &[u8], Option<f32>)] = &[
        ("1", b"", Some(1.0)),
        ("1m", b"", Some(1e-3)),
        ("1M", b"", Some(1e-3)),
        ("1MA", b"", Some(1e6)),
        ("1ma", b"", Some(1e6)),
        ("2.5k", b"", Some(2.5e3)),
        ("2.5K", b"", Some(2.5e3)),
        ("3u", b"", Some(3e-6)),
        ("3U", b"", Some(3e-6)),
        ("4n", b"", Some(4e-9)),
        ("1G", b"", Some(1e9)),
        ("7p", b"", Some(7e-12)),
        ("-1.5k", b"", Some(-1.5e3)),
        ("2e-3", b"", Some(2e-3)),
        ("2E+3", b"", Some(2e3)),
        ("1 k", b"", Some(1e3)),
        ("1x", b"", None),
        ("1V", b"", None),
        ("1E", b"", None),
        ("1E99999", b"", None),
        ("", b"", None),
        ("k", b"", None),
        ("1MHZ", b"HZ", Some(1e6)),
        ("1mhz", b"HZ", Some(1e6)),
        ("1MAHZ", b"HZ", Some(1e6)),
        ("1KHZ", b"HZ", Some(1e3)),
        ("1khz", b"HZ", Some(1e3)),
        ("50HZ", b"HZ", Some(50.0)),
        ("1M", b"HZ", Some(1e-3)),
        ("1MA", b"HZ", Some(1e6)),
        ("2.5 MHz", b"HZ", Some(2.5e6)),
        ("1V", b"HZ", None),
        ("1MOHM", b"OHM", Some(1e6)),
        ("1mohm", b"OHM", Some(1e6)),
        ("10KOHM", b"OHM", Some(1e4)),
        ("60OHM", b"OHM", Some(60.0)),
        ("5MA", b"A", Some(5e-3)),
        ("5ma", b"A", Some(5e-3)),
        ("5MAA", b"A", Some(5e6)),
        ("600UA", b"A", Some(600e-6)),
        ("5A", b"A", Some(5.0)),
        ("100 mV", b"V", Some(0.1)),
        ("100MV", b"V", Some(0.1)),
        ("6V", b"V", Some(6.0)),
        ("100US", b"S", Some(1e-4)),
        ("2MS", b"S", Some(2e-3)),
        ("50PCT", b"PCT", Some(50.0)),
    ];
    for &(t, u, want) in cases {
        let got = parse_num_u(t.as_bytes(), u);
        let ok = match want {
            Some(w) => close(got, w),
            None => got.is_none(),
        };
        check(ok, &format!("parse_num_u({t:?}, {:?}) = {got:?}, want {want:?}", String::from_utf8_lossy(u)));
    }
    check(close(parse_num(b"1m"), 1e-3), "parse_num(\"1m\") is milli (IEEE 488.2), not mega");
}

fn set_commands() {
    let mut s = Scpi::new();
    // Good values.
    check(matches!(s.handle(b"SOUR:FREQ 1KHZ", 1), Out::Instr(Cmd::GenFreq(Some(v))) if v == 1000.0), "SOUR:FREQ 1KHZ");
    check(matches!(s.handle(b"sour:freq 2.5mhz", 1), Out::Instr(Cmd::GenFreq(Some(v))) if v == 2.5e6), "sour:freq 2.5mhz = 2.5 MHz");
    check(matches!(s.handle(b"SOUR:FREQ 1M", 1), Out::Instr(Cmd::GenFreq(Some(v))) if (v - 1e-3).abs() < 1e-9), "SOUR:FREQ 1M = 1 mHz");
    check(matches!(s.handle(b"SOUR:FREQ 1MA", 1), Out::Instr(Cmd::GenFreq(Some(v))) if v == 1e6), "SOUR:FREQ 1MA = 1 MHz");
    check(matches!(s.handle(b"SOUR:FREQ?", 1), Out::Instr(Cmd::GenFreq(None))), "SOUR:FREQ? is the query");
    check(matches!(s.handle(b"SOUR:VOLT 500MV", 1), Out::Instr(Cmd::GenVolt(Some(v))) if (v - 0.5).abs() < 1e-6), "SOUR:VOLT 500MV");
    check(matches!(s.handle(b"SOUR:VOLT:OFFS AUTO", 1), Out::Instr(Cmd::GenOffs(Some(v))) if v.is_nan()), "SOUR:VOLT:OFFS AUTO");
    check(matches!(s.handle(b"SOUR:FUNC:SQU:DCYC 25", 1), Out::Instr(Cmd::GenDuty(Some(v))) if v == 25.0), "SOUR:FUNC:SQU:DCYC 25");
    check(matches!(s.handle(b"SOUR:FUNC tri", 1), Out::Instr(Cmd::GenFunc(Some(2)))), "SOUR:FUNC tri");
    check(matches!(s.handle(b"OUTP ON", 1), Out::Instr(Cmd::GenOut(Some(true)))), "OUTP ON");
    check(matches!(s.handle(b"TIM:SCAL 100US", 1), Out::Instr(Cmd::TimScale(Some(v))) if (v - 1e-4).abs() < 1e-9), "TIM:SCAL 100US");
    check(matches!(s.handle(b"TIM:MAIN:SCAL 1m", 1), Out::Instr(Cmd::TimScale(Some(v))) if (v - 1e-3).abs() < 1e-9), "TIM:MAIN:SCAL 1m");
    check(matches!(s.handle(b"CHAN1:SCAL 0.5", 1), Out::Instr(Cmd::ChScale(Some(v))) if v == 0.5), "CHAN1:SCAL 0.5");
    check(matches!(s.handle(b"CHAN:COUP AC", 1), Out::Instr(Cmd::ChCoup(Some(true)))), "CHAN:COUP AC");
    check(matches!(s.handle(b"TRIG:SLOP NEG", 1), Out::Instr(Cmd::TrigSlope(Some(false)))), "TRIG:SLOP NEG");
    check(matches!(s.handle(b"TRIG:EDGE:SLOP POS", 1), Out::Instr(Cmd::TrigSlope(Some(true)))), "TRIG:EDGE:SLOP POS");
    check(matches!(s.handle(b"TRIG:EDGE:LEV 1.5", 1), Out::Instr(Cmd::TrigLevel(Some(v))) if v == 1.5), "TRIG:EDGE:LEV 1.5");
    check(matches!(s.handle(b"TRIG:SWE NORM", 1), Out::Instr(Cmd::TrigSweep(Some(1)))), "TRIG:SWE NORM");
    check(matches!(s.handle(b"TRIG:SOUR CHAN1", 1), Out::Instr(Cmd::TrigSource(Some(0)))), "TRIG:SOUR CHAN1 (short form with its suffix)");
    check(matches!(s.handle(b"TRIG:SOUR chan", 1), Out::Instr(Cmd::TrigSource(Some(0)))), "TRIG:SOUR chan");
    check(matches!(s.handle(b"TRIG:EDGE:SOUR CHANnel1", 1), Out::Instr(Cmd::TrigSource(Some(0)))), "TRIG:EDGE:SOUR CHANnel1");
    check(matches!(s.handle(b"TRIG:SOUR GEN", 1), Out::Instr(Cmd::TrigSource(Some(1)))), "TRIG:SOUR GEN");
    check(matches!(s.handle(b"RES:NEG ON", 1), Out::Instr(Cmd::ResNeg(Some(true)))), "RES:NEG ON");
    check(matches!(s.handle(b"SENS:RESistance:NEGative off", 1), Out::Instr(Cmd::ResNeg(Some(false)))), "SENS:RESistance:NEGative off");
    check(matches!(s.handle(b"RES:NEG?", 1), Out::Instr(Cmd::ResNeg(None))), "RES:NEG? is the query");
    check(matches!(s.handle(b"DISP:BURD 1", 1), Out::Instr(Cmd::DispBurden(Some(true)))), "DISP:BURD 1");
    check(matches!(s.handle(b"MEM:STAT:REC:AUTO ON", 1), Out::Instr(Cmd::ModeRemember(Some(true)))), "MEM:STAT:REC:AUTO ON");
    check(matches!(s.handle(b"MEMory:STATe:RECall:AUTO 0", 1), Out::Instr(Cmd::ModeRemember(Some(false)))), "MEMory:STATe:RECall:AUTO 0");
    check(matches!(s.handle(b"MEM:STAT:REC:AUTO?", 1), Out::Instr(Cmd::ModeRemember(None))), "MEM:STAT:REC:AUTO? is the query");    check(matches!(s.handle(b"VOLT:RANG 600MV", 1), Out::Instr(Cmd::Range(_, Rng::Value(v))) if (v - 0.6).abs() < 1e-6), "VOLT:RANG 600MV");
    check(matches!(s.handle(b"CURR:RANG 6MA", 1), Out::Instr(Cmd::Range(_, Rng::Value(v))) if (v - 6e-3).abs() < 1e-9), "CURR:RANG 6MA = 6 mA");
    check(matches!(s.handle(b"RES:RANG 6MOHM", 1), Out::Instr(Cmd::Range(_, Rng::Value(v))) if v == 6e6), "RES:RANG 6MOHM = 6 Mohm");
    check(matches!(s.handle(b"CONF:VOLT:DC 60mV", 1), Out::Instr(Cmd::Conf(_, Rng::Value(v), None)) if (v - 0.06).abs() < 1e-6), "CONF:VOLT:DC 60mV");
    // The experimental 600 Mohm range (dmm::OHM600, ohm r7): RES:RANG /
    // CONF:RES above 60 Mohm select it (app.rs scpi_range: the smallest item
    // that holds the value); MAX is the item before it, 60 Mohm.
    check(matches!(s.handle(b"RES:RANG 600E6", 1), Out::Instr(Cmd::Range(10, Rng::Value(v))) if v == 600e6), "RES:RANG 600E6");
    check(matches!(s.handle(b"CONF:RES 600E6", 1), Out::Instr(Cmd::Conf(10, Rng::Value(v), None)) if v == 600e6), "CONF:RES 600E6");
    check(matches!(s.handle(b"RES:RANG MAX", 1), Out::Instr(Cmd::Range(10, Rng::Value(v))) if v.is_infinite()), "RES:RANG MAX");
    {
        let items = scpi::range_items(10);
        // app.rs scpi_range: MAX = the top item, on ohm the one below 600 Mohm.
        let pick = |x: f32| if x.is_infinite() { items.len().checked_sub(2) } else { items.iter().position(|&it| it >= x * 0.9999) };
        let code = |x: f32| pick(x).map(|i| scpi::range_item_code(10, i));
        check(code(600e6) == Some(7) && code(100e6) == Some(7) && code(60.1e6) == Some(7), "RES:RANG 100E6 .. 600E6: r7 (600 Mohm)");
        check(code(60e6) == Some(6) && code(f32::INFINITY) == Some(6) && code(0.0) == Some(0), "RES:RANG 60E6 / MAX: 60 Mohm, MIN: 60 ohm");
        check(code(601e6).is_none(), "RES:RANG 601E6: out of range");
        check(scpi::range_si(10, 7) == Some(600e6), "RANG? on 600 Mohm");
    }
    check(matches!(s.handle(b"DBG:OHM6?", 0), Out::Dbg(scpi::Dbg::Ohm6(None))), "DBG:OHM6?");
    check(
        matches!(s.handle(b"DBG:OHM6 1.007,,3", 0), Out::Dbg(scpi::Dbg::Ohm6(Some(v))) if v[0] == 1.007f32 && v[1].is_nan() && v[2] == 3.0 && v[4].is_nan()),
        "DBG:OHM6 1.007,,3: empty fields NaN",
    );
    check(err(&mut s) == "0,\"No error\"", "no error after the good values");

    // Bad values: no reply, no Cmd (the setting kept), an error.
    let bad: &[(&str, &str)] = &[
        ("SOUR:FREQ abc", "-224"),
        ("SOUR:FREQ", "-109"),
        ("SOUR:FREQ -5", "-222"),
        ("SOUR:FREQ 0", "-222"),
        ("SOUR:FREQ 1V", "-224"),
        ("SOUR:VOLT x", "-224"),
        ("SOUR:VOLT -1", "-222"),
        ("SOUR:VOLT:OFFS x", "-224"),
        ("SOUR:FUNC:SQU:DCYC 150", "-222"),
        ("POW:PHAS 200", "-222"),
        ("POW:PHAS x", "-224"),
        ("SOUR:FUNC NOISE", "-224"),
        ("SOUR:FUNC", "-109"),
        ("OUTP MAYBE", "-224"),
        ("OUTP", "-109"),
        ("TIM:SCAL fast", "-224"),
        ("TIM:SCAL 0", "-222"),
        ("TIM:POS 1", "-113"),
        ("CHAN1:SCAL x", "-224"),
        ("CHAN1:COUP GND", "-224"),
        ("CHAN1:OFFS x", "-224"),
        ("CHAN1:DISP ON", "-113"),
        ("TRIG:SLOP UP", "-224"),
        ("TRIG:SWE NEVER", "-224"),
        ("TRIG:SOUR CHAN2", "-224"),
        ("TRIG:SOUR CHANNEL", "-224"),
        ("TRIG:LEV x", "-224"),
        ("TRIG:LEV", "-109"),
        ("LOG:STAT MAYBE", "-224"),
        ("LOG:OSR 9", "-222"),
        ("LOG:OSR 2.5", "-224"),
        ("LOG:OSR x", "-224"),
        ("LOG:DUR -5", "-222"),
        ("LOG:DUR x", "-224"),
        ("LOG:SRAT 0", "-222"),
        ("LOG:CSV:INT 5", "-222"),
        ("LOG:CSV:INT x", "-224"),
        ("LOG:CSV:DUR 7", "-222"),
        ("LOG:CSV:CHAN 2", "-222"),
        ("LOG:CSV:CHAN x", "-224"),
        ("LOG:CSV:INT CHANNEL", "-224"),
        ("LOG:SOUR AD3", "-224"),
        ("SYST:BEEP:VOL 101", "-222"),
        ("SYST:BEEP:VOL -1", "-222"),
        ("SYST:BEEP:VOL x", "-224"),
        ("SYST:BEEP:VOL", "-109"),
        ("SYST:ALER:ENAB 16", "-222"),
        ("SYST:ALER:ENAB 2.5", "-224"),
        ("MMEM:DATA?", "-109"),
        ("LOG:CHOP 2", "-224"),
        ("LOG:CSV x", "-224"),
        ("VOLT:RANG x", "-224"),
        ("VOLT:RANG", "-109"),
        ("FOO:BAR 1", "-113"),
        ("*IDN? 5", "-108"),
        ("*CLS 1", "-108"),
        ("READ? 2", "-108"),
        ("SOUR#FREQ 5", "-102"),
        ("SYST:ERR??", "-102"),
    ];
    for &(m, code) in bad {
        let o = s.handle(m.as_bytes(), 1);
        let none = matches!(o, Out::None) && s.len == 0;
        let e = err(&mut s);
        check(none && e.starts_with(&format!("{code},\"")), &format!("{m:?}: no reply / Cmd ({none}), SYST:ERR? {e:?} (want {code})"));
    }
    let long = format!("SOUR:FREQ {}", "1".repeat(70));
    let o = s.handle(long.as_bytes(), 1);
    let e = err(&mut s);
    check(matches!(o, Out::None) && e.starts_with("-102,"), &format!("a command over 64 bytes: refused, {e:?}"));
    // A bad value inside a message: the rest still runs.
    let o = s.handle(b"SOUR:FREQ x;SOUR:FREQ?", 1);
    check(matches!(o, Out::Instr(Cmd::GenFreq(None))) && err(&mut s).starts_with("-224,"), "SOUR:FREQ x;SOUR:FREQ?: the query runs, -224 queued");
    check(matches!(s.handle(b"LOG:STAT ON", 1), Out::Log(Log::State(Some(true)))), "LOG:STAT ON");
    // Record page On change (csvlog.rs on-change mode).
    check(matches!(s.handle(b"LOG:CSV:INT CHAN", 1), Out::Log(Log::CsvInt(Some(4)))), "LOG:CSV:INT CHAN: On change");
    check(matches!(s.handle(b"log:csv:interval change", 1), Out::Log(Log::CsvInt(Some(4)))), "log:csv:interval change");
    check(matches!(s.handle(b"LOG:CSV:INT 10", 1), Out::Log(Log::CsvInt(Some(2)))), "LOG:CSV:INT 10");
    check(matches!(s.handle(b"LOG:CSV:DUR 3600", 1), Out::Log(Log::CsvDur(Some(2)))), "LOG:CSV:DUR 3600");
    check(matches!(s.handle(b"LOG:CSV:CHAN 0.1", 1), Out::Log(Log::CsvChg(Some(0)))), "LOG:CSV:CHAN 0.1");
    check(matches!(s.handle(b"LOG:CSV:CHANge 5", 1), Out::Log(Log::CsvChg(Some(2)))), "LOG:CSV:CHANge 5");
    check(matches!(s.handle(b"LOG:CSV:CHAN?", 1), Out::Log(Log::CsvChg(None))), "LOG:CSV:CHAN? is the query");
    check(matches!(s.handle(b"LOG:CSV:FREE?", 1), Out::Log(Log::CsvFree)), "LOG:CSV:FREE?");
    check(matches!(s.handle(b"LOG:SOUR ad2", 1), Out::Log(Log::Source(Some(true)))), "LOG:SOUR ad2: parameters are not case-sensitive");
    check(matches!(s.handle(b"log:sour Ad1", 1), Out::Log(Log::Source(Some(false)))), "log:sour Ad1");
}

fn error_queue() {
    let mut s = Scpi::new();
    check(err(&mut s) == "0,\"No error\"", "empty queue: 0,\"No error\"");
    check(reply(&mut s, "SYST:ERR:COUN?") == "0", "SYST:ERR:COUN? 0");
    s.set_err("Settings conflict; detail text");
    s.set_err("Data stale");
    s.set_err("Undefined header");
    check(reply(&mut s, "SYST:ERR:COUN?") == "3", "SYST:ERR:COUN? 3");
    check(err(&mut s) == "-221,\"Settings conflict; detail text\"", "FIFO 1: -221 with its detail");
    check(err(&mut s) == "-230,\"Data stale\"", "FIFO 2: -230");
    check(err(&mut s) == "-113,\"Undefined header\"", "FIFO 3: -113");
    check(err(&mut s) == "0,\"No error\"", "then empty");
    for _ in 0..12 {
        s.set_err("Command error");
    }
    check(reply(&mut s, "SYST:ERR:COUN?") == scpi::ERRQ.to_string(), "overflow: the queue holds ERRQ entries");
    for i in 0..scpi::ERRQ - 1 {
        let e = err(&mut s);
        check(e == "-100,\"Command error\"", &format!("overflow: entry {i} kept ({e})"));
    }
    check(err(&mut s) == "-350,\"Queue overflow\"", "overflow: the newest entry is -350");
    check(err(&mut s) == "0,\"No error\"", "overflow: then empty");
    s.set_err("Data out of range");
    s.set_err("Mass storage busy");
    let _ = s.handle(b"*RST", 1);
    check(reply(&mut s, "SYST:ERR:COUN?") == "2", "*RST keeps the queue");
    let _ = s.handle(b"*CLS", 1);
    check(err(&mut s) == "0,\"No error\"", "*CLS clears the queue");
    let codes: &[(&str, i16)] = &[
        ("Command error", -100),
        ("Syntax error", -102),
        ("Parameter not allowed", -108),
        ("Missing parameter", -109),
        ("Undefined header", -113),
        ("Execution error", -200),
        ("Command protected", -203),
        ("Settings conflict", -221),
        ("Data out of range", -222),
        ("Too much data", -223),
        ("Illegal parameter value", -224),
        ("Data stale", -230),
        ("Queue overflow", -350),
        ("Query error", -400),
        ("Query INTERRUPTED", -410),
        ("File name not found", -256),
        ("Upload not started", -200),
    ];
    for &(m, c) in codes {
        check(scpi::err_code(m) == c, &format!("err_code({m:?}) = {} (want {c})", scpi::err_code(m)));
    }
}

/// `Scpi::put_f32` (nr3) of `v`.
fn nr3(v: f32) -> String {
    let mut s = Scpi::new();
    s.len = 0;
    s.put_f32(v);
    String::from_utf8_lossy(&s.buf[..s.len]).into_owned()
}

/// nr3 prints exact values exactly (device 2f56ff9: ACQ:SRAT? 76 800 was
/// "+7.680001E+04"), and any other value within one unit of the 7th digit
/// of the correctly rounded decimal.
fn nr3_exact() {
    for (v, want) in [
        (76800.0f32, "+7.680000E+04"),
        (38400.0, "+3.840000E+04"),
        (4800.0, "+4.800000E+03"),
        (9600.0, "+9.600000E+03"),
        (1.0, "+1.000000E+00"),
        (10.0, "+1.000000E+01"),
        (6.0, "+6.000000E+00"),
        (600.0, "+6.000000E+02"),
        (1000.0, "+1.000000E+03"),
        (1.0e6, "+1.000000E+06"),
        (4_915_200.0, "+4.915200E+06"),
        (0.5, "+5.000000E-01"),
        (0.25, "+2.500000E-01"),
        (-76800.0, "-7.680000E+04"),
        (0.0, "+0.000000E+00"),
        (9_999_999.0, "+9.999999E+06"),
        (0.001, "+1.000000E-03"),
        (1.0e-6, "+1.000000E-06"),
        (6.0e-4, "+6.000000E-04"),
    ] {
        let r = nr3(v);
        check(r == want, &format!("nr3 {v} = {r} (want {want})"));
    }
    // No value (a scope measurement without a result, #79): SCPI NaN.
    let r = nr3(f32::NAN);
    check(r == "+9.91000000E+37", &format!("nr3 NaN = {r} (want +9.91000000E+37)"));
    // A sweep against Rust's correctly rounded {:.6E}.
    let mut bad = 0;
    let mut seed: u32 = 12345;
    for k in 0..200_000u32 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let v = if k % 2 == 0 {
            f32::from_bits((seed >> 9) | (((seed % 70) + 97) << 23)) // 1e-9 .. 1e12
        } else {
            (seed % 2_000_000) as f32
        };
        let r = nr3(v);
        let w = format!("{:.6E}", v as f64);
        let (rm, re) = r[1..].split_once('E').unwrap();
        let (wm, we) = w.split_once('E').unwrap();
        let rm: f64 = rm.parse().unwrap();
        let wm: f64 = wm.parse().unwrap();
        let rv = rm * 10f64.powi(re.parse().unwrap());
        let wv = wm * 10f64.powi(we.parse().unwrap());
        let ulp = 10f64.powi(we.parse::<i32>().unwrap() - 6);
        if (rv - wv).abs() > ulp * 1.01 || !(1.0..10.0).contains(&rm) {
            if bad < 5 {
                println!("     nr3 {v:e}: {r} vs {w}");
            }
            bad += 1;
        }
    }
    check(bad == 0, &format!("nr3 sweep: {bad} of 200000 off by more than one digit"));
}

/// `pow10f` (VOLT:RES? = 10^step_exp, the RES set threshold) is the f32
/// nearest 10^e, so nr3 prints it exactly (device 1a11eda: VOLT:RES? on
/// DC V 6 V MED "+9.999999E-06", from an f32 divided by 10 five times).
fn pow10f_exact() {
    for e in -15..=6 {
        let v = scpi::pow10f(e);
        let want: f32 = format!("1e{e}").parse().unwrap();
        check(v == want, &format!("pow10f({e}) = {v:e} (want {want:e})"));
        let r = nr3(v);
        let w = format!("+1.000000E{}{:02}", if e < 0 { '-' } else { '+' }, e.abs());
        check(r == w, &format!("nr3(pow10f({e})) = {r} (want {w})"));
    }
    // Every range's resolution as VOLT:RES? computes it (the +1 digit too).
    for &v in &[0.06f32, 0.6, 6.0, 60.0, 600.0, 1000.0, 6e-4, 6e-3, 0.06, 0.6, 6.0, 10.0, 60.0, 600.0, 6e3, 6e4, 6e5, 6e6, 6e7] {
        for fine in 0..2 {
            let e = scpi::step_exp(v, false) - fine;
            let r = nr3(scpi::pow10f(e));
            check(r.starts_with("+1.000000E"), &format!("RES of range {v:e} fine {fine}: {r}"));
        }
    }
    for &v in &[6e-9f32, 6e-8, 6e-7, 6e-6, 6e-5, 6e-4, 6e-3] {
        let r = nr3(scpi::pow10f(scpi::step_exp(v, true)));
        check(r.starts_with("+1.000000E"), &format!("RES of cap range {v:e}: {r}"));
    }
}

/// MMEM:DATA? / MMEM:DELete with a prefix of another directory (the stub
/// parse_path: "WRONG/..."): -256, nothing for the main loop to run (device
/// 2f56ff9: "SHOTS/LOG005.DLG" read LOGS/LOG005.DLG; a delete would have
/// removed it).
fn mmem_dirs() {
    let mut s = Scpi::new();
    check(matches!(s.handle(b"MMEM:DATA? \"LOGS/LOG005.DLG\",0", 1), Out::Mmem(Mmem::Data(_, 0, _))), "MMEM:DATA? own prefix runs");
    check(matches!(s.handle(b"MMEM:DEL \"LOGS/LOG005.DLG\"", 1), Out::Mmem(Mmem::Del(_))), "MMEM:DEL own prefix runs");
    check(err(&mut s) == "0,\"No error\"", "no error");
    check(!matches!(s.handle(b"MMEM:DATA? \"WRONG/LOG005.DLG\",0", 1), Out::Mmem(_)), "MMEM:DATA? other prefix: not run");
    let e = err(&mut s);
    check(e.starts_with("-256,"), &format!("MMEM:DATA? other prefix: {e}"));
    check(!matches!(s.handle(b"MMEM:DEL \"WRONG/LOG005.DLG\"", 1), Out::Mmem(_)), "MMEM:DEL other prefix: not run");
    let e = err(&mut s);
    check(e.starts_with("-256,"), &format!("MMEM:DEL other prefix: {e}"));
}

fn idn() {
    let mut s = Scpi::new();
    let r = reply(&mut s, "*IDN?");
    check(r == format!("Alientek,DM40A,000102030405060708090A0B,opendm40 {}", ident::VERSION), &format!("*IDN? {r}"));
    let _ = s.handle(b"SYST:IDN:EMUL 34401A", 1);
    let r = reply(&mut s, "*IDN?");
    check(r == format!("HEWLETT-PACKARD,34401A,0,opendm40 {}", ident::VERSION), &format!("*IDN? emulated {r}"));
    let _ = s.handle(b"SYST:IDN:EMUL OFF", 1);
    let want = std::env::var("FW_VERSION").unwrap_or_default();
    check(!want.is_empty() && ident::VERSION == want, &format!("ident::VERSION {:?} = tools/fw-version.sh {want:?}", ident::VERSION));
}

/// DBG:TOUCH:CAL? / RESET / a,b,c,d,e,f (the values via touchcal.rs).
fn touch_cal() {
    let mut s = Scpi::new();
    check(matches!(s.handle(b"DBG:TOUCH:CAL?", 1), Out::Instr(Cmd::DbgTouchCal(0))), "DBG:TOUCH:CAL?");
    check(matches!(s.handle(b"DBG:TOUCH:CAL RESET", 1), Out::Instr(Cmd::DbgTouchCal(1))), "DBG:TOUCH:CAL RESET");
    check(matches!(s.handle(b"dbg:touch:cal res", 1), Out::Instr(Cmd::DbgTouchCal(1))), "dbg:touch:cal res");
    check(matches!(s.handle(b"DBG:TOUCH:CAL 0.9961,0.0101,-5.65,0.0061,0.9523,0.7", 1), Out::Instr(Cmd::DbgTouchCal(2))), "DBG:TOUCH:CAL six numbers");
    let c = touchcal::scpi_take();
    check(c.is_some_and(|c| (c[2] - (-5.65f32 * 65536.0) as i32).abs() <= 1 && c[0] == ((0.9961f32 - 1.0) * 65536.0 - 0.5) as i32), "DBG:TOUCH:CAL values kept");
    check(touchcal::scpi_take().is_none(), "DBG:TOUCH:CAL values taken once");
    check(err(&mut s) == "0,\"No error\"", "no error after the good DBG:TOUCH:CAL");
    for bad in ["DBG:TOUCH:CAL 1,0,0,0,1", "DBG:TOUCH:CAL 1,0,0,0,1,0,0", "DBG:TOUCH:CAL 1,0,x,0,1,0", "DBG:TOUCH:CAL"] {
        check(!matches!(s.handle(bad.as_bytes(), 1), Out::Instr(_)), &format!("{bad}: refused"));
        check(err(&mut s).starts_with('-'), &format!("{bad}: an error"));
    }
}

/// DBG:PING? field 3 (issue #97): the boot hold reads u32::MAX, not a
/// wrapped u64; a host keepalive reads the ms left; past the deadline 0.
fn ping_keepalive() {
    let hold = u64::MAX / 4; // main.rs INITIAL_HOLD_MS
    for now in [0u64, 7750, 1 << 40] {
        let l = scpi::keepalive_left(hold, now);
        check(l == u32::MAX, &format!("keepalive_left(hold, {now}) = {l}"));
    }
    check(scpi::keepalive_left(20_000, 5_000) == 15_000, "keepalive_left 15 s");
    check(scpi::keepalive_left(5_000, 20_000) == 0, "keepalive_left past the deadline");
}

/// DECode:... (the protocol decoder): the parse, the refused values.
fn decode_cmds() {
    use scpi::Dcd as D;
    let mut s = Scpi::new();
    let ok: &[(&str, D)] = &[
        ("DEC:PROT MODB", D::Proto(Some(2))),
        ("DECODE:PROTOCOL UART", D::Proto(Some(1))),
        ("DEC:PROT OFF", D::Proto(Some(0))),
        ("DEC:PROT?", D::Proto(None)),
        ("DEC:BAUD 9600", D::Baud(Some(9600))),
        ("DEC:BAUD AUTO", D::Baud(Some(0))),
        ("DEC:BAUD 115.2k", D::Baud(Some(115_200))),
        ("DEC:BAUD?", D::Baud(None)),
        ("DEC:RATE?", D::Rate),
        ("DEC:FORM 7e1", D::Fmt(Some(3))),
        ("DEC:FORMAT 8O1", D::Fmt(Some(2))),
        ("DEC:POL INV", D::Inv(Some(true))),
        ("DEC:POLARITY NORMAL", D::Inv(Some(false))),
        ("DEC:THR 1.5", D::Thr(Some(1.5))),
        ("DEC:THR 800MV", D::Thr(Some(0.8))),
        ("DEC:THR?", D::Thr(None)),
        ("DEC:LEV?", D::Level),
        ("DEC:COUN?", D::Count),
        ("DEC:FRAM?", D::Frames(0)),
        ("DEC:FRAM? 10", D::Frames(10)),
        ("DEC:MODE LOG", D::Mode(Some(1))),
        ("DECODE:MODE BAND", D::Mode(Some(0))),
        ("DEC:MODE?", D::Mode(None)),
        ("DEC:CLE", D::Clear),
        ("DECODE:CLEAR", D::Clear),
    ];
    for (cmd, want) in ok {
        let got = s.handle(cmd.as_bytes(), 1);
        check(matches!(got, Out::Dcd(c) if c == *want), &format!("{cmd}: parsed"));
    }
    match s.handle(b"DEC:THR AUTO", 1) {
        Out::Dcd(D::Thr(Some(v))) => check(v.is_nan(), "DEC:THR AUTO: NaN"),
        _ => check(false, "DEC:THR AUTO: parsed"),
    }
    check(err(&mut s) == "0,\"No error\"", "DECode: no error");
    for bad in ["DEC:BAUD 100", "DEC:BAUD 1E6", "DEC:FORM 8N2", "DEC:POL UP", "DEC:PROT CAN", "DEC:THR 1A", "DEC:FRAM? -1", "DEC:MODE LIST"] {
        check(!matches!(s.handle(bad.as_bytes(), 1), Out::Dcd(_)), &format!("{bad}: refused"));
        check(err(&mut s).contains("Illegal parameter value"), &format!("{bad}: Illegal parameter value"));
    }
    for undef in ["DEC:WHAT?", "DEC ON", "DEC:CLE?"] {

        check(!matches!(s.handle(undef.as_bytes(), 1), Out::Dcd(_)), &format!("{undef}: refused"));
        check(err(&mut s).contains("Undefined header"), &format!("{undef}: Undefined header"));
    }
}

fn main() {
    decode_cmds();
    suffixes();
    set_commands();
    error_queue();
    idn();
    nr3_exact();
    pow10f_exact();
    mmem_dirs();
    touch_cal();
    ping_keepalive();
    let f = unsafe { FAILS };
    if f > 0 {
        println!("{f} FAILED");
        std::process::exit(1);
    }
    println!("PASS");
}
