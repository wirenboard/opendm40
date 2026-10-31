//! Native board bring-up: everything the old RAM-loaded image used to inherit
//! from the stock firmware (audit in docs/native-boot.md "Implementation").
//! Runs after clock::init_core and before `main` (normal boot).
//!
//! Values are the stock ones (stock/re/analysis/lcd_buzzer_v2_6.h,
//! scope_gen_ui_v2_6.h, and the stock boot FUN_0800af34 -> FUN_0800b410 rails,
//! FUN_0800aac4 GPIO/FSMC/backlight, FUN_0800a0e4 panel init).

use crate::clock::Clock;
use crate::lcd;
use crate::mmio::{modify32, r32, w32};

const RCU_AHB1EN: u32 = 0x4002_3830;
const RCU_AHB3EN: u32 = 0x4002_3838;
const RCU_APB1EN: u32 = 0x4002_3840;

const GPIOA: u32 = 0x4002_0000;
const GPIOC: u32 = 0x4002_0800;
const GPIOD: u32 = 0x4002_0C00;
const GPIOE: u32 = 0x4002_1000;
const MODER: u32 = 0x00;
const OTYPER: u32 = 0x04;
const OSPEED: u32 = 0x08;
const PUPD: u32 = 0x0C;
const BOP: u32 = 0x18;
const AFRL: u32 = 0x20;

/// FSMC bank 1 NE1 (SRAM/NOR controller): BCR1, BTR1, BWTR1.
const FSMC_BCR1: u32 = 0xA000_0000;
const FSMC_BTR1: u32 = 0xA000_0004;
const FSMC_BWTR1: u32 = 0xA000_0104;
/// Stock FUN_0800506c: keep `old & 0xFFF70081`, set MWID=16 bit (0x10), WREN
/// (bit 12), EXTMOD (bit 14); MBKEN (bit 0) by FUN_080051f4.
const BCR1_KEEP: u32 = 0xFFF7_0081;
const BCR1_SET: u32 = 0x5011;
/// Read: ADDSET 4, DATAST 15, mode A. Write: ADDSET 1, DATAST 2, mode A.
/// BWTR1 = the live stock value measured on the device (BCR1 0x00005091,
/// BTR1 0x00000F04, BWTR1 0x0FF00201): bits 31..12 are the register's reset
/// value, which the stock leaves in place; only the low timing fields are set.
const BTR1_VAL: u32 = 0x0000_0F04;
const BWTR1_VAL: u32 = 0x0FF0_0201;

const TIM13: u32 = 0x4000_1C00;

/// What the registers held before `init` overwrote them (as the FS-loader left
/// them): BCR1, BTR1, BWTR1, panel ID.
#[derive(Clone, Copy, Default)]
pub struct Before {
    pub bcr1: u32,
    pub btr1: u32,
    pub bwtr1: u32,
    pub lcd_id: u32,
}

/// Configure `pins` (mask) of `port`: mode (0 in, 1 out, 2 AF), pull (0 none,
/// 1 up, 2 down), push-pull, very high speed, AF number `af` (AF mode only).
fn pins(port: u32, mask: u32, mode: u32, pull: u32, af: u32) {
    for p in 0..16 {
        if mask & (1 << p) == 0 {
            continue;
        }
        unsafe {
            if mode == 2 {
                let (reg, sh) = if p < 8 { (AFRL, 4 * p) } else { (AFRL + 4, 4 * (p - 8)) };
                modify32(port + reg, 0xF << sh, af << sh);
            }
            modify32(port + OTYPER, 1 << p, 0);
            modify32(port + OSPEED, 3 << (2 * p), 3 << (2 * p));
            modify32(port + PUPD, 3 << (2 * p), pull << (2 * p));
            modify32(port + MODER, 3 << (2 * p), mode << (2 * p));
        }
    }
}

fn set(port: u32, pin: u32, high: bool) {
    unsafe { w32(port + BOP, if high { 1 << pin } else { 1 << (pin + 16) }) };
}

