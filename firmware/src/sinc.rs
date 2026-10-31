//! Band-limited (sin(x)/x) interpolation of a scope record for fast
//! timebases (UF-41, docs/user-feedback.md §6.2): when the record has fewer
//! samples than graph columns, the trace between two samples is drawn from
//! a windowed-sinc reconstruction instead of a straight line, so a sine near
//! the Nyquist rate looks like a sine and not like a zig-zag.
//!
//! Kernel: Lanczos, a = 3 (6 taps), L(d) = sinc(d) * sinc(d / 3) for
//! |d| < 3, weights normalised to sum 1 (DC passes unchanged, a constant
//! record stays flat). Samples past the record ends are clamped to the end
//! samples. A step (square edge) shows the usual sin(x)/x ringing (about
//! 9-13 % overshoot); the caller clamps rows to the graph.
//!
//! Cost: two sin / cos pairs (polynomial, no libm) and 6 multiply-adds per
//! output point; ~399 points per frame. Pure, no state: host-tested by
//! tools/sinc-test.
//!
//! Hook (scope_page.rs, the "fewer samples than columns" branch): replace
//! the linear `v0 + (v1 - v0) * fr` at fractional sample `p` with
//! `crate::sinc::at(&f.data, p)` (any `pack10::Src`).

use core::f32::consts::PI;

/// Lanczos order (taps = 2 A).
pub const A: i32 = 3;

/// sin(x) for |x| <= pi / 2 (Taylor to x^11, < 1e-7 error there).
fn sin_q(x: f32) -> f32 {
    let x2 = x * x;
    x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0 * (1.0 - x2 / 110.0)))))
}

/// sin(x), any x (reduced to [-pi, pi], then folded to [-pi/2, pi/2]).
pub fn sin(x: f32) -> f32 {
    // (No `%`: f32 rem pulls in fmodf; the callers pass |x| < 2 pi.)
    let mut x = x;
    while x > PI {
        x -= 2.0 * PI;
    }
    while x < -PI {
        x += 2.0 * PI;
    }
    let x = if x > PI / 2.0 { PI - x } else if x < -PI / 2.0 { -PI - x } else { x };
    sin_q(x)
}

pub fn cos(x: f32) -> f32 {
    sin(x + PI / 2.0)
}

/// The 6 normalised Lanczos-3 weights for fractional offset `fr` (0 <= fr
/// < 1) of samples k - 2 .. k + 3, where k = floor(p), fr = p - k.
pub fn weights(fr: f32) -> [f32; 6] {
    let mut w = [0.0f32; 6];
    if fr <= 1e-6 {
        w[2] = 1.0;
        return w;
    }
    // d_j = fr - j, j = -2..=3. sin(pi d_j) = (-1)^j sin(pi fr);
    // sin(pi d_j / 3) by rotating sin / cos(pi fr / 3) by -j pi / 3.
    let s1 = sin(PI * fr);
    let (s3, c3) = (sin(PI * fr / 3.0), cos(PI * fr / 3.0));
    const C: f32 = 0.5; // cos(pi / 3)
    const S: f32 = 0.866_025_4; // sin(pi / 3)
    let mut sum = 0.0;
    for (i, wi) in w.iter_mut().enumerate() {
        let j = i as i32 - 2;
        let d = fr - j as f32;
        // sin(t - j pi/3) for t = pi fr / 3, by repeated rotation (|j| <= 3).
        let (mut s, mut c) = (s3, c3);
        for _ in 0..j.unsigned_abs() {
            (s, c) = if j > 0 { (s * C - c * S, c * C + s * S) } else { (s * C + c * S, c * C - s * S) };
        }
        let sp = if j & 1 == 0 { s1 } else { -s1 };
        let v = A as f32 * sp * s / (PI * PI * d * d);
        *wi = v;
        sum += v;
    }
    for wi in w.iter_mut() {
        *wi /= sum;
    }
    w
}

/// The record `src` reconstructed at fractional sample position `p`
/// (0 ..= len - 1; outside it is clamped). Empty `src`: 0.
pub fn at<S: crate::pack10::Src + ?Sized>(src: &S, p: f32) -> f32 {
    let n = src.len();
    if n == 0 {
        return 0.0;
    }
    let p = p.clamp(0.0, (n - 1) as f32);
    let k = p as i32;
    let w = weights(p - k as f32);
    let mut acc = 0.0;
    for (i, wi) in w.iter().enumerate() {
        let m = (k + i as i32 - 2).clamp(0, n as i32 - 1) as usize;
        acc += wi * src.at(m) as f32;
    }
    acc
}

/// `out[i]` = `at(src, x0 + i * dx)`.
#[allow(dead_code)]
pub fn resample<S: crate::pack10::Src + ?Sized>(src: &S, x0: f32, dx: f32, out: &mut [f32]) {
    for (i, o) in out.iter_mut().enumerate() {
        *o = at(src, x0 + i as f32 * dx);
    }
}
