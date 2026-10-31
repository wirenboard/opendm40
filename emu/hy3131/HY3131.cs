//
// HY3131 DMM analog front end (Hycon, DS-HY3131-V10_EN) on the DM40's bit-banged
// SPI (SCK PC10, MOSI PC12, MISO PC11, CS PD2; stock/re/analysis/hy3131_v2_6.h,
// firmware/src/hy3131.rs). Research, open points and the per-row analog model:
// emu/hy3131/RESEARCH.md and emu/hy3131/README.md.
//
// Two classes:
//   Hy3131Core  the chip: register file R00..R3F with the datasheet semantics
//               (clear-on-read INTF, enable bits clearing data, counter preset,
//               reset command, reserved bits), the conversion timing (AD1 =
//               OSR1 * 30 / SYSCLK, x2 with the chopper; AD2 = OSR2 * 2 / SYSCLK;
//               LPF / RMS at the measured period), the frequency counter (gate on
//               input edges), the comparator / CMP pin, peak hold, the
//               temperature sensor, and the input model (V / COM waveform, A
//               current, R / C / diode on the ohm path, thermocouple). It has no
//               Renode dependency and is unit-tested on the host
//               (emu/hy3131/tests). Time is passed in by the caller.
//   HY3131      the Renode peripheral: the SPI frame on the GPIO pins, virtual
//               time, the calibration record of the flash image, the monitor API
//               (SetVin / SetAd1 kept, SetInput "spec" added).
// Frame: cmd = (reg << 1) | rd, MSB first; a read has one extra SCK pulse after
// the command, then MISO is sampled with SCK low before each pulse; registers
// auto-increment. With CS high, DO carries the IRQ (INTF & INTE).
// Hardware SPI (opendm40 hy3131.rs: SPI2 is the bus): HY3131SpiPort, registered
// on the GD32 SPI2 controller (emu/peripherals/GD32_SPI.cs "spi3" @
// 0x40003C00), hands each byte to HY3131.SpiByteTimed when it has completed
// on the bus (2026-10-22: the controller times its bytes, /8 = 1.28 us): the
// same bit engine, clock edge by clock edge at their own times (the chip's
// conversions, INTF and its race at those times), with the master's sampling
// per CKPH (mode 0 at /8: the marginal DO path, MisoErrRate; CKPH 1: a full
// period late) and TornAd2 for a data frame across an AD2 edge. SpiByte (no
// time) is the untimed path. CS stays the GPIO; the read-period pulse opendm40
// makes on the PC10 GPIO between SPI bytes lands in the same engine (so would
// the extra clocks of a shifted framing); before such a pin event the
// controller delivers the bytes that completed before it (SpiCatchUp).
//
using System;
using System.Collections.Generic;
using System.Globalization;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.CPU;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public class Hy3131Core
    {
        public const double SysClk = 4915200.0;
        public const double Vref = 1.25;    // PB<6> external reference, buffered by REFO (SREFO = 1)
        public const double Vdda = 3.6;     // VDSC taps are fractions of this
        public const double AcNodeTau = 0.3;        // AC coupling time constant of the AC V node (acdc doc 6a)
        // OP1O level while OP1 is off (AC mV), relative to AGND: the coupling capacitor's start on the next
        // AC V row (INFERRED as a level). Fitted to device 185c066 (0 V, CONF:VOLT:AC 0.6 -> CONF:VOLT:AC 6:
        // first finals 0.018-0.036 V, 3 mV at 1 s; the 0 / 9.5 V square: 6 V only, no 60 V visit); -0.6 V
        // before read 2.78-3.9 V there and stepped 6 -> 60 -> 6 V.
        public const double Op1OffLevel = -0.005;
        // RMS seen while the node DC slews, per volt of remaining step: device 0.2.1 (AC V 60 -> 6 V on the
        // 0 / 9.5 V square, DBG:ACDC?) read RMS = sqrt(AC^2 + LPF^2) within ~0.1 V over the step's tail
        public const double HpfTransientGain = 1.0;
        public const double LpfRateDivisor = 0.5;   // measured LPF period = datasheet OSR4 * this (RESEARCH.md 3)

        public Hy3131Core()
        {
            Reset(0);
        }

        // ------------------------------------------------------------ registers
        public void Reset(double now)
        {
            Array.Clear(regs, 0, regs.Length);
            intf = 0; ctbov = false; acpo = false;
            ad1Start = now; ad1Done = 0; ad1Code = 0; ad1Last = now; ad1MixT = double.NegativeInfinity;
            r22PartT = double.NaN; r22PartLeft = 0; chopSwT = double.NegativeInfinity; chopSwOld = 1;
            ad2Start = now; ad2Done = 0; lpfStart = now; lpfDone = 0;
            ad2LastT = double.NegativeInfinity;
            lpfCode = 0; rmsVal = 0; ad2Code = 0;
            pkhMin = 0x3FFFF; pkhMax = -0x40000;
            ctEnabled = false; ctStart = double.NaN; ctStop = double.NaN;
            ctA = 0; ctB = 0; ctC = 0;
            rampV = 0; rampUp = true; rampLast = now;
            vc = Op1OffLevel; vcLast = now;
            role = Role.None; roleMode = -1; roleRange = 0;
            if(OhmExitRst) ohmExitT = now;
            ohmLagV = 0; ohmLagT = now; ohmLagTarget = 0;
            vjSee = false; acaT0 = double.NegativeInfinity; acvT0 = double.NegativeInfinity; acaStepT = double.NegativeInfinity; lastRole = Role.None; lastMode = -1;
            Rewrites = 0;
            rng = new Random(12345);
            faultRng = new Random(777);
            pkRng = new Random(4242);
            hpfM = 0;
            lpfNoiseRng = new Random(9191);
            creepV = double.NaN; creepT = 0;
        }

        /// Register read with the datasheet side effects (INTF cleared, CTBOV cleared).
        public byte Read(int reg, double now)
        {
            reg &= 0x3F;
            switch(reg)
            {
            case 0x00: return (byte)ad1Code;
            case 0x01: return (byte)(ad1Code >> 8);
            case 0x02: return (byte)(ad1Code >> 16);
            case 0x03: return (byte)ad2Code;
            case 0x04: return (byte)(ad2Code >> 8);
            case 0x05: return (byte)(ad2Code >> 16);
            case 0x06: return (byte)lpfCode;
            case 0x07: return (byte)(lpfCode >> 8);
            case 0x08: return (byte)(lpfCode >> 16);
            case 0x09: case 0x0A: case 0x0B: case 0x0C: case 0x0D:
                return (byte)(rmsVal >> (8 * (reg - 0x09)));
            case 0x0E: return (byte)pkhMin;
            case 0x0F: return (byte)(pkhMin >> 8);
            case 0x10: return (byte)(pkhMin >> 16);
            case 0x11: return (byte)pkhMax;
            case 0x12: return (byte)(pkhMax >> 8);
            case 0x13: return (byte)(pkhMax >> 16);
            case 0x14:
            {
                var v = (byte)((CntPin ? 0x80 : 0) | (acpo ? 0x40 : 0) | (cmpHigh ? 0x20 : 0) | (cmpLow ? 0x10 : 0) | (ctbov ? 1 : 0));
                ctbov = false;
                return v;
            }
            case 0x15: return (byte)ctC;
            case 0x16: return (byte)(ctC >> 8);
            case 0x17: return (byte)(ctC >> 16);
            case 0x18: return (byte)ctB;
            case 0x19: return (byte)(ctB >> 8);
            case 0x1A: return (byte)(ctB >> 16);
            case 0x1B: return (byte)CtaNow(now);
            case 0x1C: return (byte)(CtaNow(now) >> 8);
            case 0x1D: return (byte)(CtaNow(now) >> 16);
            case 0x1E:
            {
                // The read-clears-flag race (device c50425f, feasibility 9.5 /
                // 10): a conversion that completes while the INTF frame is under
                // way sets AD2F after the bit was sampled, and the clear of the
                // same read removes it: the flag is lost, the data (R03..R05)
                // stays readable until the next conversion. Modelled as: an AD2
                // result that completed within IntfRaceWindow before this read is
                // cleared and not reported (Swallowed counts them).
                Update(now);
                if(EnAd2)
                {
                    var ph = (now - ad2LastT) / Ad2Period;
                    var bin = (int)Math.Floor(Math.Max(0, Math.Min(0.999, ph)) * IntfPhase.Length);
                    IntfPhase[bin]++;
                }
                var v = intf;
                if(IntfRaceWindow > 0 && (v & 0x02) != 0 && now - ad2LastT < IntfRaceWindow)
                {
                    v = (byte)(v & ~0x02);
                    Swallowed++;
                }
                if(IntfRaceAd1 && IntfRaceWindow > 0 && (v & 0x04) != 0 && now - ad1Last < IntfRaceWindow)
                {
                    v = (byte)(v & ~0x04);
                    SwallowedAd1++;
                }
                intf = 0;
                return v;
            }
            default:
                return regs[reg];
            }
        }

        public byte Peek(int reg) => reg == 0x1E ? intf : regs[reg & 0x3F];

        public void Write(int reg, byte v, double now)
        {
            reg &= 0x3F;
            if(reg == 0x37)
            {
                if(v == 0x60) Reset(now);
                return;
            }
            if(reg < 0x1B)
            {
                return; // data registers
            }
            if(reg <= 0x1D)
            {
                // CTA<23:8> writable while ENCTR = 0; CTA<7:0> is held at 0.
                if(!ctEnabled && reg != 0x1B)
                {
                    regs[reg] = v;
                }
                return;
            }
            if(reg == 0x1E)
            {
                intf &= v; // 0s clear, 1s cannot be set
                return;
            }
            if(reg == 0x1F) { regs[reg] = (byte)(v & 0x1F); rowT = now; return; } // R_INTE: the end of a row write
            if(reg >= 0x38) { regs[reg] = v; return; }

            // Ad1Mix: the AD1 input as the configuration before this write burst (its first
            // byte: a row of 22 bytes plus the quiesce takes well under Ad1MixBurst; none
            // when AD1 restarted in it)
            if(Ad1Mix && EnAd1 && reg <= 0x35 && now - ad1MixT > Ad1MixBurst && now - ad1RestartT > Ad1MixBurst
               && ForcedAd1 == int.MinValue)
            {
                ad1MixOld = Clamp24(ComputeAd1(now));
                ad1MixT = now;
            }
            var old = regs[reg];
            var masked = (byte)(v & WriteMask[reg - 0x20]);
            regs[reg] = masked;
            switch(reg)
            {
            case 0x20:
                SetCounterEnable((masked & 0x02) != 0, now);
                break;
            case 0x21:
                r21T = now;
                break;
            case 0x22:
                // R22PartWinMs: only an R22 change within that long of the last row write (or in
                // the row write itself: R21 written just before) ends as a partial
                var early = R22PartWinMs <= 0 || now - rowT <= R22PartWinMs * 1e-3 || now - r21T < 1e-4;
                if(old != masked && R22PartFrac >= 0 && EnAd1 && !(r22PartT > now) && early)
                {
                    // R22PartFrac: the conversion in progress ends on the old period's grid
                    var pOld = (256 << (old & 7)) * 30.0 / SysClk * ((old & 0x10) != 0 ? 2 : 1);
                    var t0 = Math.Max(ad1Start, ad1Last);
                    r22PartT = t0 + (Math.Floor((now - t0) / pOld) + 1) * pOld;
                    r22PartSign = -r22PartSign;
                    r22PartLeft = (int)R22PartN - 1;
                }
                if(old != masked && ChopOnlySwitch(old, masked))
                {
                    // AD1CHOP 00 <-> 01 alone (same OSR, AD1OS): no restart, the conversion grid
                    // goes on and the polarity switches inside the running window (device
                    // exp/ad1-chopseq b0bbdbd7: the next result reads ~50 % of each, the one after
                    // it clean; the switch lands with a ~+-5 ms spread, Ad1ChopJitMs)
                    chopSwT = now + Ad1ChopJitMs * 1e-3 * (2 * rng.NextDouble() - 1);
                    chopSwOld = (old & 0x08) != 0 ? -1 : 1;
                    break;
                }
                if(old != masked) RestartAd1(now);
                break;
            case 0x23:
                if(((old ^ masked) & 0x80) != 0)
                {
                    if((masked & 0x80) == 0) ad1Code = 0;
                    else n7GainNow = N7Gain * (2 * rng.NextDouble() - 1);
                    RestartAd1(now);
                }
                break;
            case 0x26:
                if(((old ^ masked) & 0x80) != 0)
                {
                    ad2Start = now; ad2Done = 0;
                    if((masked & 0x80) == 0) ad2Code = 0;
                    else hpfM = 0;
                    RestartLpf(now);
                }
                else if(old != masked) { ad2Start = now; ad2Done = 0; RestartLpf(now); }
                break;
            case 0x29:
                if((masked & 0x40) == 0) lpfCode = 0;
                if((masked & 0x80) == 0) rmsVal = 0;
                else if((old & 0x80) == 0) hpfM = 0;
                if((masked & 0x04) == 0) { pkhMax = -0x40000; pkhMin = 0x3FFFF; }
                if(((old ^ masked) & 0xF8) != 0) RestartLpf(now);
                break;
            case 0x34:
                if(((old ^ masked) & 0x80) != 0) ad3Start = now;
                if(((old ^ masked) & 0x0C) != 0 && role == Role.Power && PowMix) PowerStatSwitch(old, now);
                break;
            }
            if(reg >= 0x20 && reg <= 0x35)
            {
                Rewrites++;
                DeriveRole(now);
            }
        }

        // ------------------------------------------------------------ time
        /// Advance the chip to `now` (seconds): completes conversions, counter gates, ramps.
        public void Update(double now)
        {
            if(now < lastUpdate) now = lastUpdate; // monotonic
            double vs, rref;
            OhmSource(out vs, out rref);
            Fe.Update(now, vs, rref);
            UpdateComparatorStatic(now);
            UpdateAcNode(now);
            UpdateAd1(now);
            UpdateRamp(now);
            UpdateAd2(now);
            UpdateLpf(now);
            UpdateCounter(now);
            lastUpdate = now;
        }

        // ------------------------------------------------------------ decoded fields
        public bool EnAd1 => (regs[0x23] & 0x80) != 0;
        public bool EnAd2 => (regs[0x26] & 0x80) != 0;
        public bool EnAd3 => (regs[0x34] & 0x80) != 0;
        public bool EnLpf => (regs[0x29] & 0x40) != 0;
        public bool EnRms => (regs[0x29] & 0x80) != 0;
        public bool EnPkh => (regs[0x29] & 0x04) != 0;
        public bool EnCmp => (regs[0x20] & 0x10) != 0;
        public bool EnCntI => (regs[0x20] & 0x08) != 0;
        public bool EnPCmpO => (regs[0x20] & 0x04) != 0;
        public bool EnOp1 => (regs[0x32] & 0x08) != 0;
        public int Osr1 => 256 << (regs[0x22] & 7);
        public bool Ad1Chop => (regs[0x22] & 0x10) != 0;
        public double Ad1Ig => new[] { 0.9, 1.8, 2.7, 3.6 }[(regs[0x25] >> 4) & 3];
        public double Ad1Rg => (regs[0x23] & 0x10) != 0 ? 1.0 / 3 : 1.0;
        public double Ad2Ig => new[] { 0.5, 1.0, 1.5, 2.0 }[(regs[0x25] >> 6) & 3];
        public double Ad2Rg => (regs[0x26] & 0x10) != 0 ? 1.0 / 3 : 1.0;
        public double Dcos => new[] { 0, 0.25, 0.5, 0.75, 0, -0.25, -0.5, -0.75 }[(regs[0x22] >> 5) & 7];
        public int Osr2 => new[] { 32, 64, 128, 256, 512, 1024, 1024, 1024 }[regs[0x26] & 7];
        public int Osr4 => 128 << ((regs[0x29] >> 3) & 7);
        public int Sad1I => regs[0x33] & 3;
        public int Sfuvr => regs[0x31] & 0x0F;
        public bool Svxi => (regs[0x34] & 0x08) != 0;
        public bool Sdo23 => (regs[0x34] & 0x04) != 0;

        /// AD1 conversion period in seconds (datasheet 13.1 + the chopper halving).
        public double Ad1Period => Osr1 * 30.0 / SysClk * (Ad1Chop ? 2 : 1);
        // ENCHOPAD2 does not slow AD2 down on the device (opendm40 device check of f80524f), whatever the datasheet says
        public double Ad2Period => Osr2 * ((regs[0x26] & 0x08) != 0 ? 4.0 : 2.0) / SysClk
            / ((regs[0x26] & 0x0F) == 0 ? 1.0 + Ad2Osr32Ppm * 1e-6 : 1.0);
        /// The /2, OSR 32 setting's rate offset from FAD2CLK / OSR2, ppm (the
        /// stock AC rows' 0x80 and opendm40' 76.8 kS/s rung). 0 = the datasheet,
        /// as the device converts (opendm40 device check of c830e09; the 76 574.6
        /// S/s of the 20a91d0 / 825080c streams came from opendm40' old Pll);
        /// -2935 = the rate c830e09 assumed (monitor: hy3131 Ad2Osr32Ppm).
        public double Ad2Osr32Ppm { get; set; } = 0;
        public double LpfPeriod => Ad2Period * Osr4 * LpfRateDivisor;

        /// Comparator thresholds (V relative to AGND) from R21.
        public double CmpHighThreshold => RefNode(ScmprhNames[regs[0x21] >> 4]);
        public double CmpLowThreshold => RefNode(ScmprlNames[regs[0x21] & 0x0F]);

        // ------------------------------------------------------------ observable state
        /// The DM40 front end: what is connected to the jacks, the relay / shunt state.
        public Dm40Frontend Fe = new Dm40Frontend();
        private double vNoise;                          // V-jack white noise (V rms), "noise=" of a source
        private double sigHz = 50, sigDuty = 0.5; private string sigShape = "sine"; // the waveform at the node
        public bool CalConsistent = true;      // apply the meter's calibration deviations (README "Calibration")
        public double NoiseScale = 1.0;        // 0 = noise free
        public bool BeatModel = false;         // empirical AD2/AD3 enable-order beat (RESEARCH.md 8.2)
        public bool PowMix = true;             // power role: an R34 statistic switch mid-block mixes that block
        public double PowVGain = 1.0;          // power role: extra Y gain deviation ("powvg:", for the V self-cal)
        public bool PowZeros = false;          // power role: the device's LPF zeros ("powz:1", doc 3.11 T1)
        /// OP1 input offset, V (opendm40 DC-coupled V row, feasibility E7: AD2 = OP1O in an AC V row). Device
        /// hint: the AC V sentinel row's 37 mV RMS floor at 6 V (OP1 x10, /100 tap) = a 2 kHz square of
        /// +-0.37 mV x 10 at OP1O (sentinel.rs). "op1vos:V".
        public double Op1Vos = 0.37e-3;
        /// AD2 gain deviation in that row ("ad2dcg:", 1 = nominal): the one-shot self-cal fixes the offset only.
        public double Ad2DcGain = 1.0;
        /// Per-range part of that gain, as the per_code factor (the node reads 1 / this; device test 6.5
        /// and acdcmeth e5 of 20a91d0: the self-cal's per_code against the nominal K x 1.25 / 2^18: 6 V
        /// +0.96 %, 60 V +1.88 %, 600 V +0.53 %, 1000 V +0.27 %). The RMS (the AC part) of this node does not follow it: it reads like the stock
        /// AC V row's PB4 node, through the factory AC V calibration (device e2 of 20a91d0: method C's
        /// sqrt(RMS) x the factory AC V gain - offset = A's AC part within +0.014 / -0.003 % on 6 / 60 V,
        /// where the DC self-cal's gain read it 0.13 / 0.23 % high).
        public double[] Ad2DcGainR = { 1.0096, 1.0188, 1.0053, 1.0027 };
        /// Method C's 60 V DC error (device acdcmeth e2 / e5 of 20a91d0: the E7 self-cal's DC part read
        /// +13.6 mV at 4.78 V and +22.7 mV at 24 V, constant over 20 min), split INFERRED into two parts the
        /// data cannot separate further: AD1 on SENSE - PB5 in the AC V row's switch configuration (R2A FS1,
        /// R2E / R2F without DS9 / SMODE2; DC V has DS0 FS0 / DS9 / SMODE2) reads +0.047 % against the DC V
        /// row ("ad1acg:" scales), and AD2 / the LPF read +23 codes while AD3 converts on the same OP1O node
        /// ("ad3sh:" scales). 6 V: inside the device's scatter (+0.45 +- 0.5 mV at 3 V), 0; 600 / 1000 V not
        /// measured, 0.
        public double[] Ad1AcRowGain = { 0, 4.7e-4, 0, 0 };
        public double[] Ad3ShiftCodes = { 0, 23, 0, 0 };
        public double Ad1AcRowScale = 1.0, Ad3ShiftScale = 1.0;
        /// Method C's internal zero (the DC-coupled V row with OP1+ on AGND, SOP1P 011) against a real 0 V
        /// input on SENSE, in AD2 / LPF codes (the AGND zero reads this many codes above). INFERRED from the
        /// device (ac1e852, acdcmeth e8 / z60): C's 60 V gain from that zero was 0.030 % high, its errors
        /// -7.77 mV + 0.302 mV/V x level with the anchor at 24 V = a zero ~14-16 codes off; 6 V showed
        /// nothing similar (slope -0.007 %), 600 / 1000 V not measured. "agndz:" scales (0 = off).
        public double[] AgndZeroCodes = { 0, 15, 0, 0 };
        public double AgndZeroScale = 1.0;
        /// The RMS block's HPF in the DC-coupled V row (method C): first order, time constant HpfTau (s); its
        /// state starts at 0 when ENRMS or ENAD2 is switched on, so a row write puts the node's whole DC
        /// through it as a step (an input step: the change). INFERRED from the device (20a91d0, by hand): the
        /// first method C reading after a 6 V row write with 5 V DC showed 2.11 V of AC = 0.42 x the DC, the
        /// RMS of the 320..427 ms block for 0.43 s. "hpftau:" (0 = an ideal HPF).
        public double HpfTau = 0.43;
        /// Method C's RMS fault with a large DC on its X x Y node (device, 20a91d0 by hand: 5 / 5.5 V DC on
        /// 6 V = 105 / 116 k codes gave ~0.7 readings / s, the other RMS results dropped as garbage, and the
        /// range hunted 6 <-> 60 V; 4.0 / 4.5 V = 85 / 95 k codes and the e2 square's 100.4 k mean were
        /// fine). The mechanism is not known (the power meter's YY statistic with 104 k codes of DC was fine,
        /// power doc 3.11 T1), so this is phenomenological: above RmsBigDcCodes of |DC| at the node a block's
        /// RMS register comes back as a random 38-bit pattern with probability RmsBigDcP. "rmsbig:codes"
        /// (0 = off), "rmsbigp:".
        public double RmsBigDcCodes = 102500, RmsBigDcP = 0.85;
        /// The stock AC rows' RMS block filling after a restart (opt-in; 0 = an instant RMS): the
        /// AC amplitude it reports ramps linearly from 0 to the input's over RmsFill seconds after
        /// the LPF / RMS start (a row write). Device ad97fd7 (DBG:ACDC:TRACE, a 4.0 -> 5.8 V step):
        /// the AC results rose 0.080 -> 1.166 -> 2.384 V as the RMS window filled; the fill time per
        /// row is INFERRED. Used by couptest `iacfill` (TODO "AC+DC / AUTO down-range on one
        /// part": a settle that outlasts an AC phase). "rmsfill:s".
        public double RmsFill = 0;
        /// The AC V node carrying the jack's AC only from this long after the AC V row was entered
        /// from another function (opt-in, "acarrive:ms"; 0 = at once): an RMS block over the arrival
        /// reports the share of its window after it of the input's mean square (the rest of the
        /// window had none). Device 185c066 (DC V -> AC V on the 0 / 9.5 V square, 20 / 50 / 90 Hz):
        /// the first result read after the 300 ms settle was latched during it and read 0.707 of
        /// the input (half the mean square), the next one ~107 ms later was right. The path's delay
        /// (relays / coupling) and its timing are INFERRED: "acarrive:160" with the blocks on the
        /// row write's grid puts the last block before the deadline half over the arrival.
        public double AcArriveMs = 0;
        /// An input offset of the AC A rows' AD2 path (opt-in, "acaoff:mV" referred to the shunt's input,
        /// through the row's OP1 gain; 0 = none): the LPF and the AD2 samples read it as a DC current.
        /// Device 185c066: the AC A 10 A row's AD2 stream with the A jack open averaged +150.4 codes =
        /// +0.0717 A (rms about it 1.72 mA), i.e. "acaoff:0.717" on the 10 mohm shunt. Where it arises
        /// is INFERRED (an offset at OP1 / AD2, the same on every shunt).
        public double AcaOffMv = 0;
        /// The LPF's slow creep after an input step in the DC-coupled V row (method C; device 80d5b01 tc6,
        /// acdc-approaches.md 10.11 / 10.14): after 5 -> 3 V on 6 V (41.6 k codes) the LPF was still 5..12 codes above
        /// the value it reached ~8 s later when C's second reference ended (3.6 s), while DC V (AD1) settled
        /// within 0.1 mV in ~1 s; the trace stayed ~+5 codes over the first ~4 s and fell from there (a
        /// delayed tail, not a first-order one). INFERRED, phenomenological: the LPF reads LpfCreep x (s - x)
        /// codes extra, x the node's code of the jack's DC and s the same through LpfCreepOrder cascaded
        /// first-order lags of LpfCreepTau (s). Order 3 (10.14): a step leaves (1 + q + q^2 / 2) e^(-q) of it, q =
        /// t / tau: flat for the first seconds, so a 3 s window's slope stays below 1 code / s while ~6 codes
        /// are left at ~5.6 s, which is what the device's second references saw (a8048ba e8s: the two-point gain
        /// over 5 -> 3 V +3.9..+13 codes above the accurate internal zero's at the new point; order 2 left
        /// only ~3). Keyed on the jack's DC only (an input step), not on row writes or the internal zero's OP1+
        /// switch (the device's internal-zero gain was right). Only steps while this row runs on one range:
        /// the lag survives a gap of up to CreepGap s (C's DC V reference, ~1 s; it is advanced at this row's
        /// LPF results, so a step inside the gap counts from the last one) and starts again (no creep) after a
        /// longer one or a range change (the device's entries came seconds after the calibrator's step with an
        /// accurate internal zero; 80d5b01 saw the creep on a step inside a session). 2.4e-4 / 2.5 s / order 3
        /// over the 41.6 k step: +10 codes at the step, +9.2 at 2.5 s, +6.0 at 5.6 s, +3.8 at 8 s, +1.4 at
        /// 12 s. "lpfcreep:" scales (0 = off), "creeptau:" s, "creepord:" 1 / 2 / 3.
        public double LpfCreep = 2.4e-4, LpfCreepScale = 1.0, LpfCreepTau = 2.5;
        public int LpfCreepOrder = 3;
        private const double CreepGap = 3.0;
        /// The LPF's noise in that row, codes rms per result (device 80d5b01 tc6: 3 V on 6 V, +-2..3 codes
        /// p-p at rest, sd ~1.7); the model's LPF had none. "lpfn:" codes (0 = off).
        public double LpfDcNoise = 1.7;
        /// ... times this per range (device a8048ba e8s, 10.14: the second reference's 3 s window, residual rms
        /// after a line fit, 6 V ~1.9 codes, 60 V ~1.0; 600 mV / 600 V not measured).
        public double[] LpfDcNoiseR = { 1.0, 1.0, 0.6, 0.6 };
        public bool CntPin;                    // CNT pin level (ENCNTI source); set by the board model
        public int ForcedAd1 = int.MinValue;   // SetAd1: raw code override
        public int ForcedAd2 = int.MinValue;
        public ulong Rewrites;                 // configuration writes (R20..R35)
        public long Ad2Conversions;            // AD2 results produced so far (tests)
        public double Ad2LastEdge => ad2LastT; // chip time of the last AD2 result (tests)
        /// INTF read-clears-flag race window, s (Read 0x1E): 0 = an ideal flag. The
        /// Renode peripheral sets the device-like default (HY3131.IntfRaceUs).
        public double IntfRaceWindow;
        /// INTF reads by their phase after the last AD2 result, 1/20 periods
        /// (diagnostics: HY3131.IntfPhaseHist).
        public readonly long[] IntfPhase = new long[20];
        /// AD2 flags lost to the race (tests, HY3131.Swallowed).
        public long Swallowed;
        /// The same race for AD1F (opt-in: HY3131.IntfRaceAd1; the device's
        /// AD1F is unmeasured, the read clears the whole register alike);
        /// AD1 flags lost to it.
        public bool IntfRaceAd1;
        public long SwallowedAd1;
        /// AD2 start-up (opt-in, codes; 0 = none): the first Ad2StartResults
        /// results after an AD2 (re)start read this much off, as a decimation
        /// filter that does not start from the input (device d8e249d: the
        /// A-jack probe's first peak-hold window tripped A-PROT with the A
        /// jack open). Monitor: hy3131 Ad2StartGlitch 3000.
        public int Ad2StartGlitch;
        /// PB1 picks up the V jack in the ohm / temperature rows (opt-in, mV per V; 0 = none).
        /// Device 22c85ba / c2ddd2a: with the A jack open, 5 / 10 V DC or the 0..9.6 V square
        /// on V-COM tripped the comparator on PB1 there (A-JACK), 1 V did not; the same rows
        /// put ~3.4 mV per volt on PB2 (the probe read 0.34 A / V). The ratio is INFERRED
        /// (any k with 1 V below and 5 V above the comparator's 23.75 mV fits: 10 mV / V).
        /// Monitor: hy3131 Pb1VjackMv 10.
        public int Pb1VjackMv;
        /// mV input clamp (opt-in, "mvclamp:V"; device 2f56ff9: AC V AUTO, mV-first, the 40 Hz 0 / 9.5 V
        /// square on V-COM read 0.606 V on 600 mV for 12 s, under the 1.03 x 600 mV up-range threshold):
        /// the V jack on the mV path (path 1) clipped to +-V before the AC mV node. 0.606 V AC RMS of a
        /// 0 / 9.5 V square = a node swing of 0 .. 1.21 V: "mvclamp:1.212". Where the clamp sits and its
        /// shape are INFERRED (a symmetric hard clip). AC mV rows only. 0 = none.
        public double MvClampV;
        /// Ohm: a positive source on the jack above the ohm source's open-circuit voltage (the drop
        /// across Rref reversed) reads +full scale, OL (device 519fb8a 7b: 5 / 10 V DC on V-COM read
        /// OL on every range 600 ohm .. 60 Mohm, never the negative code of a source of the other
        /// sign). Default on; "ohmrevol:0" = the plain ratio (a negative code, LIVE?).
        public bool OhmRevOl = true;
        /// Ohm: OP1O (the row's x1 buffer of the input, PB0) does not follow a source on the jack
        /// above the open-circuit voltage: device 1a11eda (the jack check's AD2 = OP1O - PB3 peak)
        /// read 2.25 V at 5 V DC (600 ohm; 2.14 V on 60 Mohm), 2.02-2.03 V at 10 V. The model clips
        /// the node at OhmOp1V0 - OhmOp1K x the jack's volts (2.47 - 0.044 V: 2.25 V at 5 V, 2.03 V
        /// at 10 V); the line and the range independence are INFERRED from those points. Default on;
        /// "ohmop1k:0" = the old swing clip (+2.55 V), which made the jack check say "driven".
        public double OhmOp1K = 0.044, OhmOp1V0 = 2.47;
        /// Ohm: a source on the jack at or above this many volts overdrives the comparator: CMPHO
        /// and CMPLO in one read (CTSTA 0x70) on PB1. Device 1a11eda / f540f0d E4: RES at 5 / 10 V
        /// DC and the 0 / 9.5 V square read CTSTA 0x70, 1 V and 3 V no hit at all; the level between
        /// 3 and 5 V is INFERRED (4 V). Instantaneous (the square reads it in its high half).
        /// Default 4; "ohmboth:0" = off (PB1 alone, Pb1VjackMv).
        public double OhmBothV = 4.0;
        /// PB1 picking up the mV clamp's current in the AC mV rows (opt-in, "mvpb1:mV per V" of the jack
        /// beyond the clamp; device 2f56ff9: the same square asserted AJACK,AD1PK_PB1 with the A jack
        /// empty; the ratio is INFERRED: 10 mV / V puts 83 mV peaks on PB1, over the 23.75 mV threshold).
        public double MvPb1Mv;
        /// Continuity contact transient (opt-in, "contbump:mV", "contbumptau:ms"; owner 2026-10-26:
        /// the probes touched together in continuity raised A-JACK for ~1.5 s, A jack empty): every
        /// change of the V-COM resistor while the continuity / diode row runs puts contbump mV x
        /// exp(-dt / contbumptau) x cos(2 pi 25 Hz dt) on PB1 (a bouncing contact = a burst). The
        /// shape and size are INFERRED (phenomenological; the device's PB1 during a contact was not
        /// recorded). Default off, 40 ms.
        public double ContBumpMv, ContBumpTau = 40;
        private double contBumpT0 = double.NegativeInfinity;
        /// AD1 decimation memory (opt-in, "ad1mix:1"; device decaf60, DC V AUTO from 600 V: one
        /// reading at 77-79 % on 6 V after two range steps). A configuration write that does not
        /// restart AD1 (R22 unchanged, ENAD1 kept: one autorange step right after another, both
        /// rows at OSR 8192) leaves the conversions of the next 3 periods a sinc^3-weighted mix
        /// of the old and the new input. A restart (an R22 change, ENAD1 toggled) flushes it.
        /// Off: every conversion is the input at its end, as before.
        public bool Ad1Mix;
        /// Switch transient on the sentinel nodes (opt-in, "swtr:mV", "swtau:ms", "swhz:Hz";
        /// HYPOTHETICAL shape, no device numbers yet: owner 2026-10-23, false A-JACK / AC? after
        /// switches): from every row change (role / mode / range) PB1, the V tap and the A rows'
        /// SENSE tap carry swtr mV x exp(-dt / swtau) x cos(2 pi swhz dt) on top of the input,
        /// both signs (a relay / front-end ring). Default 60 ms, 25 Hz; off (0 mV) by default.
        public double SwTrMv, SwTrTau = 60, SwTrHz = 25;
        /// Leaving ohm (device 469f354, 2026-10-23: RES -> DC A / mA, nothing connected): the
        /// relay back to the V/A path (PA10) while the chip runs an ohm row; the A rows'
        /// comparator on SENSE then reads CMPHO | CMPLO together (CTSTA 0x70) for ohmexit s and
        /// CMPLO alone (0x50) for ohmexitlo s more (the device: ~3.5 s, then 0x50 until ~5 s),
        /// whatever the input ("ohmexit:3.5;ohmexitlo:1.2"; off by default).
        public double OhmExitBoth, OhmExitLow;
        private double ohmExitT = double.NegativeInfinity;
        /// The same CTSTA trace also from a chip reset (opt-in, "ohmexitrst:1"; device 6bc494a:
        /// A / mA -> Scope -> DMM with 5 V on V-COM, 0x70 to 7.75 s, 0x50 to 8.01 s after the
        /// switch: the DMM rail comes back, the chip resets, the A row follows). Off by default.
        public bool OhmExitRst;
        /// Ohm node lag (opt-in, "ohmtau:ms"; CHOSEN, not measured): in the ohm rows the node
        /// voltage across Rx (what AD1 sees against the reference drop) relaxes to its DC value
        /// with this time constant after every row change (an ohm range step changes Rref, so
        /// the target), starting from 0 V (the node outside the ohm rows with a passive Rx). It
        /// stands for the input network's RC and a calibrator's resistance simulation catching
        /// up (device 6bc494a: DC V -> RES on 390 / 100 ohm read 8.8 Mohm, then 219 / 57 ohm on
        /// the right range before the right value). Off (0) by default.
        public double OhmTauMs;
        private double ohmLagV, ohmLagT, ohmLagTarget;
        /// AC A rows, AD1 on SENSE - PB5 (the opendm40 40728b8 V-JACK? sentinel): PB5 is not
        /// defined by the A rows (only the DC V 60 V+ rows' power bus / OP3 drive it), and the
        /// device's AD1 peak there ran from ~-117 000 at 0.7 s to the negative rail by ~2 s
        /// whatever was on V-COM (device-check 40728b8 2a). Replay: PB5 rises by
        /// pb5drift V x (1 - exp(-t / pb5tau s)) from the first AC A row write (kept across AC A
        /// range changes), so SENSE - PB5 falls ("pb5drift:0.6;pb5tau:2.2"; off by default).
        /// SENSE - AGND (the fixed row) does not drift.
        public double Pb5DriftV, Pb5DriftTau = 2.2;
        /// AC A rows, SENSE itself (either AD1 reference; device d2aff22): with 24 V DC on V-COM
        /// SENSE - AGND ramped from ~-64 mV at 0.5 s to the negative rail (< -347 mV at IG 3.6) by
        /// ~3.3 s, 9 000-12 000 codes per 107 ms hold. Replay: SENSE falls by senseneg V x
        /// (1 - exp(-t / sensetau s)) from the first AC A row ("senseneg:0.75;sensetau:5"; off by
        /// default).
        public double SenseNegV, SenseNegTau = 5;
        private double acaT0 = double.NegativeInfinity;
        /// When the AC V row was entered from another role (AcArriveMs).
        private double acvT0 = double.NegativeInfinity;
        /// AC A 10 A -> mA / uA (device 40728b8: CONF:CURR:AC 10, then 0.06, the A jack empty:
        /// the first 60 mA result read OL): the AD2 node (OP1O - PB3) carries a step of acastep
        /// codes for acastepms ms from that row write, in the LPF and the RMS blocks it overlaps
        /// ("acastep:200000;acastepms:600"; off by default). The shape is not measured: only
        /// "the first result after the settle reads over range" is.
        public double AcaStepCodes, AcaStepMs = 600;
        /// ... and after it a tail that decays with this time constant, ms (opt-in, "acasteptau:ms";
        /// 0 = none, the step ends at acastepms). Device v0.1-pre1 (2026-10-24, CONF:CURR:AC 10 then
        /// 0.06, open jack): the first in-range 60 mA results read 28.7-59.5 mA up to ~0.85 s after the
        /// command, then fell ~100x per result to ~1e-6 A. The shape is INFERRED from those results.
        public double AcaStepTauMs;
        /// AD1 after an R22 (OSR / chop) change (opt-in, "r22part:frac", "r22off:codes", "r22n:n";
        /// INFERRED shape, device v0.1-pre1 2026-10-24): the conversion in progress is not restarted;
        /// it ends on its old schedule as a partial result, frac x the input plus r22off codes with a
        /// sign that alternates per event, the grid at the new period starts there, and its first
        /// r22n - 1 results read the same way. A second change before that end keeps the pending
        /// partial. Device: right after a function change, DC V at MED read 0.376-0.405 V for 1 V in
        /// the two conversions after the firmware's 0x15 -> 0x17 write (from AC V / ohm), RES the same
        /// after its in-range OSR write; the first DC mA conversion after an entry from DC V 6 V at
        /// MED / SLOW read +-44..56 mA or over range on an open jack (none at FAST, where the
        /// conversion in progress is 100 ms, not 400). The device showed none for the same R22 steps
        /// later in a function; the model does not tell the two apart. Off (frac -1) by default.
        public double R22PartFrac = -1, R22OffCodes, R22PartN = 1;
        /// ... only for an R22 change within this many ms of the last row write (opt-in,
        /// "r22win:ms"; 0 = any change, the default). INFERRED from device v0.1-pre1: the partials
        /// came after an R22 write ~0.4 s after the row write, not after the same OSR steps later
        /// in a function (input steps at MED / SLOW, osrstep.py).
        public double R22PartWinMs;
        private double rowT = double.NegativeInfinity, r21T = double.NegativeInfinity;
        private int r22PartLeft;
        /// Chopped AD1 restart (opt-in, "chophalf:1"; INFERRED from device ea73ea5, osr/ fr_40_15:
        /// the first record of an OSR 8192 chopped session after a 640/s one came 51.9 ms after the
        /// session's first (stale) flag, the later ones 100.8 ms apart): after a restart with the
        /// chopper on, the first result comes half a period later, then every period. Off by default.
        public bool ChopHalf;
        /// Chopped AD1: every other result that many ms early (opt-in, "chopskew:ms"; INFERRED from
        /// device 25b6ec3, an OSR 8192 chopped session right after a 640/s one: flags 78.4 / 123.2 ms
        /// apart in turn, 100.8 ms on average). Off (0) by default.
        public double ChopSkewMs;
        private double r22PartT = double.NaN;
        /// The last AD1CHOP 00 <-> 01 switch (UnchopTerm): when it took effect, the old sign.
        private double chopSwT = double.NegativeInfinity;
        private double chopSwOld = 1;
        public double Ad1VosCodes = 0;
        public double Ad1UnchopGainPpm = 0;
        public double Ad1ChopJitMs = 5;
        private int r22PartSign = 1;
        private double acaStepT = double.NegativeInfinity;
        private Role lastRole = Role.None;
        private int lastMode = -1;
        /// DC A rows' SENSE comparator blind to the V jack depending on the history (device
        /// 40728b8, 24 V on V-COM: CTSTA 0x70 / 0x50 in DC A only when the temperature row came
        /// before; DC V 60 V+, an ohm-network row or a chip reset left it reading 0 for 10+ s;
        /// AC V and the A rows keep either state). "vjblind:1": SENSE reads 0 in the DC A role
        /// unless a temperature row ran since the last DC V r1+ / ohm / continuity / diode /
        /// capacitance / frequency row or reset. A replay of the observation, not a mechanism.
        public bool VjBlind;
        private bool vjSee;
        private int paExitsSeen;
        private double swT0 = double.NegativeInfinity;
        private double SwTr(double t)
        {
            if(SwTrMv == 0) return 0;
            var dt = t - swT0;
            if(dt < 0 || dt > 20 * SwTrTau * 1e-3) return 0;
            return SwTrMv * 1e-3 * Math.Exp(-dt / (SwTrTau * 1e-3)) * Math.Cos(2 * Math.PI * SwTrHz * dt);
        }
        private const double Ad1MixBurst = 20e-3; // the bit-banged row (+ quiesce) takes a few ms
        private double ad1MixT = double.NegativeInfinity, ad1RestartT = double.NegativeInfinity;
        private int ad1MixOld;
        public const int Ad2StartResults = 4;
        private double ad2LastT = double.NegativeInfinity;
        public void ForceIntf(byte bits) { intf |= bits; }
        public int Intf => intf;
        public int Ad1Code => ad1Code;
        public long RmsValue => rmsVal;
        public int LpfCode => lpfCode;
        public bool Acpo => acpo;
        public bool CmpPin => EnPCmpO && acpo;  // CMP output pin (continuity beeper)
        public bool Irq => (intf & regs[0x1F]) != 0;
        public string RoleName => role == Role.Power ? string.Format("Power vr{0} iu{1} ir{2}", PowVr, PowIu, PowIr)
                                : role.ToString() + (role == Role.None ? "" : " m" + roleMode + " r" + roleRange);
        public int Mode => roleMode;
        public int Range => role == Role.Power ? PowVr * 6 + PowIu * 2 + PowIr : roleRange;
        public double RampNodeV => rampV;
        public double AcNodeVc => vc;
        public double LastAd1Time => ad1Last;

        /// One-line description of the front end and the chip configuration (key=value tokens).
        public string FrontendState(string extra = "")
        {
            var ci = CultureInfo.InvariantCulture;
            var sb = new System.Text.StringBuilder();
            sb.Append(Fe.PinState());
            sb.AppendFormat(ci, " role={0} mode={1} range={2}", role, roleMode, roleRange);
            sb.AppendFormat(ci, " ad1={0} ad2={1} ad3={2} lpf={3} rms={4} ctr={5} cmp={6} cmppin={7} op1={8} op2={9} osr1={10} chop={11}",
                EnAd1 ? 1 : 0, EnAd2 ? 1 : 0, EnAd3 ? 1 : 0, EnLpf ? 1 : 0, EnRms ? 1 : 0, (regs[0x20] & 2) != 0 ? 1 : 0,
                EnCmp ? 1 : 0, CmpPin ? 1 : 0, EnOp1 ? 1 : 0, (regs[0x32] & 0x80) != 0 ? 1 : 0, Osr1, Ad1Chop ? 1 : 0);
            sb.AppendFormat(ci, " sop1p={0} sad1fp={1} sad1fn={2} sad1rh={3} sad1rl={4} sad2ip={5} scmpi={6} sdio={7} sad1i={8} smode=0x{9:X2} sfuvr={10}",
                Sop1pNames[regs[0x32] & 7], Sad1fpNames[regs[0x24] >> 4], Sad1fnNames[regs[0x24] & 7], Sad1rhNames[(regs[0x28] >> 4) & 7],
                Sad1rlNames[regs[0x28] & 7], Sad2ipNames[regs[0x27] >> 6], ScmpiNames[regs[0x20] >> 5], (regs[0x24] & 8) != 0 ? 1 : 0,
                Sad1I, regs[0x2F] & 0x7F, Sfuvr);
            // the rest of the internal switching, for the GTK schematic (names as in ref/registers.md)
            var smode3 = (regs[0x2F] & 0x08) != 0;
            sb.AppendFormat(ci, " sop2p={0} sad2in={1} sad2rh={2} sad2rl={3} sad3ip={4} sad3in={5} ops1={6} ops2={7} sft1={8} srefo={9} sagnd={10} vrefsel={11} pasw={12}",
                Sop2pNames[(regs[0x32] >> 4) & 7], Sad2inNames[(regs[0x27] >> 4) & 3], Sad2rhNames[(regs[0x27] >> 2) & 3],
                Sad2inNames[regs[0x27] & 3], Sad2ipNames[regs[0x35] >> 6], Sad2inNames[(regs[0x35] >> 4) & 3],
                regs[0x25] & 1, (regs[0x25] >> 1) & 1, (regs[0x33] >> 2) & 3, regs[0x30] >> 7, (regs[0x31] >> 4) & 3,
                smode3 ? SfuvrSmode3[Sfuvr, acpo ? 1 : 0] : SfuvrNames[Sfuvr], PaSwitches());
            double jdc, jac, hz, duty; string shape;
            Fe.VJack(Fe.RelayPa, out jdc, out jac, out hz, out shape, out duty);
            sb.AppendFormat(ci, " vjack={0:G5} vjack_ac={1:G5} vjack_hz={2:G5}", jdc, jac, hz);
            double idc, iac;
            Fe.ACurrent(out idc, out iac, out hz, out shape, out duty);
            sb.AppendFormat(ci, " ijack={0:G5} ijack_ac={1:G5} vshunt={2:G5}", idc, iac, idc * Fe.ShuntR);
            double vs, rref;
            OhmSource(out vs, out rref);
            if(rref > 0)
            {
                double vx, vref;
                Fe.OhmNode(vs, rref, out vx, out vref);
                // the test current through Rref into the node (GTK ohm-source block)
                sb.AppendFormat(ci, " vsrc={0:G4} rref={1:G5} vx={2:G5} itest={3:G5}", vs, rref, vx, (vs - vx) / rref);
            }
            if(role == Role.Diode) sb.AppendFormat(ci, " vx={0:G5}", Fe.DiodeNode(DiodeTestCurrent, DiodeCompliance));
            // diode = OP3 as a current source (compliance), cap = the RC oscillator's R or the ramp current
            if(role == Role.Diode) sb.AppendFormat(ci, " itest={0:G5} compl={1:G3}", DiodeTestCurrent, DiodeCompliance);
            if(role == Role.CapOsc) sb.AppendFormat(ci, " rcosc={0:G5}", CapOscR());
            if(role == Role.CapRamp || role == Role.CapDischarge)
                sb.AppendFormat(ci, " itest={0:G5} vramp={1:G5}", role == Role.CapRamp ? CapK[roleRange] * 1e-6 : 0, rampV);
            // converter / statistics settings and results for the GTK signal-flow diagram
            sb.AppendFormat(ci, " svxi={0} sdo23={1} osr2={2} osr4={3} ad1ig={4:G3} ad2ig={5:G3} ad3ig={6:G3} pkh={7} pkhsel={8} sphacal={9}",
                Svxi ? 1 : 0, Sdo23 ? 1 : 0, Osr2, Osr4, Ad1Ig, Ad2Ig, Ad3Ig, EnPkh ? 1 : 0, regs[0x29] & 3, regs[0x36]);
            sb.AppendFormat(ci, " ad2rate={0:G5} lpfper={1:G4} ad2code={2} pkmax={3} pkmin={4} ctb={5} ctc={6} ctf={7:G6}",
                1.0 / Ad2Period, LpfPeriod, ad2Code, pkhMax, pkhMin, ctB, ctC, CounterHz());
            sb.Append(Readout());
            sb.Append(extra);
            sb.AppendFormat(ci, " ad1code={0} lpf={1} rms={2} period={3:G4} cj={4:G4} die={5:G4} conn=\"{6}\"",
                ad1Code, lpfCode, rmsVal, Ad1Period, Fe.CjTemp, Fe.Die, Fe.Connection());
            return sb.ToString();
        }

        /// Frequency of the last completed counter gate (CTB cycles over the gate), 0 before one.
        private double CounterHz()
        {
            if(!ctEnabled || double.IsNaN(ctStop) || ctB <= 0) return 0;
            var ticks = (0x1000000 - ctInit) + (double)ctA;
            return ticks > 0 ? ctB * SysClk / ticks : 0;
        }

        /// What the chip's results mean at the jacks, with the nominal (uncalibrated) stock / opendm40
        /// scale factors of the current row: tokens rd_<quantity>=<value> (V, A, W, ohm) for the GTK
        /// signal-flow diagram (power: the selected statistic of the RMS block, the LPF mean).
        private string Readout()
        {
            var ci = CultureInfo.InvariantCulture;
            var o = new System.Text.StringBuilder();
            var r = roleRange;
            var sqrtRms = Math.Sqrt(Math.Max(0, (double)rmsVal));
            var opg = (regs[0x25] & 1) != 0 ? 10.0 : 1.0;
            switch(role)
            {
            case Role.DcV:
                if(EnAd1) o.AppendFormat(ci, " rd_vdc={0:G6}", ad1Code * AcvK[r] * 1.38889 / 8388606.0);
                break;
            case Role.DcMv:
                if(EnAd1) o.AppendFormat(ci, " rd_vdc={0:G6}", ad1Code * 1.38889 / MvDiv[r] / 8388606.0);
                break;
            case Role.DcA:
                if(EnAd1) o.AppendFormat(ci, " rd_idc={0:G6}", ad1Code * 1.25 / 0.9 / 8388606.0 / (Fe.ShuntR * MvDiv[r]));
                break;
            case Role.AcV:
                // the row's AD2 gain cancels: the node is V / (k * IG) (NodeRatioAc), the code IG * node / VR
                if(EnRms) o.AppendFormat(ci, " rd_vrms={0:G6}", sqrtRms * 1.25 / 262144.0 * AcvK[r]);
                break;
            case Role.AcA:
                if(EnRms) o.AppendFormat(ci, " rd_irms={0:G6}", sqrtRms * 1.25 / 262144.0 / Ad2Ig / (Fe.ShuntR * opg));
                break;
            case Role.Ohm: case Role.Cont:
            {
                double vs, rref;
                OhmSource(out vs, out rref);
                if(EnAd1 && rref > 0) o.AppendFormat(ci, " rd_r={0:G6}", Math.Abs(ad1Code) * rref / 0.9 / 8388606.0);
                break;
            }
            case Role.Power:
            {
                // X = AD2 = I (codes per A: shunt x OP1 gain at AD2), Y = AD3 = V (codes per V: the tap)
                var xs = Fe.ShuntR * opg * Ad2Ig / (Ad2Rg * Vref) * 262144.0;
                var ys = ((regs[0x25] & 2) != 0 ? 10 : 1) / AcvK[PowVr] * Ad3Ig / (Ad3Rg * Vref) * 262144.0;
                o.AppendFormat(ci, " powxs={0:G6} powys={1:G6}", xs, ys);
                if(EnRms)
                {
                    var st = PowerStat(regs[0x34]);
                    if(st == 0) o.AppendFormat(ci, " rd_irms={0:G6}", sqrtRms / xs);
                    else if(st == 2) o.AppendFormat(ci, " rd_vrms={0:G6}", sqrtRms / ys);
                    else o.AppendFormat(ci, " rd_p={0:G6}", rmsVal / (xs * ys));
                }
                if(EnLpf) o.AppendFormat(ci, Sdo23 ? " rd_vdc={0:G6}" : " rd_idc={0:G6}", lpfCode / (Sdo23 ? ys : xs));
                if(EnAd1 && (regs[0x24] >> 4) == 3)
                    o.AppendFormat(ci, " rd_ad1idc={0:G6}", ad1Code * 1.25 / 0.9 / 8388606.0 / (Fe.ShuntR * MvDiv[PowIr]));
                break;
            }
            }
            return o.ToString();
        }

        /// Closed PA<n> switches (R2A..R2E: PS power bus, DS OP3 output, FS FB node, SS SENSE node),
        /// e.g. "9S,5F,4D" (pin number + letters), "-" when all are open.
        private string PaSwitches()
        {
            var parts = new List<string>();
            for(var n = 9; n >= 0; n--)
            {
                var nib = (regs[0x2A + n / 2] >> (4 * (n & 1))) & 0xF;
                if(nib == 0) continue;
                var sw = "";
                if((nib & 8) != 0) sw += "P";
                if((nib & 4) != 0) sw += "D";
                if((nib & 2) != 0) sw += "F";
                if((nib & 1) != 0) sw += "S";
                parts.Add(n + sw);
            }
            return parts.Count == 0 ? "-" : string.Join(",", parts);
        }

        private static readonly string[] Sop2pNames = { "SENSE", "FB", "RLU", "AGND", "PB0", "PB1", "PB2", "PB3" };
        private static readonly string[] Sad2inNames = { "RLU", "AGND", "PB3", "PB5" };
        private static readonly string[] Sad2rhNames = { "FB", "REFO", "VREF", "PB6" };
        private static readonly string[] Sop1pNames = { "SENSE", "FB", "RLU", "AGND", "PB0", "PB1", "PB2", "PB8" };
        private static readonly string[] Sad1fpNames = { "SENSE", "FB", "RLU", "OP1O", "OP2O", "VDDA", "REFO", "VREF", "PB0", "PB1", "PB2", "PB3", "PB4", "PB5", "PB6", "PB7" };
        private static readonly string[] Sad1fnNames = { "SENSE", "RLU", "VSSA", "AGND", "PB2", "PB3", "PB4", "PB5" };
        private static readonly string[] Sad1rhNames = { "FB", "REFO", "VREF", "PB6", "RLU", "VDDA", "AGND", "-" };
        private static readonly string[] Sad1rlNames = { "RLU", "AGND", "PB3", "PB5", "FB", "VSSA", "VREF", "-" };
        private static readonly string[] Sad2ipNames = { "OP1O", "OP2O", "PB4", "PB7" };
        private static readonly string[] ScmpiNames = { "SENSE", "FB", "OP1O", "PB0", "PB1", "RLD", "PB3", "PB4" };

        /// Calibration record (stock layout, hy3131_v2_6.h): gain / offset per path.
        public float[] Cal = DefaultCal();

        public static float[] DefaultCal()
        {
            var c = new float[0x1B8 / 4];
            // gains 1, offsets 0 at the stock offsets
            foreach(var o in new[] { 0x04, 0x24, 0x34, 0x54, 0x64, 0x84, 0x94, 0xc4, 0xf4, 0x124, 0x15c, 0x164, 0x170 })
            {
                var n = GainCount(o);
                for(var i = 0; i < n; i++) c[o / 4 + i] = 1f;
            }
            return c;
        }

        private static int GainCount(int o)
        {
            switch(o)
            {
            case 0x04: case 0x34: case 0x64: return 4;
            case 0x24: case 0x54: case 0x84: return 2;
            case 0x94: case 0xc4: case 0xf4: return 6;
            case 0x124: case 0x170: return 8;
            default: return 1;
            }
        }

        private float CalGain(int off) => CalConsistent ? Cal[off / 4] : 1f;
        private float CalOffs(int off) => CalConsistent ? Cal[off / 4] : 0f;

        // ------------------------------------------------------------ input spec
        /// Connection spec (Dm40Frontend.Apply): what is on the jacks, plus the model options
        /// noise:, cal:, beat:. The old per-function keys (dcv:, acv:, dca:, ohm:, cap:, temp: ...)
        /// are aliases for the equivalent connection.
        public void SetInput(string spec)
        {
            var r0 = Fe.R;
            var opts = Fe.Apply(spec);
            if(Fe.R != r0 && !(double.IsInfinity(Fe.R) && double.IsInfinity(r0))) contBumpT0 = lastUpdate;
            foreach(var o in opts)
            {
                switch(o.Key)
                {
                case "noise": NoiseScale = o.Value; break;
                case "cal": CalConsistent = o.Value != 0; break;
                case "beat": BeatModel = o.Value != 0; break;
                case "powmix": PowMix = o.Value != 0; break;
                case "ad1mix": Ad1Mix = o.Value != 0; break;
                case "swtr": SwTrMv = o.Value; break;
                case "mvclamp": MvClampV = o.Value; break;
                case "mvpb1": MvPb1Mv = o.Value; break;
                case "contbump": ContBumpMv = o.Value; break;
                case "contbumptau": ContBumpTau = o.Value; break;
                case "ohmrevol": OhmRevOl = o.Value != 0; break;
                case "ohmop1k": OhmOp1K = o.Value; break;
                case "ohmboth": OhmBothV = o.Value; break;
                case "ohmexit": OhmExitBoth = o.Value; break;
                case "ohmexitlo": OhmExitLow = o.Value; break;
                case "ohmexitrst": OhmExitRst = o.Value != 0; break;
                case "ohmtau": OhmTauMs = o.Value; break;
                case "capda": CapDaK = o.Value; break;
                case "capdatau": CapDaTau = o.Value; break;
                case "capasym": CapAsym = o.Value; break;
                case "capcmpw": CapCmpW = o.Value; break;
                case "capvavg": CapVAvg = o.Value; break;
                case "pb5drift": Pb5DriftV = o.Value; break;
                case "pb5tau": Pb5DriftTau = o.Value; break;
                case "senseneg": SenseNegV = o.Value; break;
                case "sensetau": SenseNegTau = o.Value; break;
                case "acastep": AcaStepCodes = o.Value; break;
                case "acastepms": AcaStepMs = o.Value; break;
                case "acasteptau": AcaStepTauMs = o.Value; break;
                case "ad1vos": Ad1VosCodes = o.Value; break;
                case "ad1ug": Ad1UnchopGainPpm = o.Value; break;
                case "ad1chopjit": Ad1ChopJitMs = o.Value; break;
                case "r22part": R22PartFrac = o.Value; break;
                case "r22off": R22OffCodes = o.Value; break;
                case "r22n": R22PartN = o.Value; break;
                case "r22win": R22PartWinMs = o.Value; break;
                case "chophalf": ChopHalf = o.Value != 0; break;
                case "chopskew": ChopSkewMs = o.Value; break;
                case "vjblind": VjBlind = o.Value != 0; break;
                case "swtau": SwTrTau = o.Value; break;
                case "swhz": SwTrHz = o.Value; break;
                case "ctsettle": CtSettleMs = o.Value; break;
                case "ctretain": CtRetain = o.Value != 0; break;
                case "ct0dead": Ct0DeadMs = o.Value; break;
                case "powvg": PowVGain = o.Value; break;
                case "powz": PowZeros = o.Value != 0; break;
                case "vnoise": vNoise = o.Value; break;
                case "op1vos": Op1Vos = o.Value; break;
                case "ad2dcg": Ad2DcGain = o.Value; break;
                case "n7": N7Scale = o.Value; break;
                case "n7g": N7Gain = o.Value; break;
                case "n7a": N7ACodes = o.Value; break;
                case "ad1acg": Ad1AcRowScale = o.Value; break;
                case "ad3sh": Ad3ShiftScale = o.Value; break;
                case "agndz": AgndZeroScale = o.Value; break;
                case "hpftau": HpfTau = o.Value; break;
                case "rmsfill": RmsFill = o.Value; break;
                case "acarrive": AcArriveMs = o.Value; break;
                case "acaoff": AcaOffMv = o.Value; break;
                case "rmsbig": RmsBigDcCodes = o.Value; break;
                case "rmsbigp": RmsBigDcP = o.Value; break;
                case "lpfcreep": LpfCreepScale = o.Value; break;
                case "creeptau": LpfCreepTau = o.Value; break;
                case "creepord": LpfCreepOrder = o.Value >= 3 ? 3 : o.Value >= 2 ? 2 : 1; break;
                case "lpfn": LpfDcNoise = o.Value; break;
                }
            }
        }


        // ============================================================ the chip
        private void RestartAd1(double now)
        {
            ad1Start = r22PartT > now ? r22PartT : now; ad1Done = 0;
            if(ChopHalf && Ad1Chop) ad1Start -= Ad1Period / 2; // the first result half a period after the start
            ad1RestartT = now; ad1MixT = double.NegativeInfinity; // a restart flushes the filter
        }

        /// Ad1Mix: the share of a sinc^3 kernel (3 conversion periods; the shape is INFERRED) that falls on its first
        /// `s` periods (the input before an unrestarted reconfiguration).
        private static double Sinc3Head(double s)
        {
            if(s <= 0) return 0;
            if(s >= 3) return 1;
            if(s < 1) return s * s * s / 6;
            if(s < 2) return -s * s * s / 3 + 1.5 * s * s - 1.5 * s + 0.5;
            var u = 3 - s;
            return 1 - u * u * u / 6;
        }

        /// An R22 change of AD1CHOP between 00 and 01 only.
        private static bool ChopOnlySwitch(byte a, byte b)
        {
            return (a & ~0x18 & 0xFF) == (b & ~0x18 & 0xFF) && (a & 0x10) == 0 && (b & 0x10) == 0;
        }

        /// Unchopped AD1 (AD1CHOP 00 / 01): + / - Ad1VosCodes (the converter offset the chopper
        /// cancels; device 6 V row ~13 270 codes = 22.24 mV), weighted by the sinc^2 window
        /// (a 2-period triangle) across a polarity switch, and the unchopped gain
        /// (Ad1UnchopGainPpm, device +95 ppm vs chopped at 1 V on 6 V). Opt-in, both 0 by default.
        private double UnchopTerm(double code, double tEnd, double p)
        {
            if(Ad1Chop || (Ad1VosCodes == 0 && Ad1UnchopGainPpm == 0)) return code;
            var sNow = (regs[0x22] & 0x08) != 0 ? -1.0 : 1.0;
            var s = sNow;
            if(chopSwT > tEnd - 2 * p && chopSwT < tEnd)
            {
                var x = (tEnd - chopSwT) / p; // the window part after the switch, in periods (0..2)
                var fNew = x <= 1 ? x * x / 2 : 1 - (2 - x) * (2 - x) / 2;
                s = chopSwOld * (1 - fNew) + sNow * fNew;
            }
            else if(chopSwT >= tEnd) s = chopSwOld;
            return code * (1 + Ad1UnchopGainPpm * 1e-6) + s * Ad1VosCodes;
        }

        private void RestartLpf(double now)
        {
            lpfStart = now; lpfDone = 0;
        }

        private void UpdateAd1(double now)
        {
            if(!EnAd1) return;
            if(r22PartT <= now)
            {
                // R22PartFrac: the partial conversion across the R22 change
                ad1Last = r22PartT;
                ad1Code = Clamp24((int)Math.Round(R22PartFrac * ComputeAd1(r22PartT) + r22PartSign * R22OffCodes));
                intf |= 0x04;
                r22PartT = double.NaN;
            }
            var p = Ad1Period;
            var n = (long)Math.Floor((now - ad1Start) / p);
            var skew = Ad1Chop ? ChopSkewMs * 1e-3 : 0;
            if(skew > 0 && ((n + 1) & 1) == 1 && now >= ad1Start + (n + 1) * p - skew) n++; // ChopSkewMs: an odd result early
            if(n <= ad1Done) return;
            if(EnPkh && (regs[0x29] & 3) == 1 && ForcedAd1 == int.MinValue)
            {
                // the peak hold sees every conversion, also those between two bus accesses
                // (the last 256 of them: 0.4 s at OSR 256)
                for(var k = Math.Max(ad1Done + 1, n - 255); k < n; k++)
                    PeakHold(Clamp24(ComputeAd1(ad1Start + k * p)) >> 5);
            }
            var newConv = (int)Math.Min(n - ad1Done, int.MaxValue);
            ad1Done = n;
            ad1Last = ad1Start + n * p - ((n & 1) == 1 ? skew : 0);
            UpdateRamp(ad1Last); // the ramp node as it was when the conversion ended
            ad1Code = Clamp24(ForcedAd1 != int.MinValue ? ForcedAd1 : (int)Math.Round(UnchopTerm(ComputeAd1(ad1Last), ad1Last, p)));
            var partNow = r22PartLeft > 0 && double.IsNaN(r22PartT) && newConv <= r22PartLeft;
            if(r22PartLeft > 0) r22PartLeft = Math.Max(0, r22PartLeft - newConv);
            if(partNow && ForcedAd1 == int.MinValue)
            {
                // R22PartN: the new grid's first results read as the partial one
                ad1Code = Clamp24((int)Math.Round(R22PartFrac * ComputeAd1(ad1Last) + r22PartSign * R22OffCodes));
            }
            if(Ad1Mix && ForcedAd1 == int.MinValue && ad1MixT > double.NegativeInfinity)
            {
                // the decimation filter still holds samples of the input before a row write that
                // did not restart AD1 (same R22, ENAD1 kept): weighted by a sinc^3 over the last 3
                // periods, a chosen shape (INFERRED; the datasheet's AD1 comb is a sinc^2 over 2 T,
                // see the AD1 AC response below)
                var head = (ad1MixT - (ad1Last - 3 * p)) / p;
                if(head <= 0) ad1MixT = double.NegativeInfinity;
                else if(ad1MixT < ad1Last)
                {
                    var w = Sinc3Head(head);
                    ad1Code = Clamp24((int)Math.Round(w * ad1MixOld + (1 - w) * ad1Code));
                }
            }
            intf |= 0x04;
            if(EnPkh && (regs[0x29] & 3) == 1) PeakHold(ad1Code >> 5);
        }

        /// One AD2 result per conversion period on the grid from the last (re)start (as AD1):
        /// AD2F set once per result, R03..R05 = the input at the grid point plus the
        /// per-sample noise (Ad2NoiseCodes). A register that is not read is overwritten
        /// (no FIFO). The chip advances at CS low (the Renode peripheral calls Update
        /// there) and, in a read frame from R03, at each of its bytes (the timed SPI path,
        /// HY3131.OnFalling): such a frame across a conversion edge is torn (TornAd2), the
        /// pessimistic reading of device test 6.2 (docs/dmm-input-scope-feasibility.md 1.4,
        /// DBG:AD2:TORN?: whether the device latches R03..R05 during a read is untested).
        private void UpdateAd2(double now)
        {
            if(!EnAd2) return;
            var p = Ad2Period;
            var n = (long)Math.Floor((now - ad2Start) / p);
            if(n <= ad2Done) return;
            Ad2Conversions += n - ad2Done;
            if(EnPkh && (regs[0x29] & 3) == 0 && ForcedAd2 == int.MinValue)
            {
                // the peak hold sees every AD2 result, also those between two bus accesses
                // (the last 256 of them)
                // (the start-up results, Ad2StartGlitch, also when they are older than that)
                for(var k = ad2Done + 1; Ad2StartGlitch != 0 && k <= Ad2StartResults && k < n - 255; k++)
                    PeakHold(Clamp19(Ad2Sample(ad2Start + k * p) + Ad2StartGlitch));
                // older ones: up to 256 spread over the rest of the interval (a long gap between two
                // accesses still meets the waveform's extremes; the noise's a little less often)
                var stride = Math.Max(1, (n - 256 - ad2Done) / 256);
                for(var k = ad2Done + 1; k < n - 255; k += stride)
                    PeakHold(Clamp19(Ad2Sample(ad2Start + k * p) + (int)Math.Round(PkGauss() * Ad2NoiseCodes)));
                for(var k = Math.Max(ad2Done + 1, n - 255); k < n; k++)
                    PeakHold(Clamp19(Ad2Sample(ad2Start + k * p) + (int)Math.Round(Gauss() * Ad2NoiseCodes) + (k <= Ad2StartResults ? Ad2StartGlitch : 0)));
            }
            ad2Done = n;
            var t = ad2Start + n * p;
            ad2LastT = t;
            intf |= 0x02;
            var x = ForcedAd2 != int.MinValue ? ForcedAd2 : Ad2Sample(t) + (int)Math.Round(Gauss() * Ad2NoiseCodes) + (n <= Ad2StartResults ? Ad2StartGlitch : 0);
            ad2Code = Clamp19(x);
            if(EnPkh && (regs[0x29] & 3) == 0) PeakHold(ad2Code);
        }

        /// Per-result AD2 noise, codes rms: the device floor of the stock AC rows (~30
        /// codes at OSR2 = 32, acdc doc 6a/6b) scaled as white noise by the modulator
        /// samples per result (OSR2; the chopper does not change the rate). A model, not a
        /// measurement (test 6.4).
        public double Ad2NoiseCodes => 30.0 * NoiseScale * Math.Sqrt(32.0 / Osr2);

        private void UpdateLpf(double now)
        {
            if(!EnAd2 || !(EnLpf || EnRms)) return;
            var p = LpfPeriod;
            var n = (long)Math.Floor((now - lpfStart) / p);
            if(n <= lpfDone) return;
            var blocks = n - lpfDone;
            lpfDone = n;
            var t = lpfStart + n * p;
            if(role == Role.Power) { PowerBlock(t, p, n); return; }
            double dc, acRms;
            Ad2Window(t, p, out dc, out acRms);
            var cDc = role == Role.AcV && Ad2OnOp1Dc;
            double hpfMs = 0;
            if(cDc && EnRms && HpfTau > 0)
            {
                // the RMS block's HPF (HpfTau): its state relaxes toward the node's DC; the block's mean
                // square of what is left (the DC taken as constant since the last update)
                var a = Math.Exp(-p / HpfTau);
                var e = (dc - hpfM) * Math.Pow(a, blocks - 1);
                hpfMs = e * e * HpfTau / (2 * p) * (1 - a * a);
                hpfM = dc - e * a;
            }
            if(role == Role.AcA || (role == Role.AcV && Ad2OnOp1Dc))
            {
                double ndc, nac;
                NodeDcAc(t, out ndc, out nac);
                dc += LpfRipple(t, p, nac * Ad2Scale());
            }
            if(cDc) dc += LpfCreepCodes(t);
            double stepSq = 0;
            if(role == Role.AcA && AcaStepCodes != 0)
            {
                // the block [t - p, t] overlapping the step [acaStepT, + AcaStepMs]
                var ov = Math.Min(t, acaStepT + AcaStepMs * 1e-3) - Math.Max(t - p, acaStepT);
                var st = ov > 0 ? AcaStepCodes * Math.Min(1.0, ov / p) : 0.0;
                var te = acaStepT + AcaStepMs * 1e-3;
                if(AcaStepTauMs > 0 && t > te)
                {
                    // AcaStepTauMs: the tail's mean over the block [max(t - p, te), t]
                    var tau = AcaStepTauMs * 1e-3;
                    var a0 = Math.Max(t - p, te) - te;
                    st += AcaStepCodes * tau * (Math.Exp(-a0 / tau) - Math.Exp(-(t - te) / tau)) / p;
                }
                if(st != 0)
                {
                    dc += st;
                    stepSq = st * st;
                }
            }
            if(EnLpf)
            {
                lpfCode = Clamp19((int)Math.Round(dc));
                intf |= 0x08;
                if(EnPkh && (regs[0x29] & 3) >= 2) PeakHold(lpfCode);
            }
            if(EnRms && cDc && EnAd3 && RmsBigDcCodes > 0 && Math.Abs(dc) > RmsBigDcCodes && faultRng.NextDouble() < RmsBigDcP)
            {
                rmsVal = (long)(faultRng.NextDouble() * (1L << 38)); // RmsBigDcCodes: a garbage block
                intf |= 0x10;
            }
            else if(EnRms)
            {
                var sq = acRms * acRms + hpfMs + stepSq;
                if(RmsFill > 0 && !cDc)
                {
                    var fill = Math.Min(1.0, (t - lpfStart) / RmsFill);
                    sq *= fill * fill;
                }
                if(AcArriveMs > 0 && role == Role.AcV && !cDc)
                {
                    // AcArriveMs: the share of the block [t - p, t] after the AC's arrival
                    var ta = acvT0 + AcArriveMs * 1e-3;
                    sq *= Math.Max(0.0, Math.Min(1.0, (t - ta) / p));
                }
                if(Svxi && !Sdo23 && EnAd3)
                {
                    // sum(X*Y)/N with X and Y the same node: equal to sum(X^2)/N unless the
                    // AD2/AD3 decimation phases differ (BeatModel, empirical).
                    if(BeatModel && beatBad) sq *= 1 + 0.11 * Math.Sin(2 * Math.PI * 2.04 * t);
                }
                // AD2 clips at +-262143 codes, so the mean square cannot exceed 262143^2
                // (the stock drops sqrt(RMS) > 0x40000 as garbage: a clipped input must
                // still produce an accepted, over-range reading)
                rmsVal = (long)Math.Min(sq, 262143.0 * 262143.0);
                intf |= 0x10;
            }
        }

        private void PeakHold(int v)
        {
            if(v > pkhMax) pkhMax = v;
            if(v < pkhMin) pkhMin = v;
        }

        private static int Clamp24(long v) => (int)Math.Max(-8388608, Math.Min(8388607, v));
        private static int Clamp19(int v) => Math.Max(-262144, Math.Min(262143, v));

        // ------------------------------------------------------------ counter
        private void SetCounterEnable(bool on, double now)
        {
            if(on == ctEnabled) return;
            if(on) ctEnabled = true;
            if(!on)
            {
                // Datasheet 14: ENCTR = 0 clears CTA<7:0>, CTB, CTC and CTBOV; CTA<23:8> keeps
                // its count (it is the writable gate preset). So a counter re-enabled without
                // a preset (a row write with ENCTR = 1 after a row without it) starts from the
                // last gate's final CTA: ~0x00xx00 after an overflow, i.e. a ~3.4 s gate
                // (device 2026-10-24, DC V -> Hz: 0.00 / 0.99 / 19.94 Hz first readings).
                // CtRetain = false: the old model (the last written preset is kept).
                if(CtRetain)
                {
                    var live = CtaNow(now);
                    regs[0x1C] = (byte)(live >> 8);
                    regs[0x1D] = (byte)(live >> 16);
                }
                ctEnabled = false;
                ctB = 0; ctC = 0; ctbov = false;
                regs[0x1B] = 0;
                ctStart = double.NaN; ctStop = double.NaN;
                ctEnableTime = now;
            }
            else
            {
                ctInit = (regs[0x1D] << 16) | (regs[0x1C] << 8);
                ctEnableTime = now;
                ctStart = double.NaN; ctStop = double.NaN;
                ctStartDecided = false;
            }
        }

        /// Live CTA value.
        private int CtaNow(double now)
        {
            if(!ctEnabled) return (regs[0x1D] << 16) | (regs[0x1C] << 8);
            if(!double.IsNaN(ctStop)) return ctA;
            var t0 = double.IsNaN(ctStart) ? ctEnableTime : ctStart;
            var ticks = (long)((now - t0) * SysClk);
            return (int)((ctInit + ticks) & 0xFFFFFF);
        }

        private void UpdateCounter(double now)
        {
            if(!ctEnabled || !double.IsNaN(ctStop)) return;
            double f, duty;
            ctNow = now;
            CounterSignal(out f, out duty);
            if(f <= 0) return; // no edges: CTA runs, nothing else happens
            var period = 1.0 / f;
            if(!ctStartDecided)
            {
                // first (seen) rising edge after enable opens the gate
                // (not before the end of a ct0dead window: those edges were not seen)
                var t0 = Ct0DeadMs > 0 && role == Role.Freq && !EnCntI ? Math.Max(ctEnableTime, Ct0DeadEnd) : ctEnableTime;
                var k = Math.Ceiling((t0 - ctPhase) / period);
                if(EdgeHidden((long)k, period)) k++;
                ctStart = ctPhase + k * period;
                ctStartDecided = true;
            }
            if(now < ctStart) return;
            if(ctStart < ctChgT) { UpdateCounterSpan(now, period, duty); return; }
            var k0 = (long)Math.Round((ctStart - ctPhase) / period);
            var gate = (0x1000000 - ctInit) / SysClk;
            var overflowAt = ctStart + gate;
            var cycles = Math.Ceiling((overflowAt - ctStart) / period - 1e-9);
            if(EdgeHidden(k0 + (long)cycles, period)) cycles++;
            var stop = ctStart + cycles * period;
            if(now < stop)
            {
                // The gate is open: CTB / CTC hold the counts so far (a read now gets them,
                // with the live CTA).
                var sofar = Math.Floor((now - ctStart) / period + 1e-9);
                ctB = (int)Math.Min(0xFFFFFF, (long)sofar - EdgesHidden(k0, k0 + (long)sofar, period));
                ctC = (int)Math.Min(0xFFFFFF, Math.Round((now - ctStart) * SysClk * CtDuty(duty)));
                return;
            }
            ctStop = stop;
            var total = (long)Math.Round((stop - ctStart) * SysClk);
            ctA = (int)((ctInit + total) & 0xFFFFFF);
            var b = (long)cycles - EdgesHidden(k0, k0 + (long)cycles, period);
            if(b > 0xFFFFFF) { ctbov = true; b &= 0xFFFFFF; }
            ctB = (int)b;
            ctC = (int)Math.Min(0xFFFFFF, Math.Round(total * CtDuty(duty)));
            intf |= 0x01;
        }

        /// A gate opened on the old waveform across a frequency change of the input (SetInput,
        /// SignalHzChanged): the source restarts its waveform with a rising edge at the change
        /// (as the MR9270 calibrator's output on a frequency change, inferred from the device's
        /// 1 Hz -> 0.5 Hz step: a gate read 1.40 Hz, a period cut short). Edges: the old period
        /// from ctStart up to the change, then the change itself, then the new period. No
        /// ctsettle hidden edges here.
        private void UpdateCounterSpan(double now, double period, double duty)
        {
            double op = ctChgOldP, tc = ctChgT;
            long nOld = Math.Max(0, (long)Math.Ceiling((tc - ctStart) / op - 1e-9) - 1);
            Func<double, long> count = t => t < tc ? (long)Math.Floor((t - ctStart) / op + 1e-9)
                : nOld + 1 + (long)Math.Floor((t - tc) / period + 1e-9);
            var overflowAt = ctStart + (0x1000000 - ctInit) / SysClk;
            double stop;
            if(overflowAt < tc)
            {
                var e = ctStart + Math.Max(1, Math.Ceiling((overflowAt - ctStart) / op - 1e-9)) * op;
                stop = e < tc ? e : tc;
            }
            else stop = tc + Math.Ceiling((overflowAt - tc) / period - 1e-9) * period;
            if(now < stop)
            {
                ctB = (int)Math.Min(0xFFFFFF, count(now));
                ctC = (int)Math.Min(0xFFFFFF, Math.Round((now - ctStart) * SysClk * CtDuty(duty)));
                return;
            }
            ctStop = stop;
            var total = (long)Math.Round((stop - ctStart) * SysClk);
            ctA = (int)((ctInit + total) & 0xFFFFFF);
            ctB = (int)Math.Min(0xFFFFFF, count(stop));
            ctC = (int)Math.Min(0xFFFFFF, Math.Round(total * CtDuty(duty)));
            intf |= 0x01;
        }

        /// The counter's input frequency now (0: no edges), for SignalHzChanged.
        public double CounterInputHz()
        {
            double f, duty;
            ctNow = lastUpdate;
            CounterSignal(out f, out duty);
            return f;
        }

        /// The input's frequency changed at `now` from `oldHz`: its waveform restarts with a
        /// rising edge then (ctPhase); a gate open across it is UpdateCounterSpan's, one still
        /// waiting for its opening edge opens on it.
        public void SignalHzChanged(double now, double oldHz)
        {
            ctChgT = now;
            ctChgOldP = 1.0 / oldHz;
            ctPhase = now;
            if(ctEnabled && double.IsNaN(ctStop) && ctStartDecided && ctStart > now)
            {
                if(Ct0DeadMs > 0 && role == Role.Freq && !EnCntI && now < Ct0DeadEnd) ctStartDecided = false;
                else ctStart = now;
            }
        }
        private double ctChgT = double.NaN, ctChgOldP;

        /// Counter input settle (opt-in, "ctsettle:ms"; HYPOTHETICAL shape after the device's
        /// sub-harmonic first results, 2026-10-21 / -24: 1/3, 2/3, 1/2 of a 40 Hz square right
        /// after a row change): for ctsettle ms from every row change (role / mode / range,
        /// as SwTr) the comparator misses every other input edge, so a gate in that window
        /// counts about half the cycles. Off (0) by default.
        public double CtSettleMs;
        /// Datasheet CTA<23:8> retention across ENCTR = 0 (SetCounterEnable); "ctretain:0" = off.
        public bool CtRetain = true;
        /// Frequency row, comparator source: no edges for this long after a row change
        /// ("ct0dead:ms", CounterSignal; off by default).
        public double Ct0DeadMs;
        private double Ct0DeadEnd => swT0 + Ct0DeadMs * 1e-3;
        private double ctNow;
        private double CtSettleEnd => swT0 + CtSettleMs * 1e-3;
        private bool EdgeHidden(long k, double period)
        {
            if(CtSettleMs <= 0 || (k & 1) == 0) return false;
            var t = ctPhase + k * period;
            return t >= swT0 && t < CtSettleEnd;
        }
        /// Hidden edges with index in (k0, k1].
        private long EdgesHidden(long k0, long k1, double period)
        {
            if(CtSettleMs <= 0 || k1 <= k0) return 0;
            var a = (long)Math.Ceiling((swT0 - ctPhase) / period);
            var b = (long)Math.Ceiling((CtSettleEnd - ctPhase) / period) - 1;
            var lo = Math.Max(k0 + 1, a); var hi = Math.Min(k1, b);
            if(hi < lo || hi < 0) return 0;
            lo = Math.Max(lo, 0);
            return (hi + 1) / 2 - lo / 2; // odd k in [lo, hi]
        }
        /// The input as the counter sees it: high through the missed half-periods of the settle.
        private double CtDuty(double duty) => CtSettleMs > 0 && lastUpdate < CtSettleEnd && lastUpdate >= swT0 ? 0.5 + duty / 2 : duty;

        /// The signal at the counter input: frequency (Hz) and high fraction.
        private void CounterSignal(out double f, out double duty)
        {
            f = 0; duty = 0;
            // ct0dead: the frequency row's comparator source (ENCNTI = 0) counts no edges for
            // ct0dead ms after the row change (device d2aff22: 0 Hz / 0 % in source 0xf0's
            // first gates after every Hz entry; source 0x08 read the signal).
            if(Ct0DeadMs > 0 && role == Role.Freq && !EnCntI && ctNow < Ct0DeadEnd) return;
            if(role == Role.CapOsc)
            {
                var r = CapOscR();
                var c = Fe.CapOscC(r);
                if(double.IsNaN(c)) return; // the network cannot oscillate
                c += CalOffs(0x190) * 1e-6;  // stray capacitance (the cal offset)
                if(c <= 0) return;
                f = CalGain(0x170 + 4 * roleRange) * 0.72135 / (r * c);
                duty = 0.5;
                return;
            }
            double dc, ac;
            NodeDcAc(0, out dc, out ac);
            if(EnCntI)
            {
                // CNT pin: the DM40 wiring is unknown; assume the same node (RESEARCH.md 8.4)
                SignalEdges(ac, out f, out duty, 0.0, 0.0);
                return;
            }
            if(!EnCmp) return;
            switch(role)
            {
            case Role.Power:
            {
                // SENSE is DC coupled (the V tap: its DC counts against the window); PB4 is
                // OP1O coupled (the current's AC, around AGND).
                var scmpi = regs[0x20] >> 5;
                if(scmpi == 7)
                {
                    double idc, iac, ihz, iduty; string ishape;
                    Fe.ACurrent(out idc, out iac, out ihz, out ishape, out iduty);
                    sigHz = ihz; sigShape = ishape; sigDuty = iduty;
                    SignalEdgesDc(0, iac * Fe.ShuntR * ((regs[0x25] & 1) != 0 ? 10 : 1), out f, out duty, CmpHighThreshold, CmpLowThreshold);
                }
                else if(scmpi == 0) SignalEdgesDc(dc, ac, out f, out duty, CmpHighThreshold, CmpLowThreshold);
                break;
            }
            case Role.AcV: case Role.AcMv: case Role.Freq: case Role.DcV: case Role.DcMv: case Role.AcA: case Role.DcA:
                SignalEdges(ac, out f, out duty, CmpHighThreshold, CmpLowThreshold);
                break;
            }
        }

        private void SignalEdges(double rms, out double f, out double duty, double vrh, double vrl)
        {
            f = 0; duty = 0;
            var shape = sigShape;
            var pk = Peak(rms, shape);
            if(pk <= Math.Max(Math.Abs(vrh), Math.Abs(vrl))) return;
            f = sigHz;
            switch(shape)
            {
            case "square":
                duty = sigDuty;
                break;
            case "triangle":
                duty = 0.5 + (Math.Abs(vrl) - vrh) / (4 * pk);
                break;
            default:
                duty = 0.5 + (Math.Asin(Math.Min(1, Math.Abs(vrl) / pk)) - Math.Asin(Math.Min(1, vrh / pk))) / (2 * Math.PI);
                break;
            }
        }

        /// A DC-coupled node (dc + the AC waveform) against the window: it toggles only when the
        /// waveform crosses both thresholds (a unipolar signal never reaches a negative VRL).
        private void SignalEdgesDc(double dc, double rms, out double f, out double duty, double vrh, double vrl)
        {
            f = 0; duty = 0;
            var shape = sigShape;
            var pk = Peak(rms, shape);
            if(rms <= 0 || dc + pk <= vrh || dc - pk >= vrl) return;
            f = sigHz;
            double h = vrh - dc, l = vrl - dc;
            switch(shape)
            {
            case "square": duty = sigDuty; break;
            case "triangle": duty = 0.5 - (h + l) / (4 * pk); break;
            default: duty = 0.5 - (Math.Asin(Math.Max(-1, Math.Min(1, h / pk))) + Math.Asin(Math.Max(-1, Math.Min(1, l / pk)))) / (2 * Math.PI); break;
            }
        }

        private static double Peak(double rms, string shape)
        {
            switch(shape)
            {
            case "square": return rms;
            case "triangle": return rms * Math.Sqrt(3);
            default: return rms * Math.Sqrt(2);
            }
        }

        // ------------------------------------------------------------ AC node (AD2 input)
        private void UpdateAcNode(double now)
        {
            var dt = now - vcLast;
            vcLast = now;
            if(dt <= 0) return;
            // the coupling capacitor charges to the OP1O DC level (or its off level)
            var src = Op1OffLevel;
            if(EnOp1 && (role == Role.AcV || role == Role.DcV))
            {
                double dc, ac, hz, duty; string shape;
                Fe.VJack(0, out dc, out ac, out hz, out shape, out duty);
                // AC V: toward the same calibration-adjusted level NodeDcAc measures the step against
                // (else the difference, 1.5 x V x the cal error, never decays: ~18 mV RMS on 1.234 V DC)
                src = role == Role.AcV ? DcQ(dc, 0x64 + 4 * roleRange, 0x74 + 4 * roleRange) / (AcvK[roleRange] * Ad2IgRow())
                                       : dc * NodeRatioAc();
            }
            vc += (src - vc) * (1 - Math.Exp(-dt / AcNodeTau));
        }

        /// Instantaneous AD2 code.
        private int Ad2Sample(double t)
        {
            if(role == Role.Power) return (int)Math.Round(PowerSample(t));
            if(Ad2OnPb2Probe)
            {
                // opendm40 A-jack protection probe (DC V / mV / ohm / temperature): OP2 (x1) on
                // PB2, AD2 = OP2O - PB3 = the 10 mohm element's drop, instantaneous
                return (int)Math.Round(Pb2Volts(t) * Ad2Scale());
            }
            if(Ad2OnOp1Jack)
            {
                // opendm40 jack check: AD2 = OP1O - PB3 = the row's input node, instantaneous
                return (int)Math.Round(JackOp1Volts(t) * Ad2Scale());
            }
            if(Ad2OnPb1Sentinel)
            {
                // opendm40 sentinel (continuity / diode / capacitance r0..4): OP2 (x1) on PB1, AD2 =
                // OP2O - PB3 = the A-jack shunt drop, instantaneous
                return (int)Math.Round(Pb1Volts(t) * Ad2Scale());
            }
            double dc, ac;
            NodeDcAc(t, out dc, out ac);
            var w = 2 * Math.PI * sigHz * t;
            var pk = Peak(ac, sigShape);
            double s;
            switch(sigShape)
            {
            case "square": s = ((w / (2 * Math.PI)) % 1.0) < sigDuty ? pk : -pk; break;
            case "triangle": { var ph = (w / (2 * Math.PI)) % 1.0; s = pk * (ph < 0.5 ? 4 * ph - 1 : 3 - 4 * ph); break; }
            default: s = pk * Math.Sin(w); break;
            }
            return (int)Math.Round((dc + s) * Ad2Scale());
        }

        /// Mean and AC-RMS of the AD2 node over the LPF window, in AD2 codes.
        private void Ad2Window(double t, double window, out double dcCode, out double acCode)
        {
            double dc, ac;
            NodeDcAc(t, out dc, out ac);
            var scale = Ad2Scale();
            dcCode = dc * scale;
            var floor = 30.0 * NoiseScale;
            acCode = Math.Sqrt(ac * ac * scale * scale + floor * floor);
            if(ac > 0 && NoiseScale > 0) acCode += Gauss() * 1.0 * NoiseScale;
        }

        /// The opendm40 AD2 sentinel configuration: ENOP2 with SOP2P = PB1, SAD2IP = OP2O,
        /// SAD2IN = PB3, in a role whose own measurement leaves AD2 / OP2 unused.
        private bool Ad2OnPb1Sentinel =>
            (role == Role.Cont || role == Role.Diode || role == Role.CapOsc)
            && (regs[0x32] & 0xF0) == 0xD0 && (regs[0x27] >> 6) == 1 && ((regs[0x27] >> 4) & 3) == 2;

        /// The opendm40 A-jack protection probe (sentinel.rs `probe_row`): ENOP2 with SOP2P = PB2,
        /// SAD2IP = OP2O, SAD2IN = PB3, over a comparator role (OP2 / AD2 unused there).
        private bool Ad2OnPb2Probe =>
            (role == Role.DcV || role == Role.DcMv || role == Role.Ohm || role == Role.Temp)
            && (regs[0x32] & 0xF0) == 0xE0 && (regs[0x27] >> 6) == 1 && ((regs[0x27] >> 4) & 3) == 2;

        /// The opendm40 jack check (sentinel.rs `jack_row` / continuity's jack window): AD2 = OP1O -
        /// PB3 in the ohm / continuity rows, OP1 (x1) on the row's own input (PB0 / SENSE).
        private bool Ad2OnOp1Jack =>
            (role == Role.Ohm || role == Role.Cont) && EnOp1
            && (regs[0x27] >> 6) == 0 && ((regs[0x27] >> 4) & 3) == 2;

        /// OP1O in the ohm / continuity rows, instantaneous: the node across Rx (Fe.OhmNode, DC) plus
        /// the AC part of a source on the jack, within OP1's output swing (AGND - 1.18 V .. VDDA =
        /// AGND + 2.55 V, the source map's VSS / VDDA; the clip itself is INFERRED).
        private double JackOp1Volts(double t)
        {
            double vs, rref, vx, vref;
            OhmSource(out vs, out rref);
            Fe.OhmNode(vs, rref, out vx, out vref);
            double dc, ac, hz, duty; string shape;
            Fe.VJack(Fe.RelayPa, out dc, out ac, out hz, out shape, out duty);
            if(Fe.RelayPa == 1 && ac > 0) vx += Wave(t, dc, ac, hz, shape, duty) - dc;
            // ohm: the buffer stops below the source (OhmOp1K, device 1a11eda)
            if(role == Role.Ohm && OhmOp1K > 0) vx = Math.Min(vx, OhmOp1V0 - OhmOp1K * Math.Max(vx, 0));
            return Math.Max(-1.18, Math.Min(2.55, vx));
        }

        /// The opendm40 DC-coupled V row (ad2exp.rs dcv_row): an AC V row with OP1 on and SAD2IP = OP1O.
        private bool Ad2OnOp1Dc => role == Role.AcV && EnOp1 && (regs[0x27] >> 6) == 0;

        /// OP1 chopper polarity at t: OP1CHOP 1 / 2 = 1 / 2 kHz, running only while AD1 converts
        /// (device, sentinel.rs); held (+1) otherwise and for 0 / 3.
        private double Op1ChopSign(double t)
        {
            var c = regs[0x33] >> 6;
            if(!EnAd1 || c == 0 || c == 3) return 1;
            return ((t * 1000.0 * c) % 1.0) < 0.5 ? 1 : -1;
        }

        /// Codes per volt at the AD2 input: IG / (RG * VR) * 2^18.
        private double Ad2Scale() => Ad2Ig / (Ad2Rg * Vref) * 262144.0;

        /// DC level and AC RMS (V, at the AD2 / OP1 input node) for the current role, with the
        /// calibration deviations folded in. Also records the waveform parameters (sigHz ...).
        private void NodeDcAc(double t, out double dc, out double ac)
        {
            dc = 0; ac = 0;
            double jdc, jac, hz, duty; string shape;
            switch(role)
            {
            case Role.AcV:
            {
                var r = roleRange;
                Fe.VJack(0, out jdc, out jac, out hz, out shape, out duty);
                sigHz = hz; sigShape = shape; sigDuty = duty;
                if(Ad2OnOp1Dc)
                {
                    // opendm40 DC-coupled V row: OP1O = g x (SENSE + Vos), SENSE = the jack through the row's
                    // tap (r0: /100 with OP1 x10, OPS1; r1..r3 /100 .. /10000), DC-coupled; OP1+ on AGND
                    // (SOP1P 011) = the offset alone; OP1's chopper flips Vos while AD1 converts
                    var g = (regs[0x25] & 1) != 0 ? 10.0 : 1.0;
                    var k = AcvK[Math.Max(r, 1)];
                    var sense = (regs[0x32] & 7) == 0;
                    dc = ((sense ? jdc / k : 0) + Op1Vos * Op1ChopSign(t)) * g * Ad2DcGain / Ad2DcGainR[r];
                    if(EnAd3 && (regs[0x35] >> 6) == 0) dc += Ad3ShiftCodes[r] * Ad3ShiftScale / Ad2Scale();
                    if(!sense) dc += AgndZeroCodes[r] * AgndZeroScale / Ad2Scale();
                    // the AC part as the AC V row's PB4 node: the factory AC V calibration applies
                    ac = sense && jac > 0 ? AcQ(jac, 0x34 + 4 * r, 0x44 + 4 * r) / (AcvK[r] * Ad2IgRow()) : 0;
                    break;
                }
                // display = sqrt(RMS) * VR / 2^18 * k and sqrt(RMS) = IG * Vnode / VR * 2^18
                // -> Vnode = display / (k * IG)
                ac = AcQ(jac, 0x34 + 4 * r, 0x44 + 4 * r) / (AcvK[r] * Ad2IgRow());
                var dcNode = DcQ(jdc, 0x64 + 4 * r, 0x74 + 4 * r) / (AcvK[r] * Ad2IgRow());
                var step = dcNode - vc;                                   // AC coupled: only the un-charged part is seen
                dc = step;
                ac = Math.Sqrt(ac * ac + Math.Pow(HpfTransientGain * step, 2));
                break;
            }
            case Role.AcMv:
            {
                var r = roleRange;
                Fe.VJack(1, out jdc, out jac, out hz, out shape, out duty);
                sigHz = hz; sigShape = shape; sigDuty = duty;
                if(!double.IsNaN(Fe.MvHoldFor(true))) { jdc = Fe.MvHoldFor(true); jac = 0; }
                if(MvClampV > 0) MvClamp(jdc, jac, hz, shape, duty, out jdc, out jac);
                ac = AcQ(jac * 1000, 0x54 + 4 * r, 0x5c + 4 * r) / 1000 * MvDiv[r] / Ad2IgRow();
                dc = DcQ(jdc * 1000, 0x84 + 4 * r, 0x8c + 4 * r) / 1000 * MvDiv[r] / Ad2IgRow();
                break;
            }
            case Role.AcA:
            {
                var m = roleMode; var r = roleRange; var i = (m - 4) * 2 + r;
                var unit = AUnit[m - 4];
                Fe.ACurrent(out jdc, out jac, out hz, out shape, out duty);
                sigHz = hz; sigShape = shape; sigDuty = duty;
                // the drop across the shunt the relays selected, through the row's OP1 gain
                var g = Fe.ShuntR * MvDiv[r] / unit;
                ac = AcQ(jac * unit, 0xc4 + 4 * i, 0xdc + 4 * i) * g / Ad2IgRow();
                dc = DcQ(jdc * unit, 0xf4 + 4 * i, 0x10c + 4 * i) * g / Ad2IgRow();
                dc += AcaOffMv * 1e-3 * MvDiv[r] / Ad2IgRow();
                break;
            }
            case Role.Freq:
                Fe.VJack(1, out jdc, out jac, out hz, out shape, out duty);
                sigHz = hz; sigShape = shape; sigDuty = duty;
                ac = jac; dc = jdc;
                break;
            case Role.Power:
                // SENSE: the V divider tap (what the comparator / counter sees)
                Fe.VJack(0, out jdc, out jac, out hz, out shape, out duty);
                sigHz = hz; sigShape = shape; sigDuty = duty;
                dc = jdc / AcvK[PowVr]; ac = jac / AcvK[PowVr];
                break;
            case Role.DcV:
                Fe.VJack(0, out jdc, out jac, out hz, out shape, out duty);
                sigHz = hz; sigShape = shape; sigDuty = duty;
                dc = jdc * NodeRatioAc(); ac = jac * NodeRatioAc();
                break;
            case Role.DcMv:
                Fe.VJack(1, out jdc, out jac, out hz, out shape, out duty);
                sigHz = hz; sigShape = shape; sigDuty = duty;
                if(!double.IsNaN(Fe.MvHoldFor(false))) { jdc = Fe.MvHoldFor(false); jac = 0; }
                dc = jdc * NodeRatioAc(); ac = jac * NodeRatioAc();
                break;
            case Role.DcA:
                Fe.ACurrent(out jdc, out jac, out hz, out shape, out duty);
                sigHz = hz; sigShape = shape; sigDuty = duty;
                dc = jdc * Fe.ShuntR * MvDiv[roleRange]; ac = jac * Fe.ShuntR * MvDiv[roleRange];
                break;
            }
        }

        /// Quantity -> what the stock will display after calibration = input: Q' = (Q + offs) / gain.
        private double AcQ(double q, int gainOff, int offsOff) => (q + CalOffs(offsOff)) / CalGain(gainOff);
        private double DcQ(double q, int gainOff, int offsOff) => (q + CalOffs(offsOff)) / CalGain(gainOff);

        private double Ad2IgRow() => rowIg2;

        /// Node volts per V-jack volt for the voltage roles.
        private double NodeRatioAc()
        {
            switch(role)
            {
            case Role.AcV: return 1.0 / (AcvK[roleRange] * rowIg2);
            case Role.DcV: return 1.0 / (AcvK[roleRange] * 1.0);
            case Role.AcMv: case Role.DcMv: return MvDiv[roleRange];
            default: return 1.0;
            }
        }

        // ------------------------------------------------------------ AD1
        /// AD1 code for a conversion ending at time t.
        private int ComputeAd1(double t)
        {
            double code;
            var ig = Ad1Ig; var rg = Ad1Rg;
            var fs = 8388608.0;
            double jdc, jac, hz, duty; string shape;
            var fp = regs[0x24] >> 4; var fn_ = regs[0x24] & 7;
            if((role == Role.AcV || role == Role.AcMv || role == Role.Freq) && fp == 9 && fn_ == 5)
            {
                // opendm40 sentinel: AD1 on PB1 - PB3 (the A-jack shunt drop, PB3 = its return),
                // reference PB6 - AGND: code = IG * V / (RG * VR) * 2^23, instantaneous (OSR 256)
                code = ig * Pb1Volts(t) / (rg * Vref) * fs;
                if(NoiseScale > 0) code += Gauss() * 25.0 * Math.Sqrt(32768.0 / Osr1) * NoiseScale;
                return (int)Math.Round(Math.Max(-1e9, Math.Min(1e9, code + Dcos / rg * fs)));
            }
            if(role == Role.AcV && fp == 0 && fn_ == 7)
            {
                // opendm40 DC? hint (AC V): AD1 on SENSE - PB5, SENSE = the V jack through the AC V
                // range's divider tap (r0 / r1: DC V r1's /100, r2 /1000, r3 /10000; INFERRED from
                // the R2C / R2D taps), instantaneous
                // (the DC V calibration of that tap folded in, as the DC V rows: the opendm40 DC-coupled
                // V row's self-cal reads AD1 here and scales it with that calibration)
                // instantaneous at the sentinel's OSR 256; a long conversion (OSR >= 2048, the self-cal's
                // 4096) integrates: the DC part (no AC ripple sampled at 4 points)
                var kt = Math.Max(roleRange, 1);
                double vdc, vac, vhz, vduty; string vsh;
                Fe.VJack(0, out vdc, out vac, out vhz, out vsh, out vduty);
                var vt = Osr1 >= 2048 ? N7(vdc + Ad1Ripple(t, vac, vhz, vsh, vduty), roleRange) / AcvK[kt] : VTapVolts(t, AcvK[kt]);
                vt *= 1 + Ad1AcRowGain[Math.Min(3, roleRange)] * Ad1AcRowScale;
                code = ig * DcQ(vt * AcvK[kt], 0x04 + 4 * kt, 0x14 + 4 * kt) / AcvK[kt] / (rg * Vref) * fs;
                if(NoiseScale > 0) code += Gauss() * 25.0 * Math.Sqrt(32768.0 / Osr1) * NoiseScale;
                return (int)Math.Round(Math.Max(-1e9, Math.Min(1e9, code + Dcos / rg * fs)));
            }
            if(role == Role.AcV && fp == 3 && fn_ == 1)
            {
                // opendm40 AC+DC method B, 6 V candidate (ii) (dmm.rs acdc_row): AD1 = OP1O - RLU in the
                // AC V row, OP1O = (the /100 tap + Vos) x OP1's gain (x10 at r0), chopper held; the
                // DC V 6 V calibration folded in (the same /10 overall ratio as DC V 6 V's PA7 tap)
                double vdc, vac, vhz, vduty; string vsh;
                Fe.VJack(0, out vdc, out vac, out vhz, out vsh, out vduty);
                var g = (regs[0x25] & 1) != 0 ? 10.0 : 1.0;
                var jq = DcQ(N7(vdc + Ad1Ripple(t, vac, vhz, vsh, vduty), roleRange), 0x04, 0x14);
                var node = (jq / AcvK[Math.Max(roleRange, 1)] + Op1Vos) * g;
                code = ig * node / (rg * Vref) * fs;
                if(NoiseScale > 0) code += Gauss() * 25.0 * Math.Sqrt(32768.0 / Osr1) * NoiseScale;
                return (int)Math.Round(Math.Max(-1e9, Math.Min(1e9, code + Dcos / rg * fs)));
            }
            if(role == Role.AcA && fp == 3 && fn_ == 5)
            {
                // opendm40 AC+DC method B (dmm.rs acdc_row): AD1 = OP1O - PB3 in the AC A row, as the DC A
                // row of the same shunt / OP1 gain (its calibration folded in), note 7 "n7a:" codes
                var m = roleMode >= 5 ? (Fe.ShuntSel == 2 ? 6 : 5) : 4; var r = roleRange; var i = (m - 4) * 2 + r;
                double adc, aac, ahz, aduty; string ash;
                Fe.ACurrent(out adc, out aac, out ahz, out ash, out aduty);
                var unit = AUnit[m - 4];
                var q = DcQ((adc + Ad1Ripple(t, aac, ahz, ash, aduty)) * unit, 0x94 + 4 * i, 0xac + 4 * i) / unit;
                code = q * Fe.ShuntR * MvDiv[r] * 0.9 / 1.25 * 8388606.0 * (ig / rowIg) / (rg / rowRg);
                if(N7On) code += N7ACodes * N7Scale;
                if(NoiseScale > 0) code += Gauss() * 25.0 * Math.Sqrt(32768.0 / Osr1) * NoiseScale;
                return (int)Math.Round(Math.Max(-1e9, Math.Min(1e9, code + Dcos / rg * fs)));
            }
            if(role == Role.AcA && fp == 0 && (fn_ == 7 || fn_ == 3))
            {
                // opendm40 sentinel (AC A V-JACK?): AD1 on SENSE - PB5 (40728b8) or SENSE - AGND,
                // SENSE = the V jack through the /1000 tap with SS5 closed (open otherwise),
                // instantaneous; PB5 drifts with pb5drift (above)
                var vin = SenseTapVolts(t);
                if(fn_ == 7 && Pb5DriftV != 0 && t >= acaT0) vin -= Pb5DriftV * (1 - Math.Exp(-(t - acaT0) / Pb5DriftTau));
                if(SenseNegV != 0 && t >= acaT0) vin -= SenseNegV * (1 - Math.Exp(-(t - acaT0) / SenseNegTau));
                code = ig * vin / (rg * Vref) * fs;
                if(NoiseScale > 0) code += Gauss() * 25.0 * Math.Sqrt(32768.0 / Osr1) * NoiseScale;
                return (int)Math.Round(Math.Max(-1e9, Math.Min(1e9, code + Dcos / rg * fs)));
            }
            if((role == Role.Ohm || role == Role.Diode) && SrcDirect())
            {
                // opendm40 DBG:SRC: a direct node read, saturating at full scale
                code = SrcCode(ig, rg, fs);
                if(NoiseScale > 0) code += Gauss() * 25.0 * Math.Sqrt(32768.0 / Osr1) * NoiseScale;
                return (int)Math.Round(Math.Max(-(fs - 1), Math.Min(fs - 1, code + Dcos / rg * fs)));
            }
            switch(role)
            {
            case Role.DcV:
            {
                var r = roleRange;
                Fe.VJack(0, out jdc, out jac, out hz, out shape, out duty);
                var q = jdc + Ad1Ripple(t, jac, hz, shape, duty) + vNoise * Gauss();
                q = N7(q, r);
                // stock: V = AD1 * k * 1.38889 / 8388606 * gain - offs (+ negative-side gain, r0 correction)
                if(q < 0) q /= DcvNegK[r];
                if(r == 0 && Math.Abs(q) / 20000 > 3e-5) q -= 1e-4 - Math.Abs(q) / 20000;
                code = DcQ(q, 0x04 + 4 * r, 0x14 + 4 * r) / (AcvK[r] * 1.38889 / 8388606.0);
                code *= (ig / rowIg) / (rg / rowRg);
                break;
            }
            case Role.DcMv:
            {
                var r = roleRange;
                Fe.VJack(1, out jdc, out jac, out hz, out shape, out duty);
                if(!double.IsNaN(Fe.MvHoldFor(false))) jdc = Fe.MvHoldFor(false);
                var q = jdc * 1000 + vNoise * 1000 * Gauss();
                code = DcQ(q, 0x24 + 4 * r, 0x2c + 4 * r) / (1.38889 / MvDiv[r] / 8388.606);
                code *= (ig / rowIg) / (rg / rowRg);
                break;
            }
            case Role.Power:
            {
                // opendm40 DC coupling (powermeter.rs dc_row): AD1 = OP1O - PB3 on the shunt, as the
                // stock DC A row (its calibration folded in); "powz:1" adds an AD1 zero of 300 codes
                var iu = PowIu; var ir = PowIr; var i = iu * 2 + ir;
                if((regs[0x24] >> 4) != 3) { code = 0; break; }
                Fe.ACurrent(out jdc, out jac, out hz, out shape, out duty);
                var unit = AUnit[iu];
                var q = DcQ(jdc * unit, 0x94 + 4 * i, 0xac + 4 * i) / unit;
                code = q * Fe.ShuntR * MvDiv[ir] * 0.9 / 1.25 * 8388606.0;
                code *= (ig / rowIg) / (rg / rowRg);
                if(PowZeros) code += 300;
                break;
            }
            case Role.DcA:
            {
                // The mA and uA rows are the same bytes (f8_9): the shunt relays (PE) tell the
                // unit, as in the stock (mode 8: PE1, mode 9: none = the 47.5 ohm shunt).
                var m = roleMode >= 8 ? (Fe.ShuntSel == 2 ? 9 : 8) : roleMode; var r = roleRange; var i = (m - 7) * 2 + r;
                Fe.ACurrent(out jdc, out jac, out hz, out shape, out duty);
                var unit = AUnit[m - 7];
                var q = DcQ((jdc + Ad1Ripple(t, jac, hz, shape, duty)) * unit, 0x94 + 4 * i, 0xac + 4 * i) / unit; // A, calibration folded in
                // drop across the relay-selected shunt through the row's OP1 gain: IG * V / VR * 2^23
                code = q * Fe.ShuntR * MvDiv[r] * 0.9 / 1.25 * 8388606.0;
                code *= (ig / rowIg) / (rg / rowRg);
                if(N7On) code += N7ACodes * N7Scale;
                break;
            }
            case Role.Ohm: case Role.Cont:
            {
                // ratiometric: VIN = drop across Rx (OP1O - PB3), VR = drop across the reference
                // resistor (FB - RLU, or RLU - FB in the continuity row -> negative)
                double vs, rref;
                OhmSource(out vs, out rref);
                double vx, vref;
                Fe.OhmNode(vs, rref, out vx, out vref);
                if(OhmTauMs > 0 && role == Role.Ohm)
                {
                    OhmLagAdvance(t);
                    ohmLagTarget = vx;
                    vx = ohmLagV;
                    vref = vs - vx;
                }
                var pol = ((regs[0x28] >> 4) & 7) == 4 && (regs[0x28] & 7) == 4 ? -1.0 : 1.0;
                // open jack: vx -> vs, vref -> 0 with the same sign: the ratio goes to +infinity
                if(Math.Abs(vref) < 1e-9) { code = pol * 1e12; break; }
                // a source above the open-circuit voltage (vref reversed): OL on the device (OhmRevOl)
                if(OhmRevOl && role == Role.Ohm && vref * vs < 0 && vx * vs > 0) { code = pol * 1e12; break; }
                code = 0.9 * vx / vref * 8388606.0 * pol;
                code *= ig / rowIg;
                break;
            }
            case Role.Diode:
            {
                var vx = Fe.DiodeNode(DiodeTestCurrent, DiodeCompliance);
                var q = (vx + CalOffs(0x168)) / CalGain(0x164);
                code = q / (1.25 * 10 / 0.9 / 8388606.0);
                code *= (ig / rowIg) / (rg / rowRg);
                break;
            }
            case Role.CapRamp: case Role.CapDischarge:
                // node volts -> code with the stock scale 1000 * 1.25 / 0.9 / 8388606 mV per code
                code = (role == Role.CapRamp && CapVAvg > 0 ? RampMean(ad1Last) : rampV) / (1.25 / 0.9 / 8388606.0) * (ig / rowIg) / (rg / rowRg);
                break;
            case Role.Temp:
            {
                if(Sad1I >= 2)
                {
                    // internal sensor: codeA (SAD1I 11) - codeB (SAD1I 10) = (Tdie + 289.15 - cal) / 314.15 * 640000
                    var d = (Fe.Die + 289.15 - CalOffs(0x194)) / 314.15 * 640000.0;
                    code = Sad1I == 3 ? d / 2 : -d / 2;
                    code *= ig / rowIg;
                }
                else
                {
                    // the thermocouple (or whatever is on the jacks) on the mV x10 path
                    double mv;
                    if(Fe.TcConnectedOpen) mv = 200; // open thermocouple: far over range
                    else { Fe.VJack(1, out jdc, out jac, out hz, out shape, out duty); mv = jdc * 1000; }
                    code = DcQ(mv, 0x24, 0x2c) / (1.38889 * 100 / 8388606.0);
                    code *= (ig / rowIg) / (rg / rowRg);
                }
                break;
            }
            case Role.Freq:
                code = 0;
                break;
            default:
                code = 0;
                break;
            }
            if(role != Role.Temp || Sad1I < 2) code += Dcos / rg * fs;
            if(NoiseScale > 0 && role != Role.None)
            {
                code += Gauss() * 25.0 * Math.Sqrt(32768.0 / Osr1) * NoiseScale;
            }
            return (int)Math.Round(Math.Max(-1e9, Math.Min(1e9, code)));
        }

        /// AD1's response to the AC part of the input (amplitude `ac` RMS) for the conversion ending at t:
        /// at OSR >= 2048 a sinc^2 decimator (datasheet "Comb Filter 2": a triangle over 2 T, T = OSR /
        /// AD1CLK) and, chopped, the mean of two such sub-conversions T apart: |H| = sinc^2(f T) x
        /// |cos(pi f T)| (INFERRED, docs/acdc-approaches.md 3: notches every 10 Hz at OSR 8192 chopped,
        /// so 40 / 50 / 60 Hz vanish and off-notch ripple leaks). Below OSR 2048 the AD1 paths that use
        /// this are instantaneous (the sentinels' OSR 256 peak hold) and it returns 0.
        private double Ad1Ripple(double t, double ac, double hz, string shape, double duty)
        {
            if(ac <= 0 || hz <= 0 || Osr1 < 2048) return 0;
            var T = Osr1 * 30.0 / SysClk;
            const int N = 48;
            double s = 0, ws = 0;
            for(var sub = 0; sub < (Ad1Chop ? 2 : 1); sub++)
                for(var i = 0; i < 2 * N; i++)
                {
                    var x = (i + 0.5) / N;
                    var w = 1 - Math.Abs(x - 1);
                    s += w * Wave(t - sub * T - x * T, 0, ac, hz, shape, duty);
                    ws += w;
                }
            return s / ws;
        }

        /// LpfCreep: advance the lag to t and return the LPF's extra codes (plus its noise, LpfDcNoise).
        private double LpfCreepCodes(double t)
        {
            double jdc, jac, hz, duty; string shape;
            Fe.VJack(0, out jdc, out jac, out hz, out shape, out duty);
            var r = roleRange;
            if(double.IsNaN(creepV) || LpfCreepTau <= 0 || r != creepR || t - creepT > CreepGap) { creepV = jdc; creepV1 = jdc; creepV2 = jdc; }
            else if(t > creepT)
            {
                // the input constant since creepT: each stage relaxes and takes the earlier ones' share
                // (the exact step of LpfCreepOrder cascaded equal lags; the output is the last stage)
                var dt = t - creepT; var a = Math.Exp(-dt / LpfCreepTau); var q = dt / LpfCreepTau;
                var e1 = creepV1 - jdc; var e2 = creepV2 - jdc; var e3 = creepV - jdc;
                if(LpfCreepOrder >= 3)
                {
                    creepV1 = jdc + e1 * a;
                    creepV2 = jdc + (e2 + e1 * q) * a;
                    creepV = jdc + (e3 + e2 * q + e1 * q * q / 2) * a;
                }
                else
                {
                    creepV1 = jdc + e1 * a; creepV2 = creepV1;
                    creepV = jdc + (LpfCreepOrder >= 2 ? e3 + e1 * q : e3) * a;
                }
            }
            creepT = Math.Max(creepT, t); creepR = r;
            var sense = (regs[0x32] & 7) == 0;
            var g = (regs[0x25] & 1) != 0 ? 10.0 : 1.0;
            var perV = g * Ad2DcGain / Ad2DcGainR[r] / AcvK[Math.Max(r, 1)] * Ad2Scale();
            var x = sense ? LpfCreep * LpfCreepScale * (creepV - jdc) * perV : 0;
            if(LpfDcNoise > 0 && NoiseScale > 0)
            {
                var u1 = 1.0 - lpfNoiseRng.NextDouble(); var u2 = lpfNoiseRng.NextDouble();
                x += LpfDcNoise * LpfDcNoiseR[Math.Max(0, Math.Min(r, 3))] * NoiseScale * Math.Sqrt(-2 * Math.Log(u1)) * Math.Cos(2 * Math.PI * u2);
            }
            return x;
        }

        /// The LPF's response to the AC part (`acCodes` RMS at the node) for the block ending at t:
        /// INFERRED as a sinc^2 over two LPF periods (2 x 106.7 ms at LPFBW 111: the datasheet's 213 ms
        /// window, results every 106.7 ms on the device), i.e. -51 dB at 50 Hz, consistent with the
        /// device bound <= 0.5 % at 40 Hz (acdc-approaches.md 3 "LPF ripple rejection").
        private double LpfRipple(double t, double p, double acCodes)
        {
            if(acCodes <= 0 || sigHz <= 0) return 0;
            // the triangle window over [t - 2p, t] (weight 1 - |tau / p - 1|, integral p), exactly: a
            // sine through its transform sinc^2(f p); a square piece by piece between its edges (a sampled
            // window aliases the edges into a DC bias of several % of the amplitude)
            var pk = Peak(acCodes, sigShape);
            if(sigShape == "square")
            {
                // the window's integral from tau = 0 to x (x in [0, 2p]; p / 2 at p, p at 2p)
                Func<double, double> Wx = x => x <= p ? x * x / (2 * p) : p - (2 * p - x) * (2 * p - x) / (2 * p);
                var per = 1.0 / sigHz;
                var eps = 1e-9 * per;
                double acc = 0, tau = 0;
                while(tau < 2 * p - eps)
                {
                    var ph = (((t - tau - eps) * sigHz) % 1.0 + 1.0) % 1.0; // just before t - tau
                    var lvl = ph < sigDuty ? pk : -pk;
                    var tb = Math.Min(2 * p, tau + eps + (ph < sigDuty ? ph : ph - sigDuty) * per); // back to the edge
                    acc += lvl * (Wx(tb) - Wx(tau));
                    tau = tb;
                }
                return acc / p;
            }
            if(sigShape != "triangle")
            {
                var x = Math.PI * sigHz * p;
                var sinc = Math.Abs(x) < 1e-9 ? 1.0 : Math.Sin(x) / x;
                return pk * sinc * sinc * Math.Sin(2 * Math.PI * ((sigHz * (t - p)) % 1.0));
            }
            const int N = 64;
            double s = 0, ws = 0;
            for(var i = 0; i < 2 * N; i++)
            {
                var x = (i + 0.5) / N;
                var w = 1 - Math.Abs(x - 1);
                s += w * Wave(t - x * p, 0, acCodes, sigHz, sigShape, sigDuty);
                ws += w;
            }
            return s / ws;
        }

        /// Datasheet 13.1 note 7 on the device (feasibility 13.2): AD1 on the V divider reads shifted
        /// while AD2 / AD3 convert. N7Volts = the measured offsets at the jack per V range (repeat to
        /// ~1 %), "n7:" scales them (0 = off); "n7g:" the amplitude of a gain shift that does not
        /// repeat (device: up to -0.18 % on 60 V), drawn again at every AD1 enable. `q` jack volts.
        private double N7(double q, int r) => N7On ? (q + N7Volts[Math.Max(0, Math.Min(3, r))] * N7Scale) * (1 + n7GainNow) : q;
        private bool N7On => EnAd1 && (EnAd2 || EnAd3);
        public double[] N7Volts = { -2.9e-3, 17e-3, 0.17, -1.6 };
        public double N7Scale = 1.0, N7Gain = 0.0, N7ACodes = 0.0;
        private double n7GainNow;

        /// Diode test: constant current source (the stock's 0.765 mA constant) with ~3.3 V compliance.
        public const double DiodeTestCurrent = 0.765e-3, DiodeCompliance = 3.3;

        // ------------------------------------------------------------ DBG:SRC
        /// opendm40 DBG:SRC (src/dbgsrc.rs): an ohm / diode row whose AD1 mux (R24 SAD1FP / FN) or
        /// reference (R28) is not the stock row's: AD1 reads a node directly.
        private bool SrcDirect()
        {
            if(bestRow < 0) return false;
            var b = Rows[bestRow].Bytes;
            return ((b[4] ^ regs[0x24]) & 0xF7) != 0 || b[8] != regs[0x28];
        }

        /// Node volts relative to AGND (the model's AGND = VDDA / 2) for DBG:SRC: the ohm source
        /// OP3 at VREF through Rref into the jack node (FB = OP3 side, RLU / PB0 / OP1O = jack
        /// side; the diode row: the current source, PB0 / OP1O = jack / 10 by SDIO).
        private double SrcNode(string n)
        {
            double vs = 0, rref = 0, vx = 0, dummy;
            if(role == Role.Ohm)
            {
                OhmSource(out vs, out rref);
                Fe.OhmNode(vs, rref, out vx, out dummy);
            }
            else vx = Fe.DiodeNode(DiodeTestCurrent, DiodeCompliance);
            switch(n)
            {
            case "FB": return role == Role.Ohm ? vs : 0;
            case "RLU": return vx;
            case "PB0": case "OP1O": return role == Role.Diode ? vx / 10 : vx;
            case "VREF": return OhmSourceVolts();
            case "VDDA": return Vdda * 0.5;
            case "VSSA": return -Vdda * 0.5;
            case "REFO": case "PB6": return Vref;
            default: return 0; // AGND, PB3 (COM), floating nodes
            }
        }

        private double SrcCode(double ig, double rg, double fs)
        {
            var fp = Sad1fpNames[regs[0x24] >> 4]; var fnn = Sad1fnNames[regs[0x24] & 7];
            var rh = Sad1rhNames[(regs[0x28] >> 4) & 7]; var rl = Sad1rlNames[regs[0x28] & 7];
            var vr = SrcNode(rh) - SrcNode(rl);
            if(Math.Abs(vr) < 1e-6) return fs;
            return ig * (SrcNode(fp) - SrcNode(fnn)) / (rg * vr) * fs;
        }

        /// The ohm source of the current row: OP3 at VREF (SFUVR) through the reference resistor
        /// (stock Rref table, scaled by the calibration gain); 0 / 0 outside the ohm rows. The
        /// calibration offset becomes lead resistance.
        private void OhmSource(out double vs, out double rref)
        {
            vs = 0; rref = 0;
            if(role == Role.Ohm)
            {
                vs = OhmSourceVolts();
                // r0 (60 ohm, never selected by the stock): its slot is not a calibration; the
                // firmware (dmm.rs ohm_row_gain) takes r1's gain x the DC mV x10 / x1 gain ratio
                // (0x24 / 0x28), the OP1 x10 stage's error. The stock row's PA2 DS2 / FS2 (the
                // device read 3.8 % lower with them closed) is not modelled.
                rref = roleRange == 0 ? OhmRref[0] * 1000.0 * CalGain(0x128) * CalGain(0x24) / CalGain(0x28)
                    : OhmRref[roleRange] * 1000.0 * CalGain(0x124 + 4 * roleRange);
                Fe.LeadOhm = CalOffs(0x144) * 1000.0;
            }
            else if(role == Role.Cont)
            {
                vs = OhmSourceVolts();
                rref = 1000.0 * CalGain(0x15c);
                Fe.LeadOhm = CalOffs(0x160);
            }
            else Fe.LeadOhm = 0;
        }

        /// Noise of the peak hold's spread samples (own generator: the main sequence stays as it was).
        private double PkGauss()
        {
            var u1 = 1.0 - pkRng.NextDouble();
            var u2 = pkRng.NextDouble();
            return Math.Sqrt(-2 * Math.Log(u1)) * Math.Cos(2 * Math.PI * u2);
        }

        private double Gauss()
        {
            var u1 = 1.0 - rng.NextDouble();
            var u2 = rng.NextDouble();
            return Math.Sqrt(-2 * Math.Log(u1)) * Math.Cos(2 * Math.PI * u2);
        }

        // ------------------------------------------------------------ capacitance
        private double CapOscR()
        {
            return CapK[roleRange] * 1000.0; // kohm -> ohm
        }

        private void UpdateRamp(double now)
        {
            var dt = now - rampLast;
            rampLast = now;
            if(dt <= 0) return;
            if(role == Role.CapDischarge)
            {
                rampV *= Math.Exp(-dt / 0.02);
                rampUp = true;
                rampHt.Clear(); rampHv.Clear();
                return;
            }
            if(role != Role.CapRamp) return;
            if(CapVAvg > 0 && rampHt.Count == 0) RampMark(rampLast - dt);
            var vh = CmpHighThreshold; var vl = CmpLowThreshold;
            if(vh < vl) { var x = vh; vh = vl; vl = x; }
            if(CapCmpW != 1.0) { var m = 0.5 * (vh + vl); var h = 0.5 * (vh - vl) * CapCmpW; vh = m + h; vl = m - h; }
            var cf = Fe.CapRampC(CapK[roleRange] * 1e-6, vh);
            if(double.IsNaN(cf))
            {
                // the network cannot ramp (a leak, a source): the node sits at the leak level
                rampV = double.IsInfinity(Fe.R) ? 0 : Math.Min(vh * 0.9, CapK[roleRange] * 1e-6 * Fe.R);
                return;
            }
            var c = cf * 1e6; // uF
            if(c <= 0) c = 1e-4;   // open: ramps instantly to the threshold
            var slope = CalGain(0x170 + 4 * roleRange) * CapK[roleRange] / c; // V/s at the AD1 node
            // Dielectric absorption (opt-in "capda:k;capdatau:s", CHOSEN, not measured): after
            // the discharge the slope is (1 - k e^(-t / tau)) of I / C, t from the ramp's start
            // (device 832f89a4, ~1 mF electrolytic: the early slope read the capacitor 50 % high
            // and more on 600 uF and 60 mF).
            if(CapDaK > 0 && CapDaTau > 0)
                slope *= Math.Max(0.05, 1 - CapDaK * Math.Exp(-(now - capDaT0) / CapDaTau));
            var slope0 = slope;
            var remaining = dt;
            while(remaining > 0)
            {
                // Up / down asymmetry (opt-in "capasym:a", CHOSEN, not measured): the up ramp
                // (1 + a), the down ramp (1 - a) of the slope (device 40c25e2b, ~1 mF: the
                // results alternated 1227 / 974 / 1044 / 982 uF).
                slope = slope0 * (rampUp ? 1 + CapAsym : 1 - CapAsym);
                var target = rampUp ? vh : vl;
                var dist = Math.Abs(target - rampV);
                var tt = dist / slope;
                if(tt >= remaining)
                {
                    rampV += (rampUp ? 1 : -1) * slope * remaining;
                    break;
                }
                rampV = target;
                if(CapVAvg > 0) RampMark(now - remaining + tt);
                rampUp = !rampUp;
                acpo = !rampUp; // high after reaching the high threshold
                remaining -= tt;
            }
            if(CapVAvg > 0) RampMark(now);
        }

        // ------------------------------------------------------------ comparator
        private bool cmpHigh, cmpLow;

        private void UpdateComparatorStatic(double now)
        {
            if(Fe.PaExits != paExitsSeen)
            {
                paExitsSeen = Fe.PaExits;
                // (opendm40 quiesces the chip before the relay pulse: its writes pass through
                // no-role states, the last real role counts)
                if(role == Role.Ohm || (role == Role.None && lastRole == Role.Ohm)) ohmExitT = now;
            }
            if(!EnCmp) { acpo = false; cmpHigh = cmpLow = false; return; }
            // Sentinel nodes (opendm40 sentinel.rs): PB1 = the A-jack shunt node in every role that
            // leaves the comparator idle, SENSE with the /1000 tap (SS5) in the DC A rows.
            // CMPHO / CMPLO follow the instantaneous node against VRH / VRL.
            double node;
            var scmpiPb1 = (regs[0x20] >> 5) == 4;
            if(SentinelNode(now, out node))
            {
                cmpHigh = node > CmpHighThreshold;
                cmpLow = node < CmpLowThreshold;
                if(role == Role.Ohm && OhmBothV > 0 && scmpiPb1)
                {
                    // a source on V-COM overdrives the comparator (OhmBothV, device 1a11eda)
                    double dc, ac, hz, duty; string shape;
                    Fe.VJack(Fe.RelayPa, out dc, out ac, out hz, out shape, out duty);
                    if(Fe.RelayPa == 1 && Math.Abs(Wave(now, dc, ac, hz, shape, duty)) >= OhmBothV) cmpHigh = cmpLow = true;
                }
                var dx = now - ohmExitT;
                if(role == Role.DcA && dx >= 0 && dx < OhmExitBoth) cmpHigh = cmpLow = true;
                else if(role == Role.DcA && dx >= 0 && dx < OhmExitBoth + OhmExitLow) cmpLow = true;
                return;
            }
            cmpHigh = cmpLow = false;
            switch(role)
            {
            case Role.Cont:
            {
                // OP1O (the drop across Rx) against the AGNDN<2> window (-41.7 mV): CMPO high
                // -> beeper while the drop is inside the window
                double vs, rref, vx, vref;
                OhmSource(out vs, out rref);
                Fe.OhmNode(vs, rref, out vx, out vref);
                acpo = Fe.RelayPa == 1 && Math.Abs(vx) < Math.Abs(CmpLowThreshold);
                break;
            }
            case Role.Diode:
                acpo = true; // R21 = 0xFF: always high; the stock gates the pin with ENPCMPO
                break;
            case Role.CapRamp: case Role.CapOsc:
                break; // driven by the ramp / oscillator
            default:
                acpo = false;
                break;
            }
        }

        /// OhmTauMs: move the lagged node from ohmLagT to t toward the target in force since then.
        private void OhmLagAdvance(double t)
        {
            if(t <= ohmLagT) return;
            ohmLagV = ohmLagTarget + (ohmLagV - ohmLagTarget) * Math.Exp(-(t - ohmLagT) / (OhmTauMs * 1e-3));
            ohmLagT = t;
        }

        /// OhmTauMs: the node's DC value in the row now (0 V outside the ohm rows).
        private double OhmLagTargetNow()
        {
            if(role != Role.Ohm) return 0;
            double vs, rref, vx, vref;
            OhmSource(out vs, out rref);
            Fe.OhmNode(vs, rref, out vx, out vref);
            return vx;
        }

        /// VREF of OP3 relative to AGND (signed; SFUVR, SMODE<3>, CMPO): the open-circuit test voltage.
        private double OhmSourceVolts()
        {
            var smode3 = (regs[0x2F] & 0x08) != 0;
            var name = smode3 ? SfuvrSmode3[Sfuvr, acpo ? 1 : 0] : SfuvrNames[Sfuvr];
            return RefNode(name);
        }

        /// Voltage of a reference-divider node relative to AGND.
        public double RefNode(string name)
        {
            var agnd = Vdda * 0.5; // SAGND 00
            if(name.StartsWith("VDSC") || name.StartsWith("VDS"))
            {
                var n = int.Parse(name.Substring(name.StartsWith("VDSC") ? 4 : 3));
                return K1[n] * Vdda - agnd;
            }
            if(name.StartsWith("AGNDP")) return K2[int.Parse(name.Substring(5))] * Vref;
            if(name.StartsWith("AGNDN")) return -K2[int.Parse(name.Substring(5))] * Vref;
            if(name == "PB6") return Vref;
            if(name == "PB7") return 0;
            return 0;
        }

        // ------------------------------------------------------------ sentinel nodes
        /// The comparator input when the firmware points it at a sentinel node: SCMPI = PB1 (the
        /// A-jack shunt drop) in DC V / DC mV / ohm / temperature / None roles, SCMPI = SENSE in
        /// the DC A roles with SS5 closed (V jack through the /1000 divider tap).
        private bool SentinelNode(double t, out double v)
        {
            v = 0;
            var scmpi = regs[0x20] >> 5;
            switch(role)
            {
            case Role.DcV: case Role.DcMv: case Role.Ohm: case Role.Temp: case Role.None:
                if(role == Role.DcV && roleRange == 0 && scmpi == 0)
                {
                    // opendm40 AC? hint (DC V 6 V): the comparator on SENSE, the V jack / 10
                    v = VTapVolts(t, AcvK[0]);
                    return true;
                }
                if(scmpi != 4) return false;
                v = Pb1Volts(t);
                if(Pb1VjackMv != 0 && (role == Role.Ohm || role == Role.Temp || role == Role.None))
                {
                    double dc, ac, hz, duty; string shape;
                    Fe.VJack(Fe.RelayPa, out dc, out ac, out hz, out shape, out duty);
                    v += Pb1VjackMv * 1e-3 * Wave(t, dc, ac, hz, shape, duty);
                }
                return true;
            case Role.DcA:
                if(scmpi != 0) return false;
                v = VjBlind && !vjSee ? 0 : SenseTapVolts(t);
                return true;
            }
            return false;
        }

        /// PB1: drop across the relay-selected shunt (47.5 ohm while no current mode closed a
        /// MOSFET), instantaneous.
        private double Pb1Volts(double t)
        {
            double dc, ac, hz, duty; string shape;
            Fe.ACurrent(out dc, out ac, out hz, out shape, out duty);
            var v = Wave(t, dc, ac, hz, shape, duty) * Fe.ShuntR + SwTr(t);
            if(Pb1VjackMv != 0 && role == Role.Cont)
            {
                // the V jack on PB1 in the continuity row as in ohm (SentinelNode; device 40728b8 /
                // d2aff22: the AD2 peak on PB1 read -99 100 / +124 800 at 5 V DC, +-400 at 1 V)
                Fe.VJack(Fe.RelayPa, out dc, out ac, out hz, out shape, out duty);
                v += Pb1VjackMv * 1e-3 * Wave(t, dc, ac, hz, shape, duty);
            }
            if(ContBumpMv != 0 && (role == Role.Cont || role == Role.Diode) && t >= contBumpT0 && t - contBumpT0 < 10 * ContBumpTau * 1e-3)
            {
                // the contact transient (ContBumpMv)
                var dt = t - contBumpT0;
                v += ContBumpMv * 1e-3 * Math.Exp(-dt / (ContBumpTau * 1e-3)) * Math.Cos(2 * Math.PI * 25 * dt);
            }
            if(MvPb1Mv != 0 && MvClampV > 0 && role == Role.AcMv)
            {
                // the mV clamp's current (MvPb1Mv): the jack beyond the clamp, scaled
                Fe.VJack(1, out dc, out ac, out hz, out shape, out duty);
                var j = Wave(t, dc, ac, hz, shape, duty);
                v += MvPb1Mv * 1e-3 * (j - Math.Max(-MvClampV, Math.Min(MvClampV, j)));
            }
            // the floating mV input's PB1 pickup after a source went off (Fe.MvHoldPb1Mv)
            if(Fe.MvHeld && Fe.MvHoldPb1Mv != 0 && (role == Role.AcMv || role == Role.DcMv)) v += Fe.MvHoldPb1Mv * 1e-3;
            return v;
        }

        /// The mV clamp (MvClampV) on a DC + AC waveform: the clipped waveform's mean and AC RMS
        /// (one period, 512 points). A square stays a square (exact); other shapes keep their shape
        /// at the reduced RMS in the instantaneous samples (Ad2Sample, an approximation).
        private void MvClamp(double dc, double ac, double hz, string shape, double duty, out double mdc, out double mac)
        {
            const int n = 512;
            double s = 0, s2 = 0;
            var f = hz > 0 ? hz : 50;
            for(var k = 0; k < n; k++)
            {
                var v = Math.Max(-MvClampV, Math.Min(MvClampV, Wave((k + 0.5) / (n * f), dc, ac, f, shape, duty)));
                s += v; s2 += v * v;
            }
            mdc = s / n;
            mac = Math.Sqrt(Math.Max(0, s2 / n - mdc * mdc));
        }

        /// PB2: drop across the 10 mohm element, in series with the A jack in every shunt state
        /// (the uA chain is 10 mohm + 0.5 ohm + 47.5 ohm, INFERRED), instantaneous.
        private double Pb2Volts(double t)
        {
            double dc, ac, hz, duty; string shape;
            Fe.ACurrent(out dc, out ac, out hz, out shape, out duty);
            return Wave(t, dc, ac, hz, shape, duty) * Dm40Frontend.ShuntOhm[0];
        }

        /// SENSE in the V rows: the V jack (path 0) through a divider of ratio k, instantaneous.
        private double VTapVolts(double t, double k)
        {
            double dc, ac, hz, duty; string shape;
            Fe.VJack(0, out dc, out ac, out hz, out shape, out duty);
            return Wave(t, dc, ac, hz, shape, duty) / k + SwTr(t);
        }

        /// SENSE in the A rows: the /1000 divider tap through SS5 (PA5), else open (0).
        private double SenseTapVolts(double t)
        {
            if((regs[0x2C] & 0x10) == 0) return 0;
            double dc, ac, hz, duty; string shape;
            Fe.VJack(0, out dc, out ac, out hz, out shape, out duty);
            return Wave(t, dc, ac, hz, shape, duty) / 1000.0 + SwTr(t);
        }

        /// Instantaneous value of a DC + AC (RMS) waveform at time t.
        private static double Wave(double t, double dc, double ac, double hz, string shape, double duty)
        {
            if(ac <= 0 || hz <= 0) return dc;
            var ph = (t * hz) % 1.0;
            var pk = Peak(ac, shape);
            switch(shape)
            {
            case "square": return dc + (ph < duty ? pk : -pk);
            case "triangle": return dc + pk * (ph < 0.5 ? 4 * ph - 1 : 3 - 4 * ph);
            default: return dc + pk * Math.Sin(2 * Math.PI * ph);
            }
        }

        // ------------------------------------------------------------ power role (opendm40 power meter)
        // firmware/docs/power-meter-and-misconnect.md 3.1 "assignment B": X = AD2 = OP1O - PB3 (the A-jack
        // shunt through OP1, as the stock AC A rows), Y = AD3 = OP2O - AGND (OP2 x1 on SENSE = the V
        // divider tap of the row's switch bytes). Both DC coupled. The RMS block: SDO23 -> sum(Yac^2),
        // else SVXI -> sum(Xac*Yac) (signed), else sum(Xac^2); the LPF: SDO23 -> DC of Y, else of X.
        public const int PowerMode = 16;
        public const double Ad2Clk = 2457600.0;   // SPHACAL step: one AD2CLK period

        /// The 16 distinct opendm40 power rows (iu 1 and 2 share the row; PE decides), the Python
        /// builder of the design: I row (AC A f4 / f5_6) + the V range's switch bytes R2A..R30.
        public static byte[] PowerRow(int vr, int iu, int ir) => PowerRow(Rows, vr, iu, ir);

        private static byte[] PowerRow(RowDef[] rows, int vr, int iu, int ir)
        {
            var b = (byte[])Array.Find(rows, x => x.Role == Role.AcA && x.Mode == (iu == 0 ? 4 : 5) && x.Range == ir).Bytes.Clone();
            var v = vr == 0 ? Array.Find(rows, x => x.Role == Role.DcV && x.Range == 0).Bytes : Array.Find(rows, x => x.Role == Role.AcV && x.Range == vr).Bytes;
            b[0] = 0x12; b[5] &= 0xFD; b[9] = 0xF8;
            Array.Copy(v, 10, b, 10, 7);
            b[18] = (byte)(0x80 | (b[18] & 0x0F));
            b[20] = 0x88; b[21] = 0x54;
            return b;
        }

        private static RowDef[] WithPowerRows(RowDef[] stock)
        {
            var all = new List<RowDef>(stock);
            for(var vr = 0; vr < 4; vr++)
                for(var iu = 0; iu < 2; iu++)
                    for(var ir = 0; ir < 2; ir++)
                        all.Add(new RowDef(Role.Power, PowerMode, vr * 6 + iu * 2 + ir, PowerRow(stock, vr, iu, ir)));
            return all.ToArray();
        }

        private int PowVr => roleRange / 6;
        /// The mA and uA rows are the same bytes: the shunt relays (PE) tell them apart.
        private int PowIu => (roleRange % 6) / 2 == 0 ? 0 : Fe.ShuntSel == 2 ? 2 : 1;
        private int PowIr => roleRange % 2;
        public double Ad3Ig => new[] { 0.5, 1.0, 1.5, 2.0 }[(regs[0x35] >> 2) & 3];
        public double Ad3Rg => (regs[0x34] & 0x10) != 0 ? 1.0 / 3 : 1.0;
        /// The RMS / LPF statistic R34 selects: 0 = X^2, 1 = X*Y, 2 = Y^2.
        private static int PowerStat(byte r34) => (r34 & 0x04) != 0 ? 2 : (r34 & 0x08) != 0 ? 1 : 0;

        /// X and Y in codes (1.25 V / 2^18 at the converter, times IG): DC and AC RMS, and the
        /// waveform of each AC part. The firmware applies the stock AC A gains to X (RMS gain on the
        /// AC part, LPF gain on the DC part - the RMS gain while the LPF gain is the default 1.0 -,
        /// no offsets); Y has the factory-derived tap gain (PowerTapGain) and "powvg:".
        private void PowerNodes(out double xdc, out double xac, out double ydc, out double yac)
        {
            double hz, duty; string shape;
            Fe.ACurrent(out xdc, out xac, out hz, out shape, out duty);
            iHz = hz; iShape = shape; iDuty = duty; iPh = Fe.ISrc != null ? Fe.ISrc.Ph : 0;
            var ci = PowIu * 2 + PowIr;
            var xs = Fe.ShuntR * ((regs[0x25] & 1) != 0 ? 10 : 1) * Ad2Scale();
            xac = xac / CalGain(0xc4 + 4 * ci) * xs;
            // one path (shunt, OP1, AD2) for both parts: the DC part carries the LPF gain when the
            // record has one, else (1.0, never calibrated) the RMS gain as the AC part (opendm40 pw_take)
            var gl = CalGain(0xf4 + 4 * ci);
            xdc = xdc / (gl == 1f ? CalGain(0xc4 + 4 * ci) : gl) * xs;
            Fe.VJack(0, out ydc, out yac, out hz, out shape, out duty);
            vHz = hz; vShape = shape; vDuty = duty;
            var ys = EnAd3 ? ((regs[0x25] & 2) != 0 ? 10 : 1) / AcvK[PowVr] * Ad3Ig / (Ad3Rg * Vref) * 262144.0 : 0;
            // The divider tap's true ratio (doc 3.11 T1): what the factory calibration says of the
            // same taps (opendm40 pw::v_gain_factory), times the "powvg:" deviation.
            ys *= PowVGain / PowerTapGain();
            ydc *= ys; yac *= ys;
            if(PowZeros)
            {
                // The LPF zeros of the device (OP / converter offsets): Y ~1080 codes on every V
                // range, X 888 with OP1 x10 and 155 with x1 (doc 3.11 T1 / T3).
                if(EnAd3) ydc += 1080;
                xdc += (regs[0x25] & 1) != 0 ? 888 : 155;
            }
        }

        /// opendm40 pw::v_gain_factory: 60 / 600 / 1000 V = the AC V r1..r3 gains, 6 V = DC V r0 x
        /// (AC V r1 / DC V r1); 1 without the calibration deviations.
        private double PowerTapGain()
        {
            if(!CalConsistent) return 1;
            var g = PowVr == 0 ? CalGain(0x04) * CalGain(0x38) / CalGain(0x08) : CalGain(0x34 + 4 * PowVr);
            return g >= 0.9 && g <= 1.1 ? g : 1;
        }
        private double iHz, iDuty, iPh, vHz, vDuty;
        private string iShape = "sine", vShape = "sine";

        /// mean(u_V(t) * u_I(t)) of the unit-RMS AC waveforms, the current lagging by phDeg; 0 when
        /// the frequencies differ (orthogonal over the block).
        private double PowerCorr(double phDeg)
        {
            if(vHz <= 0 || Math.Abs(iHz - vHz) > 1e-9 * vHz) return 0;
            const int K = 720;
            var lag = phDeg / 360.0;
            double s = 0;
            for(var k = 0; k < K; k++)
            {
                var c = (k + 0.5) / K + 4;
                s += Wave(c, 0, 1, 1, vShape, vDuty) * Wave(c - lag, 0, 1, 1, iShape, iDuty);
            }
            return s / K;
        }

        /// Instantaneous AD2 data register: X, or Y when SDO23 (registers.md R03..R05).
        private double PowerSample(double t)
        {
            double xdc, xac, ydc, yac;
            PowerNodes(out xdc, out xac, out ydc, out yac);
            if(Sdo23) return Wave(t, ydc, yac, vHz, vShape, vDuty);
            return Wave(t - iPh / 360.0 / Math.Max(iHz, 1e-9), xdc, xac, iHz, iShape, iDuty);
        }

        /// One LPF / RMS block ending at t (length p, index n): the selected statistics, time-weighted
        /// over the R34 selections of the block when it changed mid-block (PowMix).
        private void PowerBlock(double t, double p, long n)
        {
            double xdc, xac, ydc, yac;
            PowerNodes(out xdc, out xac, out ydc, out yac);
            var w = new double[3];
            var cur = PowerStat(regs[0x34]);
            if(mixIdx == n && mixLpfStart == lpfStart)
            {
                for(var i = 0; i < 3; i++) w[i] = mixW[i];
                w[cur] += t - Math.Max(mixSegStart, t - p);
            }
            else w[cur] = 1;
            mixIdx = -1;
            var floor = 30.0 * NoiseScale;
            // SPHACAL: AD2 (I) sampled n AD2CLK periods early (bit 7) or late: only the cross term moves
            var sph = (regs[0x36] & 0x7F) / Ad2Clk * vHz * 360.0 * ((regs[0x36] & 0x80) != 0 ? -1 : 1);
            double rms = 0, lpf = 0, wsum = 0;
            for(var s = 0; s < 3; s++)
            {
                if(w[s] <= 0) continue;
                double v;
                if(s == 1)
                {
                    // uncorrelated noise averages out over the block's samples
                    v = xac * yac * PowerCorr(iPh + sph);
                    if(NoiseScale > 0) v += Gauss() * floor * Math.Sqrt((xac * xac + yac * yac + floor * floor) / (Osr4 * LpfRateDivisor));
                }
                else
                {
                    var a = s == 0 ? xac : yac;
                    var r = Math.Sqrt(a * a + floor * floor);
                    if(a > 0 && NoiseScale > 0) r += Gauss() * NoiseScale;
                    v = r * r;
                }
                rms += w[s] * v; lpf += w[s] * (s == 2 ? ydc : xdc); wsum += w[s];
            }
            if(EnLpf)
            {
                lpfCode = Clamp19((int)Math.Round(lpf / wsum));
                intf |= 0x08;
                if(EnPkh && (regs[0x29] & 3) >= 2) PeakHold(lpfCode);
            }
            if(EnRms)
            {
                // 38-bit signed in RMS<37:0>, the long's two's complement gives bits 39:38 as sign copies
                rmsVal = (long)Math.Max(-262143.0 * 262143.0, Math.Min(262143.0 * 262143.0, rms / wsum));
                intf |= 0x10;
            }
        }

        /// R34 changed the statistic: credit the time since the block (or the last switch) began to
        /// the old selection; the block in progress ends as the time-weighted mix.
        private void PowerStatSwitch(byte old, double now)
        {
            var p = LpfPeriod;
            var bStart = lpfStart + lpfDone * p;
            if(mixIdx != lpfDone + 1 || mixLpfStart != lpfStart)
            {
                Array.Clear(mixW, 0, 3);
                mixIdx = lpfDone + 1; mixLpfStart = lpfStart; mixSegStart = bStart;
            }
            mixW[PowerStat(old)] += Math.Max(0, now - Math.Max(mixSegStart, bStart));
            mixSegStart = now;
        }
        private readonly double[] mixW = new double[3];
        private long mixIdx = -1;
        private double mixLpfStart, mixSegStart;

        // ------------------------------------------------------------ roles
        public enum Role { None, DcV, DcMv, AcV, AcMv, DcA, AcA, Ohm, Cont, Diode, CapOsc, CapRamp, CapDischarge, Freq, Temp, Power }

        private void DeriveRole(double now)
        {
            var best = -1; var bestScore = int.MaxValue;
            // opendm40 data logger, AD2 source (hy3131::set_ad2_stream): an AC row with AD3, RMS and
            // LPF switched off (R34 = 0, R29 = 0) and another AD2 rate in R26: still that row.
            var ad2Stream = EnAd2 && !EnAd3 && regs[0x29] == 0;
            for(var i = 0; i < Rows.Length; i++)
            {
                var row = Rows[i];
                var score = 0;
                for(var j = 0; j < 22; j++)
                {
                    var mask = RowMask[j];
                    // R32: SOP1P distinguishes the A / mA shunt rows (PB2 / PB1), so it is
                    // compared when the row uses OP1; ENOP1 itself is always masked (AC+DC
                    // writes R32 = 0x38 after the DC V rows, whose OP1 is off).
                    if(j == 18) mask = (row.Bytes[18] & 0x08) != 0 ? 0xF7 : 0xF0;
                    // opendm40 sentinel fields (sentinel.rs): in the rows that leave AD1 off (the AC
                    // rows) its input / gain / reference fields are free, and so is OP1CHOP (the
                    // AC V sentinel row holds OP1's chopper off); PKHSEL / ENPKH are free
                    // everywhere; SS5 (R2C bit 4) is free in the current rows.
                    if((row.Bytes[3] & 0x80) == 0)
                    {
                        if(j == 3 || j == 4 || j == 8) mask = 0;
                        if(j == 5) mask = 0xCF;
                        if(j == 19) mask = 0x3C;
                    }
                    if(j == 9) mask = 0xF8;
                    if(j == 12 && (row.Role == Role.DcA || row.Role == Role.AcA)) mask = 0xEF;
                    // opendm40 sentinel fields, later ones: the frequency row's AD1 (a zero reading
                    // nobody reads) may take another input / gain; continuity / diode / capacitance
                    // leave AD2 and OP2 unused, so AD2IG, the AD2 inputs and OP2 are free there.
                    if(row.Role == Role.Freq && j == 4) mask = 0;
                    if(row.Role == Role.Freq && j == 5) mask = 0xCF;
                    if(row.Role == Role.Cont || row.Role == Role.Diode || row.Role == Role.CapOsc)
                    {
                        if(j == 5) mask &= 0x3D;
                        if(j == 7) mask = 0;
                        if(j == 18) mask &= 0x0F;
                    }
                    // opendm40 A-jack protection probe (sentinel.rs probe_row): AD2 / OP2 are unused in
                    // the comparator rows (DC V, DC mV, ohm, temperature): AD2IG, R26, R27, OP2 free.
                    if(row.Role == Role.DcV || row.Role == Role.DcMv || row.Role == Role.Ohm || row.Role == Role.Temp)
                    {
                        if(j == 5) mask &= 0x3D;
                        if(j == 6 || j == 7) mask = 0;
                        if(j == 18) mask &= 0x0F;
                    }
                    if(ad2Stream && (j == 9 || j == 20)) mask = 0;
                    // opendm40 DBG:SRC (src/dbgsrc.rs): the ohm / diode rows with AD1 cycled through
                    // other nodes: the AD1 reference buffers, SAD1FP / FN (not SDIO) and R28 are free.
                    // opendm40 ohm r0 (dmm.rs OHM_R0_ROW): the stock r0 row with PA2's DS2 / FS2 open.
                    if(row.Role == Role.Ohm && row.Range == 0 && j == 11) mask &= 0xF9;
                    if(row.Role == Role.Ohm || row.Role == Role.Diode)
                    {
                        if(j == 3) mask &= 0xF3;
                        if(j == 4) mask &= 0x08;
                        if(j == 8) mask = 0;
                    }
                    // opendm40 DC-coupled V row (ad2exp.rs dcv_row, feasibility E7): the AC V row with SAD2IP =
                    // OP1O, AD2IN AGND / RLU, OP1+ SENSE or AGND (the zero), AD3 / RMS / LPF off, also with
                    // ENAD2 off (the AD1-alone phase of DBG:AD2:DCV?).
                    if(row.Role == Role.AcV && (regs[0x27] >> 6) == 0)
                    {
                        if(j == 7) mask = 0x0F;
                        if(j == 18) mask &= 0xF0;
                        if(j == 9 || j == 20) mask = 0;
                        if(j == 21) mask = 0x0C; // AD3 inputs (opendm40 AC+DC method C: AD3 on OP1O)
                    }
                    // opendm40 power rows: R34 statistic select (SVXI / SDO23) and R35 AD3IN / AD3IG are
                    // run-time choices; the I row's power-bus bytes R2E / R2F = 08 81 are accepted too
                    if(row.Role == Role.Power)
                    {
                        if(j == 20) mask = 0x73;
                        if(j == 21) mask = 0xC3;
                        if((j == 14 || j == 15) && regs[0x2E] == 0x08 && regs[0x2F] == 0x81) mask = 0;
                    }
                    var d = (row.Bytes[j] ^ regs[0x20 + j]) & mask;
                    if(d != 0) score += BitCount(d) * (j == 0 ? 1 : 2);
                }
                if(score < bestScore) { bestScore = score; best = i; }
            }
            bestRow = best; bestScore_ = bestScore;
            var newRole = Role.None; var newMode = -1; var newRange = 0;
            if(best >= 0 && bestScore <= 2)
            {
                newRole = Rows[best].Role; newMode = Rows[best].Mode; newRange = Rows[best].Range;
                rowIg = new[] { 0.9, 1.8, 2.7, 3.6 }[(Rows[best].Bytes[5] >> 4) & 3];
                rowRg = (Rows[best].Bytes[3] & 0x10) != 0 ? 1.0 / 3 : 1.0;
                rowIg2 = new[] { 0.5, 1.0, 1.5, 2.0 }[(Rows[best].Bytes[5] >> 6) & 3];
            }
            if(newRole == Role.Temp && Sad1I == 1) newRole = Role.None;
            if(newRole != role || newMode != roleMode || newRange != roleRange)
            {
                if(newRole == Role.CapRamp && role != Role.CapRamp) { rampUp = true; if(role == Role.CapDischarge || lastRole == Role.CapDischarge) capDaT0 = now; }
                // (the quiesce writes pass through no-role states: the last real role counts)
                if(newRole == Role.AcA && lastRole != Role.AcA) acaT0 = now;
                if(newRole == Role.AcV && lastRole != Role.AcV) acvT0 = now;
                if(newRole == Role.AcA && lastRole == Role.AcA && lastMode == 4 && newMode >= 5) acaStepT = now;
                if(newRole != Role.None) { lastRole = newRole; lastMode = newMode; }
                if(OhmTauMs > 0) OhmLagAdvance(now); // the old row's target up to this write
                if(newRole == Role.Temp) vjSee = true;
                else if(newRole == Role.Ohm || newRole == Role.Cont || newRole == Role.Diode || newRole == Role.CapOsc
                        || newRole == Role.CapRamp || newRole == Role.CapDischarge || newRole == Role.Freq
                        || (newRole == Role.DcV && newRange >= 1)) vjSee = false;
                role = newRole; roleMode = newMode; roleRange = newRange;
                swT0 = now;
                if(OhmTauMs > 0) ohmLagTarget = OhmLagTargetNow();
            }
            // the AD2/AD3 enable-order phase: bad when ENAD2 was set before ENAD3 (empirical)
            beatBad = EnAd2 && EnAd3 && ad2Start <= ad3Start;
            UpdateComparatorStatic(now);
        }

        private static int BitCount(int v) { var c = 0; while(v != 0) { c += v & 1; v >>= 1; } return c; }

        private class RowDef
        {
            public Role Role; public int Mode, Range; public byte[] Bytes;
            public RowDef(Role role, int mode, int range, string hex)
            {
                Role = role; Mode = mode; Range = range;
                var parts = hex.Split(' ');
                Bytes = new byte[22];
                for(var i = 0; i < 22; i++) Bytes[i] = Convert.ToByte(parts[i], 16);
            }
            public RowDef(Role role, int mode, int range, byte[] bytes) { Role = role; Mode = mode; Range = range; Bytes = bytes; }
        }

        // Compare masks per configuration byte R20..R35 (0 = ignored): R20 ignored (the counter and
        // comparator source bits change at run time: Hz mode alternates 0xF2 / 0x0A),
        // R21 / R22 / R36 ignored, R26 only AD2RG (the enable bit: opendm40 sets it after the row; the
        // rate bits: the data logger's AD2 rungs), R34 without the enable bit, R31 without SFUVR (ohm variants), R32 without ENOP1 / SOP1P (AC+DC writes 0x38
        // after the DC V row), R33 without SAD1I (temperature phases).
        private static readonly int[] RowMask = {
            0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x10, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xF0, 0xF7, 0xFC, 0x7F, 0xFF };

        // Stock DM40 V2.6 configuration rows (stock/re/analysis/hy3131_v2_6.h) with their role.
        private static readonly RowDef[] Rows = WithPowerRows(new[] {
            new RowDef(Role.AcV, 0, 0, "f2 dd 16 03 33 43 80 9d 31 fc 20 00 00 09 28 a0 80 c7 38 ac 88 94"),
            new RowDef(Role.AcV, 0, 1, "f2 dd 16 03 33 40 80 9d 31 fc 20 00 00 09 28 a0 80 c7 38 ac 88 94"),
            new RowDef(Role.AcV, 0, 2, "f2 dd 16 03 33 40 80 9d 31 fc 20 00 90 00 28 a0 80 c7 38 ac 88 94"),
            new RowDef(Role.AcV, 0, 3, "f2 dd 16 03 33 40 80 9d 31 fc 20 00 99 00 28 a0 80 c7 38 ac 88 94"),
            new RowDef(Role.DcV, 1, 0, "f0 dd 15 83 31 40 00 55 31 00 06 00 00 90 28 a0 80 d7 38 ac 00 54"),
            new RowDef(Role.DcV, 1, 1, "f0 dd 15 83 07 40 00 55 31 00 06 00 00 09 68 a4 80 c7 33 ac 00 54"),
            new RowDef(Role.DcV, 1, 2, "f0 dd 15 83 07 40 00 55 31 00 06 00 90 00 68 a4 80 c7 33 ac 00 54"),
            new RowDef(Role.DcV, 1, 3, "f0 dd 15 83 07 40 00 55 31 00 06 00 99 00 68 a4 80 c7 33 ac 00 54"),
            new RowDef(Role.AcMv, 2, 0, "f2 dd 16 03 33 43 80 5d 31 f8 00 00 00 00 08 91 80 c7 aa ac 88 54"),
            new RowDef(Role.AcMv, 2, 1, "f2 dd 16 03 33 40 80 5d 31 f8 00 00 00 00 08 91 80 c7 aa ac 88 54"),
            new RowDef(Role.DcMv, 3, 0, "f0 dd 15 83 33 01 00 55 31 00 00 00 00 00 08 91 80 d7 3a ac 00 50"),
            new RowDef(Role.DcMv, 3, 1, "f0 dd 15 83 33 00 00 55 31 00 00 00 00 00 08 91 80 d7 3a ac 00 50"),
            new RowDef(Role.AcA, 4, 0, "f2 dd 15 03 52 43 80 2d 31 f4 00 00 00 00 08 81 80 c7 3e ac 88 24"),
            new RowDef(Role.AcA, 4, 1, "f2 dd 15 03 52 40 80 2d 31 f4 00 00 00 00 08 81 80 c7 3e ac 88 24"),
            new RowDef(Role.AcA, 5, 0, "f2 dd 14 03 35 43 80 2d 31 fc 00 00 00 00 08 81 80 c7 3d ac 88 24"),
            new RowDef(Role.AcA, 5, 1, "f2 dd 14 03 35 40 80 2d 31 fc 00 00 00 00 08 81 80 c7 3d ac 88 24"),
            new RowDef(Role.DcA, 7, 0, "f0 dd 15 80 35 01 00 55 31 00 00 00 00 00 08 81 80 c7 3e ac 00 50"),
            new RowDef(Role.DcA, 7, 1, "f0 dd 15 83 35 00 00 55 31 00 00 00 00 00 08 81 80 c7 3e ac 00 50"),
            new RowDef(Role.DcA, 8, 0, "f0 dd 15 83 35 01 00 55 31 00 00 00 00 00 08 81 80 c7 3d a4 00 50"),
            new RowDef(Role.DcA, 8, 1, "f0 dd 15 83 35 00 00 55 31 00 00 00 00 00 08 81 80 c7 3d ac 00 50"),
            new RowDef(Role.Ohm, 10, 0, "e0 dd 14 83 35 01 00 55 00 00 00 46 06 00 00 94 80 d2 3c 68 00 50"),
            new RowDef(Role.Ohm, 10, 1, "e0 dd 14 83 35 00 00 55 00 00 00 40 06 00 00 94 80 d2 3c 68 00 50"),
            new RowDef(Role.Ohm, 10, 2, "e0 dd 14 83 35 00 00 55 00 00 00 40 60 00 00 94 80 d2 3c 68 00 50"),
            new RowDef(Role.Ohm, 10, 3, "e0 dd 14 83 35 00 00 55 00 00 00 40 00 06 00 94 80 d2 3c 68 00 50"),
            new RowDef(Role.Ohm, 10, 4, "e0 dd 14 8f 35 08 00 55 00 00 00 06 00 60 00 94 80 d2 3c 68 00 50"),
            new RowDef(Role.Ohm, 10, 5, "e0 dd 14 8f 35 08 00 55 00 00 00 06 00 00 00 94 80 d2 3c 60 00 50"),
            new RowDef(Role.Ohm, 10, 6, "e0 dd 14 83 35 00 00 55 11 00 00 08 00 00 00 86 80 d1 3c 60 00 50"),
            new RowDef(Role.Cont, 11, 0, "56 ec 13 83 35 08 00 55 44 00 00 40 06 00 01 94 80 cd 38 68 00 50"),
            new RowDef(Role.Diode, 12, 0, "b0 ff 13 83 3d 00 00 55 31 00 80 00 88 00 00 86 80 e2 3c 68 00 50"),
            new RowDef(Role.CapOsc, 13, 0, "72 21 16 03 b5 00 00 55 31 00 00 08 00 80 80 9a 80 c7 3c ac 00 50"),
            new RowDef(Role.CapOsc, 13, 1, "72 21 16 03 b5 00 00 55 31 00 00 08 00 80 80 9a 80 c7 3c ac 00 50"),
            new RowDef(Role.CapOsc, 13, 2, "72 21 16 03 b5 00 00 55 11 00 00 00 00 08 00 9a 80 c7 3c ac 00 50"),
            new RowDef(Role.CapOsc, 13, 3, "72 21 16 03 b5 00 00 55 11 00 00 00 80 08 00 9a 80 c7 3c ac 00 50"),
            new RowDef(Role.CapOsc, 13, 4, "72 22 16 03 b5 00 00 55 11 00 80 00 80 00 00 9a 80 c7 3c ac 00 50"),
            new RowDef(Role.CapRamp, 13, 5, "52 bb 14 83 43 40 00 5d 31 00 00 00 00 88 00 8e 80 c2 cc a8 00 54"),
            new RowDef(Role.CapRamp, 13, 6, "52 bb 14 83 43 40 00 5d 31 00 00 00 80 08 00 8e 80 c2 cc a8 00 54"),
            new RowDef(Role.CapRamp, 13, 7, "52 bb 14 83 43 40 00 5d 31 00 00 00 88 08 00 8e 80 c2 cc a8 00 54"),
            new RowDef(Role.CapDischarge, 13, 8, "52 dd 13 83 43 40 00 5d 31 00 00 00 88 88 00 8e 80 c2 cc a8 00 54"),
            new RowDef(Role.Freq, 14, 0, "f2 dd 15 83 df 00 80 15 11 f8 00 00 00 00 08 91 80 c7 3a 2f 00 50"),
            new RowDef(Role.Temp, 15, 0, "f0 dd 16 83 33 01 00 55 31 00 00 06 00 60 08 95 80 d7 3c af 00 50"),
            new RowDef(Role.Temp, 15, 1, "f0 dd 16 83 33 00 00 55 31 00 00 00 00 06 08 95 80 d7 3c af 00 50"),
        });

        // Write masks of R20..R35 (datasheet "0" bits are held at 0, "-" bits are kept).
        private static readonly byte[] WriteMask = {
            0xFE, 0xFF, 0xFF, 0xDF, 0xFF, 0xFF, 0xBF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xBC, 0xFC,
            0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF };

        // Stock scale constants (dmm_v2_6.h)
        private static readonly double[] AcvK = { 10, 100, 1000, 10000 };
        private static readonly double[] MvDiv = { 10, 1 };
        private static readonly double[] Shunt = { 0.01, 0.0005, 4.75e-5 }; // ohm / kohm / Mohm
        private static readonly double[] AUnit = { 1, 1e3, 1e6 };           // A -> A / mA / uA
        private static readonly double[] OhmRref = { 0.1, 1, 10, 100, 1000, 10000, 63000 };
        private static readonly double[] CapK = { 1020, 1003, 102.7, 10.68, 1.5, 4.06, 41, 400 };
        private static readonly double[] DcvNegK = { 1.0, 1.00008, 1.00011, 1.0015 };
        private static readonly double[] K1 = { 0, 1 / 36.0, 2 / 36.0, 4 / 36.0, 8 / 36.0, 10 / 36.0, 12 / 36.0, 14 / 36.0, 16 / 36.0, 18 / 36.0,
            20 / 36.0, 22 / 36.0, 24 / 36.0, 26 / 36.0, 28 / 36.0, 32 / 36.0, 34 / 36.0, 35 / 36.0 };
        private static readonly double[] K2 = { 0, 2 / 120.0, 4 / 120.0, 6 / 120.0, 10 / 120.0, 20 / 120.0, 40 / 120.0, 60 / 120.0, 80 / 120.0, 100 / 120.0 };
        private static readonly string[] ScmprhNames = { "VDSC16", "VDSC13", "VDSC11", "VDSC10", "VDSC9", "VDSC8", "VDSC7", "PB7",
            "AGNDP6", "AGNDP5", "AGNDP4", "AGNDP3", "AGNDP2", "AGNDP1", "AGNDP0", "AGNDN1" };
        private static readonly string[] ScmprlNames = { "VDSC2", "VDSC5", "VDSC7", "VDSC8", "VDSC9", "VDSC10", "VDSC11", "PB6",
            "AGNDN6", "AGNDN5", "AGNDN4", "AGNDN3", "AGNDN2", "AGNDN1", "AGNDP0", "AGNDP1" };
        private static readonly string[] SfuvrNames = { "VDS17", "VDS16", "VDS15", "AGNDP9", "AGNDP8", "AGNDP7", "AGNDP6", "AGNDP0",
            "VDS1", "VDS2", "VDS3", "AGNDN9", "AGNDN8", "AGNDN7", "AGNDN6", "PB7" };
        private static readonly string[,] SfuvrSmode3 = {
            { "VDS17", "VDS1" }, { "VDS16", "VDS2" }, { "VDS15", "VDS3" }, { "AGNDP9", "AGNDN9" },
            { "AGNDP8", "AGNDN8" }, { "AGNDP7", "AGNDN7" }, { "AGNDP6", "AGNDN6" }, { "AGNDP0", "PB7" },
            { "VDS17", "VDS1" }, { "VDS16", "VDS2" }, { "VDS15", "VDS3" }, { "AGNDP9", "AGNDN9" },
            { "AGNDP8", "AGNDN8" }, { "AGNDP7", "AGNDN7" }, { "AGNDP6", "AGNDN6" }, { "AGNDP0", "PB7" } };

        private readonly byte[] regs = new byte[64];
        private byte intf;
        private bool ctbov, acpo, beatBad;
        private double ad1Start, ad1Last, ad2Start, ad3Start, lpfStart, lastUpdate;
        private long ad1Done, ad2Done, lpfDone;
        private int ad1Code, ad2Code, lpfCode, pkhMin, pkhMax;
        private long rmsVal;
        private bool ctEnabled, ctStartDecided;
        private double ctEnableTime, ctStart, ctStop, ctPhase;
        private int ctInit, ctA, ctB, ctC;
        private double rampV, rampLast;
        /// Dielectric absorption of the capacitor on the ramp rows ("capda:k;capdatau:s").
        public double CapDaK, CapDaTau = 1.0, CapAsym;
        /// The ramp comparator's window (high - low threshold) scaled by this around its middle
        /// ("capcmpw:k", CHOSEN, not measured): device 19b6e3b7, ~1 mF on 6 mF after a charge, the
        /// class-1 ramp turned between -20 and -36 mV (~60 mV wide on a clean start).
        public double CapCmpW = 1.0;
        /// The AD1 ramp reading as the node's mean over the last this many s ("capvavg:s",
        /// CHOSEN, not measured; 0: the node at the conversion's end): device 19b6e3b7 read a part
        /// of a poll's dV in the poll after a comparator turn, with no turn seen in that poll.
        public double CapVAvg;
        private readonly List<double> rampHt = new List<double>(), rampHv = new List<double>();
        private void RampMark(double t)
        {
            var k = rampHt.Count;
            if(k > 0 && t < rampHt[k - 1]) { rampHt.Clear(); rampHv.Clear(); k = 0; }
            // (open leads turn the ramp ~10^5 times a second: at most one point per 0.1 ms)
            if(k > 1 && t - rampHt[k - 2] < 1e-4) { rampHt[k - 1] = t; rampHv[k - 1] = rampV; return; }
            rampHt.Add(t); rampHv.Add(rampV);
            var old = 0;
            while(old < rampHt.Count - 2 && rampHt[old + 1] < t - CapVAvg - 1) old++;
            if(old > 0) { rampHt.RemoveRange(0, old); rampHv.RemoveRange(0, old); }
        }
        /// The node's mean over [t - CapVAvg, t] from the marked breakpoints (piecewise linear).
        private double RampMean(double t)
        {
            var n = rampHt.Count;
            if(n < 2) return rampV;
            Func<double, double> at = x =>
            {
                if(x <= rampHt[0]) return rampHv[0];
                for(var i = 1; i < n; i++)
                    if(x <= rampHt[i]) return rampHv[i - 1] + (rampHv[i] - rampHv[i - 1]) * (x - rampHt[i - 1]) / Math.Max(1e-12, rampHt[i] - rampHt[i - 1]);
                return rampHv[n - 1];
            };
            var a = t - CapVAvg;
            double sum = 0, x0 = a, v0 = at(a);
            for(var i = 0; i < n && rampHt[i] < t; i++)
            {
                if(rampHt[i] <= a) continue;
                sum += 0.5 * (v0 + rampHv[i]) * (rampHt[i] - x0);
                x0 = rampHt[i]; v0 = rampHv[i];
            }
            var vt = at(t);
            sum += 0.5 * (v0 + vt) * (t - x0);
            return sum / CapVAvg;
        }
        private double capDaT0;
        private bool rampUp;
        private double vc, vcLast;
        private Role role;
        private int roleMode, roleRange;
        private double rowIg = 0.9, rowRg = 1.0, rowIg2 = 1.0;
        private int bestRow = -1, bestScore_;
        /// Diagnostics: the closest stock row and the differing configuration bytes.
        public string RoleDebug
        {
            get
            {
                var r = string.Format("best {0} score {1}", bestRow, bestScore_);
                if(bestRow >= 0) for(var j = 0; j < 22; j++) { var d = (Rows[bestRow].Bytes[j] ^ regs[0x20 + j]) & RowMask[j]; if(d != 0) r += string.Format(" R{0:X2}:{1:X2}^{2:X2}", 0x20 + j, Rows[bestRow].Bytes[j], regs[0x20 + j]); }
                return r;
            }
        }
        private Random rng;
        private Random faultRng = new Random(777);
        private Random pkRng = new Random(4242);
        /// The RMS block's HPF state in the DC-coupled V row (codes; HpfTau).
        private double hpfM;
        /// LpfCreep: the lagged jack DC (V, NaN: not yet seen), when it was advanced; LpfDcNoise's generator.
        private double creepV = double.NaN, creepV1, creepV2, creepT;
        private int creepR;
        private Random lpfNoiseRng = new Random(9191);
    }

    /// NIST ITS-90 type K thermocouple EMF (mV) from temperature (C).
    public static class TypeK
    {
        private static readonly double[] CNeg = { 0.0, 0.394501280250e-1, 0.236223735980e-4, -0.328589067840e-6, -0.499048287770e-8,
            -0.675090591730e-10, -0.574103274280e-12, -0.310888728940e-14, -0.104516093650e-16, -0.198892668780e-19, -0.163226974860e-22 };
        private static readonly double[] CPos = { -0.176004136860e-1, 0.389212049750e-1, 0.185587700320e-4, -0.994575928740e-7, 0.318409457190e-9,
            -0.560728448890e-12, 0.560750590590e-15, -0.320207200030e-18, 0.971511471520e-22, -0.121047212750e-25 };

        public static double Emf(double t)
        {
            double e = 0;
            if(t < 0)
            {
                for(var i = CNeg.Length - 1; i >= 0; i--) e = e * t + CNeg[i];
            }
            else
            {
                for(var i = CPos.Length - 1; i >= 0; i--) e = e * t + CPos[i];
                e += 0.118597600000 * Math.Exp(-0.118343200000e-3 * (t - 126.9686) * (t - 126.9686));
            }
            return e;
        }
    }

    // ================================================================= Renode peripheral
    public class HY3131 : IDoubleWordPeripheral, IKnownSize, IGPIOReceiver
    {
        public HY3131(IMachine machine, INumberedGPIOOutput portC, INumberedGPIOOutput portD,
                      int sck = 10, int miso = 11, int mosi = 12, int cs = 2,
                      INumberedGPIOOutput portA = null, INumberedGPIOOutput portE = null)
        {
            this.machine = machine;
            this.portC = portC;
            this.misoPin = miso;
            Core = new Hy3131Core();
            Core.IntfRaceWindow = 0.7e-6;
            Reset();
            // Connect() reports the current pin levels at once (CS is low before the
            // firmware drives it): the state is initialised before the handlers run.
            portC.Connections[sck].Connect(this, 0);
            portC.Connections[mosi].Connect(this, 1);
            portD.Connections[cs].Connect(this, 2);
            // Front-end relays: PA9 / PA10 latching pair, PE0 / PE1 shunt select, PC7 (frequency mode).
            // The ports are looked up by name when the platform description does not pass them.
            if(portA == null) portA = FindPort("sysbus.gpioPortA") ?? FindPort("gpioPortA");
            if(portE == null) portE = FindPort("sysbus.gpioPortE") ?? FindPort("gpioPortE");
            portC.Connections[7].Connect(this, 7);
            if(portA != null) { portA.Connections[9].Connect(this, 3); portA.Connections[10].Connect(this, 4); }
            else this.Log(LogLevel.Warning, "gpioPortA not found: the V/A relay is not tracked (ohm / mV paths always connected)");
            if(portE != null) { portE.Connections[0].Connect(this, 5); portE.Connections[1].Connect(this, 6); portE.Connections[2].Connect(this, 8); }
            else this.Log(LogLevel.Warning, "gpioPortE not found: the shunt relays are not tracked");
            relayTracked = portA != null;
        }

        private INumberedGPIOOutput FindPort(string name)
        {
            try
            {
                IPeripheral p;
                if(machine.TryGetByName(name, out p)) return p as INumberedGPIOOutput;
            }
            catch(Exception) { }
            return null;
        }

        public void Reset()
        {
            Core.Reset(Now);
            selected = false; sck = false; mosi = false;
            rising = 0; falling = 0; shift = 0; cmdDone = false; reg = 0; rd = false; wbyte = 0;
            Transfers = 0; SpiBytes = 0;
            calLoaded = false;
        }

        public long Size => 0x100;
        public uint ReadDoubleWord(long offset) => offset == 0 ? (uint)Core.Ad1Code : 0;
        public void WriteDoubleWord(long offset, uint value) { }

        public Hy3131Core Core { get; private set; }

        // ---- scripting (monitor) ----
        /// Raw AD1 code override (int.MinValue = off).
        public void SetAd1(int raw) { Core.ForcedAd1 = raw; }
        public void ClearAd1() { Core.ForcedAd1 = int.MinValue; }
        public void SetAd2(int raw) { Core.ForcedAd2 = raw; }
        /// DC voltage on the V / COM input (kept for the older scripts).
        public void SetVin(double volts) { Core.ForcedAd1 = int.MinValue; Core.SetInput("vsrc:" + volts.ToString(CultureInfo.InvariantCulture)); }
        /// Input spec, see Hy3131Core.SetInput.
        /// The log line is the harness's input marker (emu/README.md "Input
        /// markers"): the virtual time and opendm40's uptime at the change.
        public void SetInput(string spec)
        {
            Core.ForcedAd1 = int.MinValue;
            // A frequency change restarts the waveform (Hy3131Core.SignalHzChanged).
            Core.Update(Now);
            var f0 = Core.CounterInputHz();
            Core.SetInput(spec);
            var f1 = Core.CounterInputHz();
            if(f0 > 0 && f1 > 0 && Math.Abs(f1 - f0) > 1e-9 * f0) Core.SignalHzChanged(Now, f0);
            var dwt = Antmicro.Renode.Peripherals.Miscellaneous.DM40_DWT.Instance;
            var inv = CultureInfo.InvariantCulture;
            this.Log(LogLevel.Info, "input: {0} at {1} fw {2}", spec, MachineTime.ToString("F4", inv),
                (dwt == null ? -1.0 : dwt.FwUptime).ToString("F4", inv));
        }
        public void SetOption(string name, double value) { Core.SetInput(name + ":" + value.ToString(CultureInfo.InvariantCulture)); }
        public void SetCalConsistent(bool on) { Core.CalConsistent = on; }
        public void SetNoise(double scale) { Core.NoiseScale = scale; }
        public void SetCntPin(bool level) { Core.CntPin = level; }
        /// Brown-out: BORF and chip reset.
        public void BrownOut() { Core.Reset(Now); Core.ForceIntf(0x80); }

        public ulong Transfers { get; private set; }
        public int Range => Core.Range;
        public int Mode => Core.Mode;
        public string Role => Core.RoleName;
        public int Intf => Core.Intf;
        public int Ad1 => Core.Ad1Code;
        public bool CmpPin => Core.CmpPin;
        public string Status => string.Format("{0} ad1={1} lpf={2} rms={3} intf=0x{4:X2} cmp={5} period={6:F4}s",
            Core.RoleName, Core.Ad1Code, Core.LpfCode, Core.RmsValue, Core.Intf, Core.CmpPin, Core.Ad1Period);

        /// The chip's time: the machine's virtual time scaled by the HY3131 crystal's
        /// offset from the MCU's (ClockPpm, + = the chip runs fast). Every conversion
        /// grid, the counter and the input model run on it. Set it before the
        /// firmware starts (a change moves the chip's clock by ppm x elapsed).
        private double Now => (tAt >= 0 ? tAt : MachineTime) * (1.0 + ClockPpm * 1e-6);

        /// The machine's time at this access, the CPU's progress inside its
        /// quantum included (ICPU.SyncTime).
        private double MachineTime
        {
            get
            {
                if(machine.SystemBus.TryGetCurrentCPU(out var cpu)) cpu.SyncTime();
                return machine.ElapsedVirtualTime.TimeElapsed.TotalSeconds;
            }
        }

        /// MCU time (s) of the clock edge being processed by a timed SPI byte
        /// (SpiByteTimed), else -1 (the machine's current time).
        private double tAt = -1;

        /// HY3131 crystal offset vs the MCU, ppm (monitor: hy3131 ClockPpm -63).
        public double ClockPpm { get; set; }

        /// The 76.8 kS/s setting's own rate offset, ppm (Hy3131Core.Ad2Osr32Ppm,
        /// default 0 = the datasheet's 76 800, as the device (opendm40 device check
        /// of c830e09); monitor: hy3131 Ad2Osr32Ppm).
        public double Ad2Osr32Ppm { get => Core.Ad2Osr32Ppm; set => Core.Ad2Osr32Ppm = value; }

        /// INTF read-clears-flag race window, us (Hy3131Core.IntfRaceWindow). Default
        /// 0.7 us: what the device's loss bursts imply (c50425f check: ~11.5 ms per
        /// burst at a 63 ppm tick / chip walk = 0.72 us). 0 = an ideal flag.
        public double IntfRaceUs { get => Core.IntfRaceWindow * 1e6; set => Core.IntfRaceWindow = value * 1e-6; }

        /// AD2 flags lost to the INTF race so far.
        public long Swallowed => Core.Swallowed;

        /// Apply the INTF race window to AD1F too (default off; monitor:
        /// hy3131 IntfRaceAd1 true), and the AD1 flags lost to it.
        public bool IntfRaceAd1 { get => Core.IntfRaceAd1; set => Core.IntfRaceAd1 = value; }
        public long SwallowedAd1 => Core.SwallowedAd1;

        /// AD2 start-up error, codes on the first results after a (re)start
        /// (Hy3131Core.Ad2StartGlitch; default 0; monitor: hy3131 Ad2StartGlitch 3000).
        public int Ad2StartGlitch { get => Core.Ad2StartGlitch; set => Core.Ad2StartGlitch = value; }

        /// PB1 picking up the V jack in the ohm / temperature rows, mV per V
        /// (Hy3131Core.Pb1VjackMv; default 0; monitor: hy3131 Pb1VjackMv 10).
        public int Pb1VjackMv { get => Core.Pb1VjackMv; set => Core.Pb1VjackMv = value; }

        /// INTF reads by phase after the last AD2 result (20 bins of 1/20
        /// period), then the swallowed count; cleared by the read.
        public string IntfPhaseHist()
        {
            var r = string.Join(",", Core.IntfPhase) + " swallowed " + Core.Swallowed + " torn " + TornAd2 + " misoerr " + MisoErrors
                + " frames " + Ad2Frames + " dup " + Ad2Dup + " skip " + Ad2Skip;
            Array.Clear(Core.IntfPhase, 0, Core.IntfPhase.Length);
            this.Log(LogLevel.Info, "INTF phase: {0}", r);
            return r;
        }

        // ---- GPIO bit-bang ----
        public void OnGPIO(int number, bool value)
        {
            // A timed hardware SPI master delivers its bytes lazily: bytes that
            // completed before this pin event (CS, the read-period pulse) reach
            // the chip first, with their own times.
            if(number <= 2 && tAt < 0) SpiCatchUp?.Invoke();
            switch(number)
            {
            case 0: // SCK
                if(value && !sck) OnRising();
                else if(!value && sck) OnFalling();
                sck = value;
                break;
            case 1: // MOSI
                mosi = value;
                break;
            case 3: Pin("PA9", value); break;
            case 4: Pin("PA10", value); break;
            case 5: Pin("PE0", value); break;
            case 6: Pin("PE1", value); break;
            case 7: Pin("PC7", value); break;
            case 8: Rail(value); break;
            case 2: // CS (active low)
                if(!value && !selected)
                {
                    selected = true;
                    rising = 0; falling = 0; shift = 0; cmdDone = false; wbyte = 0;
                    ad2FrameSeq = -1; ad2FrameTorn = false;
                    if(Core == null) return; // pin level reported by Connect() in the constructor
                    deaf = !Powered;
                    if(deaf) { DeafFrames++; return; }
                    Transfers++;
                    LoadCal();
                    Core.Update(Now);
                }
                else if(value && selected)
                {
                    selected = false;
                    DriveMiso(Powered && Core.Irq); // DO = IRQ while CS is high
                    MaybeWriteState();
                }
                break;
            }
        }

        // ---- DMM analog rail (PE2) ----
        /// Model the PE2 supply (default on): with PE2 low the chip is unpowered
        /// (writes ignored, DO low), a rising PE2 is a power-on reset (registers
        /// cleared, BORF) and the chip ignores the bus for PowerUpMs. Until the
        /// firmware first raises PE2 the chip counts as powered (a PE2 the
        /// ROM / an earlier boot stage left high; the unit tests).
        public bool RailModel { get; set; } = true;
        /// Power-up time after PE2 rises, ms (the stock waits 10 ms, FUN_0800b384).
        public double PowerUpMs { get; set; } = 2.0;
        public bool Powered => !RailModel || !railKnown || (railOn && Now >= railReadyAt);
        /// Rail power-ups seen, frames the chip ignored (unpowered / powering up).
        public long RailUps { get; private set; }
        public long DeafFrames { get; private set; }

        private void Rail(bool on)
        {
            if(Core == null) return;
            if(on && !railOn)
            {
                if(railKnown)
                {
                    Core.Reset(Now);
                    Core.ForceIntf(0x80); // BORF
                    railReadyAt = Now + PowerUpMs * 1e-3;
                    RailUps++;
                }
                this.Log(LogLevel.Debug, "PE2 rail on");
            }
            else if(!on && railOn)
            {
                this.Log(LogLevel.Debug, "PE2 rail off");
            }
            if(on) railKnown = true;
            railOn = on;
            if(!Powered) DriveMiso(false);
        }

        private bool railOn, railKnown, deaf;
        private double railReadyAt;

        private void Pin(string name, bool level)
        {
            if(Core == null) return;
            Core.Fe.SetPin(name, level);
            if(name != "PC7" || level) this.Log(LogLevel.Debug, "pin {0} = {1}: {2}", name, level, Core.Fe.PinState());
            MaybeWriteState();
        }

        /// One-line front-end state (relays, HY3131 configuration, what the chip sees), for
        /// scripts and the GTK panel. Also written to StateFile when it changes.
        public string Frontend => Core.FrontendState(StateExtra());

        /// The HY3131 supply (PE2 rail model) as a state token.
        private string StateExtra() => string.Format(" powered={0}", Powered ? 1 : 0);

        /// Write the front-end state line to `path` (atomically, on change, at most ~10 Hz).
        public void StateFile(string path) { statePath = path; stateLast = ""; MaybeWriteState(); }

        private void MaybeWriteState()
        {
            if(statePath == null || Core == null) return;
            var now = Now;
            if(now - stateTime < 0.1 && stateLast.Length > 0) return;
            var line = Core.FrontendState(StateExtra());
            if(line == stateLast) return;
            stateLast = line; stateTime = now;
            try
            {
                var tmp = statePath + ".tmp";
                System.IO.File.WriteAllText(tmp, line + "\n");
                System.IO.File.Move(tmp, statePath, true);
            }
            catch(Exception e)
            {
                this.Log(LogLevel.Warning, "state file: {0}", e.Message);
                statePath = null;
            }
        }

        private string statePath, stateLast = "";
        private double stateTime;
        private bool relayTracked;

        private void OnRising()
        {
            if(!selected || Core == null || deaf) return;
            rising++;
            if(!cmdDone)
            {
                shift = (byte)((shift << 1) | (mosi ? 1 : 0));
                if(rising == 8)
                {
                    cmdDone = true;
                    reg = shift >> 1;
                    rd = (shift & 1) != 0;
                    shift = 0;
                    readIdx = -1;
                    this.Log(LogLevel.Debug, "{0} R{1:X2}", rd ? "read" : "write", reg);
                }
            }
            else if(!rd)
            {
                shift = (byte)((shift << 1) | (mosi ? 1 : 0));
                if(++wbyte == 8)
                {
                    this.Log(LogLevel.Debug, "  R{0:X2} <= 0x{1:X2}", reg, shift);
                    Core.Write(reg, shift, Now);
                    reg = (reg + 1) & 0x3F;
                    shift = 0;
                    wbyte = 0;
                }
            }
        }

        private void OnFalling()
        {
            if(!selected || Core == null) return;
            if(deaf) { DriveMiso(false); return; }
            falling++;
            if(cmdDone && rd && falling >= 9)
            {
                var bit = falling - 9;
                var byteIdx = bit / 8;
                var bitIdx = 7 - (bit % 8);
                if(byteIdx != readIdx)
                {
                    readIdx = byteIdx;
                    var r = (reg + byteIdx) & 0x3F;
                    if(reg == 0x03 && r <= 0x05)
                    {
                        // AD2 data: the chip's state at THIS byte (not latched
                        // at CS low): a frame across a conversion edge reads
                        // bytes of two results (TornAd2). Pessimistic on
                        // purpose: whether the device latches R03..R05 during
                        // a read is untested (feasibility 1.4, test 6.2), and
                        // opendm40 places its reads between edges anyway.
                        Core.Update(Now);
                        var seq = Core.Ad2Conversions;
                        if(byteIdx == 0)
                        {
                            // Which result each R03 frame starts on: the
                            // same as the last frame's (Ad2Dup) or one or
                            // more skipped (Ad2Skip; gaps > 64 = a restart).
                            Ad2Frames++;
                            var d = seq - ad2LastFrameSeq;
                            if(ad2LastFrameSeq >= 0 && d == 0) Ad2Dup++;
                            else if(ad2LastFrameSeq >= 0 && d > 1 && d <= 64) Ad2Skip += d - 1;
                            ad2LastFrameSeq = seq;
                        }
                        if(ad2FrameSeq >= 0 && seq != ad2FrameSeq && !ad2FrameTorn)
                        {
                            ad2FrameTorn = true;
                            TornAd2++;
                        }
                        ad2FrameSeq = seq;
                    }
                    curByte = Core.Read(r, Now);
                    this.Log(LogLevel.Noisy, "  R{0:X2} => 0x{1:X2}", (reg + byteIdx) & 0x3F, curByte);
                }
                DriveMiso(((curByte >> bitIdx) & 1) != 0);
            }
        }

        private void DriveMiso(bool b)
        {
            doPrev = doLevel;
            doLevel = b;
            ((IGPIOReceiver)portC).OnGPIO(misoPin, b);
        }

        /// One byte on a hardware SPI in mode 0 (HY3131SpiPort): per bit, MSB
        /// first, the master samples DO, puts the bit on DI and clocks a rising
        /// then a falling edge. CS is the GPIO as on the bit-banged bus; with CS
        /// high the chip ignores the clock and the master reads the IRQ level.
        public byte SpiByte(byte tx)
        {
            SpiBytes++;
            if(!selected || Core == null) return doLevel ? (byte)0xFF : (byte)0x00;
            if(deaf) return 0x00;
            // DI comes from the SPI here, not from the PC12 GPIO level: keep the
            // GPIO's view of the line (OnGPIO only reports changes of it).
            var gpioMosi = mosi;
            var rx = 0;
            for(var i = 7; i >= 0; i--)
            {
                rx = (rx << 1) | (doLevel ? 1 : 0);
                mosi = ((tx >> i) & 1) != 0;
                OnRising();
                OnFalling();
            }
            mosi = gpioMosi;
            return (byte)rx;
        }

        /// Bytes clocked through SpiByte (hardware SPI back end).
        public ulong SpiBytes { get; private set; }

        /// One byte from a timed hardware SPI master (GD32_SPI), delivered when
        /// it has completed: `t0` = MCU time (s) the byte started shifting,
        /// `bit` = SCK period (s), `ckph` = CTL0 CKPH (CPOL 0 in both). Edge i
        /// (bit 7 - i): rising at t0 + (i + 0.5) bit, falling at t0 + (i + 1)
        /// bit; the chip's state (conversions, INTF, the race window) is taken
        /// at those times.
        ///   CKPH 0 (mode 0): the master samples DO at the rising edge, half a
        ///   period after the chip changed it at the previous falling edge.
        ///   With the DO path delay (TDODL 60 ns + the isolator, DoDelayNs)
        ///   above that half period (/8: 80 ns) a bit that just changed may be
        ///   sampled at its old level (MisoErrRate per such bit): device
        ///   b48048e / 42cbf35, plain /8 reads (feasibility 11.3).
        ///   CKPH 1: the master samples at the falling edge, a full period after
        ///   the change (clean at /8), and changes MOSI at the rising edge,
        ///   where the chip samples DI: the chip sees the previous bit (a
        ///   command or write byte in CKPH 1 is garbage; opendm40 sends them in
        ///   mode 0 and switches only the data phase of a read).
        public byte SpiByteTimed(byte tx, double t0, double bit, bool ckph)
        {
            SpiBytes++;
            TimedBytes++;
            if(!selected || Core == null) return doLevel ? (byte)0xFF : (byte)0x00;
            if(deaf) return 0x00;
            var gpioMosi = mosi;
            var marginal = !ckph && bit > 0 && bit / 2 < DoDelayNs * 1e-9;
            var rx = 0;
            try
            {
                for(var i = 7; i >= 0; i--)
                {
                    var k = 7 - i;
                    var nb = ((tx >> i) & 1) != 0;
                    bool v;
                    if(!ckph)
                    {
                        v = doLevel;
                        if(marginal && doLevel != doPrev && MisoErrRate > 0 && spiRng.NextDouble() < MisoErrRate)
                        {
                            v = doPrev;
                            MisoErrors++;
                        }
                        mosi = nb;
                        tAt = t0 + (k + 0.5) * bit;
                        OnRising();
                    }
                    else
                    {
                        // DI changes at this rising edge: the chip takes the old level.
                        tAt = t0 + (k + 0.5) * bit;
                        OnRising();
                        mosi = nb;
                        v = doLevel;
                    }
                    rx = (rx << 1) | (v ? 1 : 0);
                    tAt = t0 + (k + 1) * bit;
                    OnFalling();
                }
            }
            finally
            {
                tAt = -1;
            }
            mosi = gpioMosi;
            return (byte)rx;
        }

        /// Set by the timed SPI master (GD32_SPI via HY3131SpiPort): deliver
        /// the bytes that completed so far, before a CS / SCK / MOSI pin event.
        public Action SpiCatchUp;

        /// DO path delay, ns (TDODL 60 + the isolator; device: /8 mode 0 reads
        /// are marginal, /16 clean): mode-0 sampling with a half SCK period
        /// below it is marginal (MisoErrRate).
        public double DoDelayNs { get; set; } = 85;
        /// Probability that a marginal mode-0 bit that just changed is sampled
        /// at its old level (per such bit; 0 = off).
        public double MisoErrRate { get; set; } = 1e-3;
        /// Bits so sampled wrong so far.
        public long MisoErrors { get; private set; }
        /// Read frames from R03 whose R03..R05 bytes came from two AD2 results
        /// (a frame across a conversion edge, see OnFalling; the AD2 stream's
        /// data frames; the DMM's long reads from R00 cross edges harmlessly).
        public long TornAd2 { get; private set; }
        /// Bytes through SpiByteTimed.
        public ulong TimedBytes { get; private set; }

        private readonly Random spiRng = new Random(4242);
        private bool doPrev, ad2FrameTorn;
        private long ad2FrameSeq = -1, ad2LastFrameSeq = -1;
        /// R03 read frames, those that read the previous frame's result again,
        /// and results no frame read (gaps up to 64; IntfPhaseHist).
        public long Ad2Frames { get; private set; }
        public long Ad2Dup { get; private set; }
        public long Ad2Skip { get; private set; }

        /// The meter's calibration record in flash (0x080C0000: crc, ~crc, version 1, data;
        /// DMM calibration at +0x168) is loaded once, on the first SPI transfer.
        private void LoadCal()
        {
            if(calLoaded) return;
            if(Now <= 0) return; // not before the machine runs (flash image not loaded yet)
            calLoaded = true;
            try
            {
                var hdr = machine.SystemBus.ReadBytes(CalRecord, 8);
                var version = BitConverter.ToUInt32(hdr, 4);
                if(version != 1)
                {
                    this.Log(LogLevel.Warning, "calibration record version {0}: using nominal gains", version);
                    return;
                }
                var raw = machine.SystemBus.ReadBytes(CalRecord + 0x168, 0x1B8);
                var cal = new float[0x1B8 / 4];
                for(var i = 0; i < cal.Length; i++) cal[i] = BitConverter.ToSingle(raw, 4 * i);
                Core.Cal = cal;
                this.Log(LogLevel.Info, "calibration loaded: model {0}, DC V gain {1:F5}, ohm offs {2}", raw[0], cal[1], cal[0x144 / 4]);
            }
            catch(Exception e)
            {
                this.Log(LogLevel.Warning, "calibration record not readable: {0}", e.Message);
            }
        }

        private const ulong CalRecord = 0x080C0000;

        private readonly IMachine machine;
        private readonly INumberedGPIOOutput portC;
        private readonly int misoPin;
        private bool selected, sck, mosi, cmdDone, rd, calLoaded, doLevel;
        private int rising, falling, reg, wbyte, readIdx;
        private byte shift, curByte;
    }
}

