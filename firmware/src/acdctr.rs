//! AC+DC event trace (DBG:ACDC:TRACE, debug only, RAM only, off at boot).
//!
//! A ring of the last TR_N AC+DC events (`Dmm::tr`): every DC conversion
//! of a DC phase, every AC result, a range / function set, a phase switch,
//! a row write and the R32 write of the AC -> DC switch. With readback on, a DC
//! conversion also carries the chip's R22 / R24 / R32 and a hash of the
//! whole row R20..R35 as the chip holds it. An accepted DC part that is
//! more than TR_JUMP off the one before freezes the ring TR_POST events
//! later, so the events around it stay for the host to read
//! (`DBG:ACDC:TRACE? <i>`, entries from the oldest; `DBG:ACDC:TRACE <0|1|2>`
//! = off / on / on with the readback, and re-arms).
//! Debug builds only: cargo feature `acdc-trace` (Cargo.toml).
#![cfg_attr(not(feature = "acdc-trace"), allow(dead_code))]

/// Ring length (28 bytes each). Without the feature `acdc-trace` (Cargo.toml)
/// the trace stays off: one entry, nothing recorded, and DBG:ACDC:TRACE is
/// an undefined header (scpi.rs).
pub const TR_N: usize = if cfg!(feature = "acdc-trace") { 64 } else { 1 };
/// The trace is built (feature `acdc-trace`); the Dmm hooks test this first.
pub const ON: bool = cfg!(feature = "acdc-trace");
/// Events recorded after the trigger.
const TR_POST: u32 = 32;
/// Trigger: an accepted DC part off by more than this fraction of the one before.
pub const TR_JUMP: f32 = 0.2;

/// Event kinds.
pub const K_DC: u8 = b'D';
pub const K_AC: u8 = b'A';
pub const K_SET: u8 = b'S';
pub const K_PHASE: u8 = b'P';
pub const K_ROW: u8 = b'C';
/// R32 = 0x38 after the AC -> DC switch (V).
pub const K_R32: u8 = b'X';

/// `fl` bits of a DC entry: the conversion was used (vet_ad1), the trigger,
/// an input-step hold, the conversion ran at the steady OSR (`slow`).
pub const F_VET: u8 = 1;
pub const F_TRIG: u8 = 2;
pub const F_HOLD: u8 = 4;
/// AC entry: the result was discarded by the settle.
pub const F_SETTLING: u8 = 8;

#[derive(Clone, Copy)]
pub struct TrEnt {
    pub t: u32,
    pub k: u8,
    pub mode: u8,
    pub range: i8,
    /// R22 as opendm40 believes it (`Dmm::r22`).
    pub r22: u8,
    /// Readback (0 without it): R22, R24, R32.
    pub r22c: u8,
    pub r24c: u8,
    pub r32c: u8,
    pub vl: u8,
    pub vh: u8,
    pub pn: u8,
    pub seen: u8,
    pub fl: u8,
    /// FNV-1a 16 of R20..R35 read back (DC) or written (row).
    pub hash: u16,
    /// DC: raw AD1; set / phase: old mode << 8 | old range; R22 write: the
    /// previous AD1 code compared.
    pub ad1: i32,
    /// DC: the value; AC: the AC result; set / phase: settle ms; DC part
    /// (`dc_val`) for a row write.
    pub val: f32,
}

impl TrEnt {
    const EMPTY: TrEnt =
        TrEnt { t: 0, k: 0, mode: 0, range: 0, r22: 0, r22c: 0, r24c: 0, r32c: 0, vl: 0, vh: 0, pn: 0, seen: 0, fl: 0, hash: 0, ad1: 0, val: 0.0 };

    fn bytes(&self) -> [u8; 28] {
        let mut b = [0u8; 28];
        b[0..4].copy_from_slice(&self.t.to_le_bytes());
        b[4..16].copy_from_slice(&[
            self.k,
            self.mode,
            self.range as u8,
            self.r22,
            self.r22c,
            self.r24c,
            self.r32c,
            self.vl,
            self.vh,
            self.pn,
            self.seen,
            self.fl,
        ]);
        b[16..18].copy_from_slice(&self.hash.to_le_bytes());
        // 18..20 spare
        b[20..24].copy_from_slice(&self.ad1.to_le_bytes());
        b[24..28].copy_from_slice(&self.val.to_bits().to_le_bytes());
        b
    }
}

