//! Reset path of the image (link-native.x).
//!
//! The FS-loader (0x080a0000) launches the app at 0x08004000 with
//! `msr msp, [0x08004000]; ldr r0, [0x08004004]; bx r0` (loader 0x080a18f4).
//! Word[0] is therefore only used by the loader for those two instructions;
//! it is the stock app's own value (0x2002F7F8, SRAM) so any check the loader
//! might apply to it passes exactly as for stock. `reset` immediately moves the
//! stack to the TCM top.
//!
//! `reset` (asm, region A at the start of the image, no RAM used):
//!   cpsid i; CONTROL = 0; TCM clock on; FPU on (CPACR CP10/CP11); MSP = TCM top;
//!   VTOR = 0x08004000; then the Rust shim (src/boot.rs `shim`), which uses
//!   stack only, then native::start (.data/.bss init, recovery or main).
//! Exceptions all go to `default_handler` (`b .`): a fault hangs, the FWDGT
//! (started by the shim) resets, and the shim boots opendm40 again. The table runs
//! to IRQ 55 (TIMER6 = STM32 TIM7), the AD2 stream ISR of the `ad2-isr`
//! feature (../docs/history/decisions-2026-10-22.md #3); PRIMASK stays set (`cpsid i`)
//! except while that stream runs (hy3131.rs `stream`).

use core::arch::global_asm;

#[cfg(feature = "ad2-isr")]
macro_rules! irq55 {
    () => {
        "ad2_irq_entry"
    };
}
#[cfg(not(feature = "ad2-isr"))]
macro_rules! irq55 {
    () => {
        "default_handler"
    };
}

global_asm!(concat!(
    r#"
    .section .vectors, "a", %progbits
    .global __VECTORS
    .balign 512
__VECTORS:
    .word _loader_sp
    .word reset
    .word default_handler      /* NMI */
    .word default_handler      /* HardFault */
    .word default_handler      /* MemManage */
    .word default_handler      /* BusFault */
    .word default_handler      /* UsageFault */
    .word 0
    .word 0
    .word 0
    .word 0
    .word default_handler      /* SVCall */
    .word default_handler      /* DebugMon */
    .word 0
    .word default_handler      /* PendSV */
    .word default_handler      /* SysTick */
    /* External interrupts 0..54: never enabled. */
    .rept 55
    .word default_handler
    .endr
    /* IRQ 55, TIMER6 (STM32 TIM7): the AD2 stream ISR (logger.rs `isr`,
       feature ad2-isr, the only interrupt opendm40 enables); without the
       feature default_handler, never enabled either. */
    .word "#,
    irq55!(),
    r#"

    .section .shim, "ax", %progbits
    .thumb_func
    .global reset
reset:
    cpsid i
    /* CONTROL = 0: MSP, privileged, no FP context (a launcher that left
       SPSEL = 1 would otherwise make `ldr sp` load PSP). */
    movs  r0, #0
    msr   control, r0
    isb
    /* TCM SRAM (stack, .bss) clock: RCU_AHB1EN.TCMSRAMEN (bit 20). */
    ldr   r0, =0x40023830
    ldr   r1, [r0]
    orr   r1, r1, #0x100000
    str   r1, [r0]
    /* FPU: CPACR CP10/CP11 full access (Rust code may use FP registers). */
    ldr   r0, =0xE000ED88
    ldr   r1, [r0]
    orr   r1, r1, #0xF00000
    str   r1, [r0]
    dsb
    isb
    ldr   sp, =_stack_top
    /* VTOR = our table (the FS-loader leaves it at 0x080a0000). */
    ldr   r0, =0xE000ED08
    ldr   r1, =__VECTORS
    str   r1, [r0]
    dsb
    isb
    ldr   r0, =shim_main
    blx   r0
    b     .

    .section .shim, "ax", %progbits
    .thumb_func
    .global default_handler
default_handler:
    b     .
"#
));

// IRQ 55 entry (feature ad2-isr): the exception frame's address (MSP at
// entry: r0 r1 r2 r3 r12 lr pc xpsr) to the Rust handler, which records
// the interrupted PC of its worst-latency entry (DBG:ISR? lat_pc). A tail
// call: LR keeps EXC_RETURN.
#[cfg(feature = "ad2-isr")]
global_asm!(
    r#"
    .section .text.ad2_irq_entry, "ax", %progbits
    .thumb_func
    .global ad2_irq_entry
ad2_irq_entry:
    mov   r0, sp
    b.w   ad2_stream_irq
"#
);

/// Rust part of the shim: decide, then hand over (never returns).
#[no_mangle]
#[link_section = ".shim"]
pub unsafe extern "C" fn shim_main() -> ! {
    let info = crate::boot::shim();
    crate::native::start(info)
}