namespace Antmicro.Renode.Peripherals.SPI
{
    // The HY3131 as the slave of a hardware SPI controller (dm40.repl: on spi3 =
    // GD32 SPI2, PC10/11/12 AF6). Only the byte path: chip select, the IRQ level
    // and the bit-banged frames stay with the HY3131 peripheral itself.
    /// A slave of a timed SPI master (GD32_SPI): bytes are delivered when they
    /// have completed, with their start time, SCK period and CKPH; the master
    /// registers its catch-up for the slave's own pin events.
    public interface ITimedSpiPeripheral
    {
        byte TransmitTimed(byte data, double t0, double bit, bool ckph);
        void SetCatchUp(Action catchUp);
    }

    public class HY3131SpiPort : ISPIPeripheral, ITimedSpiPeripheral
    {
        public HY3131SpiPort(Antmicro.Renode.Peripherals.Miscellaneous.HY3131 chip)
        {
            this.chip = chip;
        }

        public byte Transmit(byte data) => chip.SpiByte(data);
        public byte TransmitTimed(byte data, double t0, double bit, bool ckph) => chip.SpiByteTimed(data, t0, bit, ckph);
        public void SetCatchUp(Action catchUp) { chip.SpiCatchUp = catchUp; }
        public void FinishTransmission() { }
        public void Reset() { }

        private readonly Antmicro.Renode.Peripherals.Miscellaneous.HY3131 chip;
    }
}