pub struct Trace {
    /// 0 off, 1 on, 2 on with the readback.
    pub on: u8,
    /// Events written so far.
    n: u32,
    /// Event number of the trigger (u32::MAX: none yet).
    trig: u32,
    ents: [TrEnt; TR_N],
}

#[cfg_attr(target_os = "none", link_section = ".sram_bss")]
static mut TR: core::mem::MaybeUninit<Trace> = core::mem::MaybeUninit::uninit();

/// Initialise and hand out the one `Trace` (call once: `Dmm::new`).
pub fn tr_static() -> &'static mut Trace {
    unsafe {
        (*core::ptr::addr_of_mut!(TR)).write(Trace { on: 0, n: 0, trig: u32::MAX, ents: [TrEnt::EMPTY; TR_N] });
        (*core::ptr::addr_of_mut!(TR)).assume_init_mut()
    }
}

/// FNV-1a folded to 16 bits.
pub fn hash16(b: &[u8]) -> u16 {
    let mut h: u32 = 0x811c_9dc5;
    for &x in b {
        h = (h ^ x as u32).wrapping_mul(0x0100_0193);
    }
    (h ^ (h >> 16)) as u16
}

impl Trace {
    /// Recording (on, and not frozen after a trigger).
    pub fn live(&self) -> bool {
        ON && self.on != 0 && (self.trig == u32::MAX || self.n < self.trig.saturating_add(TR_POST))
    }

    pub fn push(&mut self, e: TrEnt) {
        if !self.live() {
            return;
        }
        if e.fl & F_TRIG != 0 && self.trig == u32::MAX {
            self.trig = self.n;
        }
        self.ents[self.n as usize % TR_N] = e;
        self.n = self.n.wrapping_add(1);
    }

    /// DBG:ACDC:TRACE <on>: set and re-arm (clears the ring).
    pub fn arm(&mut self, on: u8) {
        self.on = if ON { on.min(2) } else { 0 };
        self.n = 0;
        self.trig = u32::MAX;
    }

    /// DBG:ACDC:TRACE? <from>: "n,held,trig,on" (held = entries in the ring,
    /// trig = index of the trigger among them or -1), then ";" + 56 hex digits
    /// per entry from index `from` (0 = oldest), as many as fit in `max` bytes.
    #[inline(never)]
    pub fn text(&self, from: usize, out: &mut [u8], max: usize) -> usize {
        fn put(out: &mut [u8], at: &mut usize, b: &[u8]) {
            for &c in b {
                if *at < out.len() {
                    out[*at] = c;
                    *at += 1;
                }
            }
        }
        fn dec(out: &mut [u8], at: &mut usize, v: i64) {
            let mut t = [0u8; 20];
            let mut n = t.len();
            let neg = v < 0;
            let mut u = v.unsigned_abs();
            loop {
                n -= 1;
                t[n] = b'0' + (u % 10) as u8;
                u /= 10;
                if u == 0 {
                    break;
                }
            }
            if neg {
                n -= 1;
                t[n] = b'-';
            }
            put(out, at, &t[n..]);
        }
        let max = max.min(out.len());
        let held = (self.n as usize).min(TR_N);
        let first = self.n as usize - held;
        let trig = if self.trig != u32::MAX && (self.trig as usize) >= first { self.trig as i64 - first as i64 } else { -1 };
        let mut at = 0;
        dec(out, &mut at, self.n as i64);
        put(out, &mut at, b",");
        dec(out, &mut at, held as i64);
        put(out, &mut at, b",");
        dec(out, &mut at, trig);
        put(out, &mut at, b",");
        dec(out, &mut at, self.on as i64);
        const H: &[u8; 16] = b"0123456789abcdef";
        for i in from..held {
            if at + 57 > max {
                break;
            }
            put(out, &mut at, b";");
            for c in self.ents[(first + i) % TR_N].bytes() {
                put(out, &mut at, &[H[(c >> 4) as usize], H[(c & 15) as usize]]);
            }
        }
        at
    }
}