/// Clocks, rails, FSMC + ILI9488, backlight PWM (CCR1 = 0, app sets the level).
pub fn init() -> Before {
    let mut clk = Clock::new();
    unsafe {
        // GPIOA..E (bits 0..4), DMA0 (bit 21, BT UART RX + generator DAC; was
        // never enabled by opendm40), DMA1 (bit 22, scope; scope enables it too),
        // USBHS (bit 29: its FIFO RAM holds `.usbhs_bss`; native.rs
        // extra_ram_init sets it first, before the area is zeroed).
        modify32(RCU_AHB1EN, 0, 0x1F | (1 << 21) | (1 << 22) | (1 << 29));
    }
    crate::wdt::kick();

    // --- Rails as the stock boot FUN_0800b410: PC0 = 1 (BT module), PE2 = 1
    // (DMM analog supply / power hold; the shim already raised it), 10 ms,
    // PC4 = 0 (scope supply; the app raises it per its analog-power setting).
    set(GPIOC, 0, true);
    set(GPIOE, 2, true);
    pins(GPIOC, 1 << 0, 1, 0, 0);
    pins(GPIOE, 1 << 2, 1, 0, 0);
    clk.delay_ms(10);
    set(GPIOC, 4, false);
    pins(GPIOC, 1 << 4, 1, 0, 0);
    // PD6 power key: input with pull-down, pressed = 1, as the stock TIM7 key
    // init FUN_0800b7e0 (gpio_mode_set(GPIOD, INPUT, PUPD 2 = pull-down, 0x40)).
    unsafe {
        modify32(GPIOD + MODER, 3 << 12, 0);
        modify32(GPIOD + PUPD, 3 << 12, 2 << 12);
    }

    // --- FSMC (FUN_0800aac4): AF12, pull-up, very high speed ---
    // PD0 PD1 PD4(NOE) PD5(NWE) PD7(NE1) PD8 PD9 PD10 PD11(A16) PD14 PD15,
    // PE7..PE15. PD2/3/6/12/13 and PE0..6 are GPIO / other functions: untouched.
    pins(GPIOD, 0xC703 | 0x0800 | 0x0030 | 0x0080, 2, 1, 12);
    pins(GPIOE, 0xFF80, 2, 1, 12);
    let before = unsafe {
        // FSMC clock first: reading a clock-gated block may fault on a cold boot.
        modify32(RCU_AHB3EN, 0, 1);
        let b = Before { bcr1: r32(FSMC_BCR1), btr1: r32(FSMC_BTR1), bwtr1: r32(FSMC_BWTR1), lcd_id: 0 };
        w32(FSMC_BCR1, (r32(FSMC_BCR1) & BCR1_KEEP & !1) | BCR1_SET);
        w32(FSMC_BTR1, BTR1_VAL);
        w32(FSMC_BWTR1, BWTR1_VAL);
        modify32(FSMC_BCR1, 0, 1); // MBKEN
        b
    };
    clk.delay_ms(20);

    // --- ILI9488 (FUN_0800a0e4): ID read, init sequence, sleep out, on ---
    lcd::cmd(0xD3);
    let _ = lcd::read_data(); // dummy
    let mut id = 0u32;
    for _ in 0..3 {
        id = id << 8 | (lcd::read_data() as u32 & 0xFF);
    }
    const SEQ: &[u8] = &[
        0xC0, 2, 0x0F, 0x0F, //
        0xC1, 1, 0x41, //
        0xC5, 3, 0x00, 0x57, 0x80, //
        0xB1, 1, 0xA0, //
        0xB4, 1, 0x02, //
        0x36, 1, 0x48, //
        0x3A, 1, 0x55, //
        0x21, 0, //
        0xE9, 1, 0x00, //
        0xF7, 4, 0xA9, 0x51, 0x2C, 0x82, //
        0xE0, 15, 0x00, 0x09, 0x0C, 0x02, 0x0E, 0x05, 0x2F, 0x46, 0x47, 0x04, 0x0E, 0x0B, 0x2D, 0x36, 0x0F,
        0xE1, 15, 0x00, 0x10, 0x13, 0x03, 0x10, 0x06, 0x3A, 0x46, 0x51, 0x06, 0x10, 0x0D, 0x33, 0x37, 0x0F,
    ];
    let mut i = 0;
    while i + 1 < SEQ.len() {
        lcd::cmd(SEQ[i] as u16);
        let n = SEQ[i + 1] as usize;
        for k in 0..n {
            lcd::data(SEQ[i + 2 + k] as u16);
        }
        i += 2 + n;
    }
    lcd::cmd(0x11); // sleep out
    clk.delay_ms(120);
    crate::wdt::kick();
    lcd::cmd(0x29); // display on
    lcd::set_madctl(lcd::MADCTL_LANDSCAPE);
    lcd::fill_screen(lcd::BLACK);

    // --- Backlight: PA6 AF9 = TIM13_CH1, 200 MHz / 100 / 100 = 20 kHz PWM ---
    unsafe {
        modify32(RCU_APB1EN, 0, 1 << 7);
        w32(TIM13 + 0x00, 0); // CR1 off while configuring
        w32(TIM13 + 0x28, 99); // PSC
        w32(TIM13 + 0x2C, 99); // ARR
        w32(TIM13 + 0x18, 0x68); // CCMR1: OC1 PWM1 + preload
        w32(TIM13 + 0x34, 0); // CCR1: dark until App::new sets the level
        w32(TIM13 + 0x20, 1); // CCER: CC1E
        w32(TIM13 + 0x14, 1); // EGR: UG
        w32(TIM13 + 0x00, 0x81); // CR1: ARPE | CEN
    }
    pins(GPIOA, 1 << 6, 2, 0, 9);

    Before { lcd_id: id, ..before }
}
