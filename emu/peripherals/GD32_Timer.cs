//
// GD32F4 / STM32F4 general and advanced timer (TIM1, TIM2, TIM5, TIM8 on the
// DM40) with the parts the scope chains use and Renode's STM32_Timer lacks:
//   * counter from virtual time (no per-tick events): CNT, PSC, ARR, OPM,
//     UDIS/URS, UG; flags UIF/CCxIF/TIF evaluated lazily on SR reads and at
//     scheduled times when an interrupt or a slave depends on them;
//   * slave mode controller: SMS reset(4) / gated(5) / trigger(6) / external
//     clock(7); TS = ITR0..3 (GPIO inputs 16..19, wired in the .repl per the
//     GD32F4 interconnect table), TI1F_ED(4) and TI1FP1(5) (GPIO input 0 =
//     CH1 pin, CCER polarity, filter ignored); CC1 input capture;
//   * output compare PWM1/PWM2/force on CH1..4 (OCxREF), master mode TRGO:
//     reset / enable (level = counter enable) / update / compare pulse /
//     OCxREF level; TRGO and TRGO2 are the same signal (two fan-out points);
//   * interrupts UIE/CCxIE/TIE on IRQ (general timers) and on the split
//     Update/CaptureCompare/Trigger/Break lines (advanced timers).
// DMA requests are not signalled: the scope feeder asks this model how many
// update events happened (UpdateEventsUntil / TimeOfUpdate), which gives
// sample-exact ADC feeding without 50 MHz events.
//
using System;
using System.Collections.Generic;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.CPU;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Timers
{
    public class GD32_Timer : IDoubleWordPeripheral, IWordPeripheral, IKnownSize, IGPIOReceiver
    {
        public GD32_Timer(IMachine machine, ulong frequency = 200000000, bool advanced = false)
        {
            this.machine = machine;
            this.frequency = frequency;
            this.advanced = advanced;
            Reset();
        }

        public void Reset()
        {
            cr1 = 0; cr2 = 0; smcr = 0; dier = 0; sr = 0; ccmr1 = 0; ccmr2 = 0; ccer = 0;
            psc = 0; arr = 0xFFFF; bdtr = 0;
            Array.Clear(ccr, 0, ccr.Length);
            cen = false; gate = true; counting = false;
            segStart = 0; absAtSegStart = 0; evalAbs = 0;
            segments.Clear();
            updatesBeforeSeg = 0;
            for(var i = 0; i < 4; i++) { ti[i] = false; ocref[i] = false; }
            for(var i = 0; i < 4; i++) itr[i] = false;
            generation++;
            IRQ.Set(false); UpdateInterrupt.Set(false); CaptureCompareInterrupt.Set(false); TriggerInterrupt.Set(false);
            TRGO.Set(false); TRGO2.Set(false);
        }

        public long Size => 0x400;
        public GPIO IRQ { get; } = new GPIO();
        public GPIO UpdateInterrupt { get; } = new GPIO();
        public GPIO CaptureCompareInterrupt { get; } = new GPIO();
        public GPIO TriggerInterrupt { get; } = new GPIO();
        public GPIO BreakInterrupt { get; } = new GPIO();
        public GPIO TRGO { get; } = new GPIO();
        public GPIO TRGO2 { get; } = new GPIO();

        // ---- bus ----
        public ushort ReadWord(long offset) => (ushort)ReadDoubleWord(offset);
        public void WriteWord(long offset, ushort value) => WriteDoubleWord(offset, value);

        public uint ReadDoubleWord(long offset)
        {
            switch(offset)
            {
            case 0x00: return (cr1 & ~1u) | (cen ? 1u : 0u);
            case 0x04: return cr2;
            case 0x08: return smcr;
            case 0x0C: return dier;
            case 0x10: Evaluate(Now()); return sr;
            case 0x14: return 0;
            case 0x18: return ccmr1;
            case 0x1C: return ccmr2;
            case 0x20: return ccer;
            case 0x24: return (uint)Counter(Now());
            case 0x28: return psc;
            case 0x2C: return arr;
            case 0x34: return ccr[0];
            case 0x38: return ccr[1];
            case 0x3C: return ccr[2];
            case 0x40: return ccr[3];
            case 0x44: return bdtr;
            default: return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            var now = Now();
            switch(offset)
            {
            case 0x00:
                {
                    Evaluate(now);
                    cr1 = value & ~1u;
                    SetCounterEnable((value & 1) != 0, now);
                }
                break;
            case 0x04:
                cr2 = value;
                UpdateTrgo(now);
                break;
            case 0x08:
                {
                    Evaluate(now);
                    smcr = value;
                    // Gated mode: the gate follows the trigger input level from now on.
                    var sms = smcr & 7;
                    gate = sms != 5 || TriggerLevel();
                    Recount(now);
                }
                break;
            case 0x0C:
                dier = value;
                UpdateIrq(now);
                Schedule(now);
                break;
            case 0x10: // rc_w0
                Evaluate(now);
                sr &= value;
                UpdateIrq(now);
                break;
            case 0x14: // EGR
                Evaluate(now);
                if((value & 1) != 0) // UG
                {
                    SetCounter(0, now);
                    if((cr1 & 4) == 0) sr |= 1; // URS = 0: UG sets UIF
                    if(((cr2 >> 4) & 7) == 0) Pulse(); // MMS = reset
                }
                if((value & 0x40) != 0) sr |= 0x40; // TG
                for(var i = 0; i < 4; i++) if((value & (2u << i)) != 0) sr |= 2u << i;
                UpdateIrq(now);
                UpdateTrgo(now);
                Schedule(now);
                break;
            case 0x18: ccmr1 = value; UpdateTrgo(now); Schedule(now); break;
            case 0x1C: ccmr2 = value; UpdateTrgo(now); Schedule(now); break;
            case 0x20: ccer = value; UpdateTrgo(now); break;
            case 0x24: Evaluate(now); SetCounter(value & 0xFFFF, now); break;
            case 0x28: Evaluate(now); Recount(now, () => psc = value & 0xFFFF); break;
            case 0x2C: Evaluate(now); Recount(now, () => arr = value & 0xFFFF); break;
            case 0x34: case 0x38: case 0x3C: case 0x40:
                Evaluate(now);
                ccr[(offset - 0x34) / 4] = value & 0xFFFF;
                UpdateTrgo(now);
                Schedule(now);
                break;
            case 0x44: bdtr = value; break;
            }
        }

        // ---- inputs: 0..3 = TI1..TI4 pins, 16..19 = ITR0..3 ----
        public void OnGPIO(int number, bool value)
        {
            var now = Now();
            if(number >= 16 && number < 20)
            {
                var n = number - 16;
                var old = itr[n];
                itr[n] = value;
                if(((smcr >> 4) & 7) == (uint)n)
                {
                    TriggerInput(value, old, now);
                }
            }
            else if(number >= 0 && number < 4)
            {
                var old = ti[number];
                ti[number] = value;
                if(number == 0)
                {
                    var ts = (smcr >> 4) & 7;
                    var rising = value && !old;
                    var falling = !value && old;
                    // CC1 polarity: CC1P (bit 1), CC1NP (bit 3): 00 rising, 10 falling, 11 both
                    var p = (ccer >> 1) & 1;
                    var np = (ccer >> 3) & 1;
                    var active = (p == 0 && np == 0 && rising) || (p == 1 && np == 0 && falling) || (p == 1 && np == 1 && (rising || falling));
                    if(active && ((ccmr1 & 3) == 1))
                    {
                        Evaluate(now);
                        ccr[0] = (uint)Counter(now);
                        sr |= 2; // CC1IF capture
                        UpdateIrq(now);
                    }
                    if(ts == 5 && active)
                    {
                        TriggerInput(value, old, now, edge: true);
                    }
                    else if(ts == 4 && (rising || falling))
                    {
                        TriggerInput(value, old, now, edge: true);
                    }
                }
            }
        }

        private bool TriggerLevel()
        {
            var ts = (smcr >> 4) & 7;
            if(ts < 4) return itr[ts];
            if(ts == 5) return ((ccer >> 1) & 1) == 0 ? ti[0] : !ti[0]; // TI1FP1 with polarity
            return ti[0];
        }

        private void TriggerInput(bool level, bool old, double now, bool edge = false)
        {
            var sms = smcr & 7;
            var rising = edge || (level && !old);
            switch(sms)
            {
            case 4: // reset
                if(rising)
                {
                    Evaluate(now);
                    SetCounter(0, now);
                    if((cr1 & 4) == 0) sr |= 1;
                    sr |= 0x40;
                    UpdateIrq(now);
                }
                break;
            case 5: // gated
                {
                    Evaluate(now);
                    var g = TriggerLevel();
                    if(g != gate)
                    {
                        gate = g;
                        Recount(now);
                    }
                    if(rising) sr |= 0x40;
                    UpdateIrq(now);
                }
                break;
            case 6: // trigger
                if(rising)
                {
                    Evaluate(now);
                    sr |= 0x40;
                    if(!cen)
                    {
                        SetCounterEnable(true, now);
                    }
                    UpdateIrq(now);
                }
                break;
            case 7: // external clock mode 1: one count per edge
                if(rising && cen)
                {
                    Evaluate(now);
                    var c = Counter(now) + 1;
                    SetCounter(c, now, countsAsAbsolute: true);
                    sr |= 0x40;
                    UpdateIrq(now);
                }
                break;
            }
        }

        // ---- counting model ----
        /// The machine's time, the CPU's progress inside its quantum included
        /// (a CNT read mid-quantum: the AD2 stream ISR's latency and interval).
        private double Now()
        {
            if(machine.SystemBus.TryGetCurrentCPU(out var cpu)) cpu.SyncTime();
            return machine.ElapsedVirtualTime.TimeElapsed.TotalSeconds;
        }
        private ulong Period => (ulong)arr + 1;
        private double SecondsPerCount => (psc + 1.0) / frequency;

        /// Absolute counts since the segment anchor.
        private ulong Abs(double t)
        {
            if(!counting) return absAtSegStart;
            var c = (ulong)Math.Floor((t - segStart) / SecondsPerCount + 1e-9);
            return absAtSegStart + c;
        }

        private ulong Counter(double t) => Abs(t) % Period;

        private void SetCounterEnable(bool on, double now)
        {
            if(on == cen) return;
            cen = on;
            Recount(now);
            if(((cr2 >> 4) & 7) == 1) UpdateTrgo(now); // MMS = enable
        }

        /// Re-anchor the segment at `now` with the current counter value (after
        /// changes of PSC/ARR/CEN/gate). `change` applies the new timing AFTER
        /// the counter and the closed segment were computed with the old one.
        private void Recount(double now, Action change = null)
        {
            var cnt = Counter(now);
            // A running counter keeps its count phase (an ARR write mid-count
            // does not restart the prescaler): the new segment starts at the
            // last count edge, not at `now` (else every write lost up to one
            // count: the opendm40 AD2 stream ISR rewrites ARR every interval).
            var wasCounting = counting;
            var edge = wasCounting ? segStart + (Abs(now) - absAtSegStart) * SecondsPerCount : now;
            CloseSegment(now);
            change?.Invoke();
            counting = cen && gate;
            var start = wasCounting && counting ? edge : now;
            segStart = start;
            absAtSegStart = cnt;
            evalAbs = cnt;
            if(counting)
            {
                segments.Add(new Segment { Start = start, AbsStart = cnt, Period = Period, SecondsPerCount = SecondsPerCount, UpdatesBefore = updatesBeforeSeg });
            }
            generation++;
            UpdateTrgo(now);
            Schedule(now);
        }

        private void CloseSegment(double now)
        {
            if(segments.Count == 0) return;
            var s = segments[segments.Count - 1];
            if(s.End != null) return;
            s.End = now;
            var abs = Abs(now);
            s.Updates = abs / s.Period - s.AbsStart / s.Period;
            updatesBeforeSeg = s.UpdatesBefore + s.Updates;
            if(segments.Count > 64) segments.RemoveAt(0);
        }

        private void SetCounter(ulong value, double now, bool countsAsAbsolute = false)
        {
            CloseSegment(now);
            counting = cen && gate;
            segStart = now;
            absAtSegStart = value;
            evalAbs = value;
            if(counting)
            {
                segments.Add(new Segment { Start = now, AbsStart = value, Period = Period, SecondsPerCount = SecondsPerCount, UpdatesBefore = updatesBeforeSeg });
            }
            generation++;
            Schedule(now);
        }

        /// Number of update events between absolute counts a0 (exclusive) and a1 (inclusive) at phase `offset`.
        private ulong EventsBetween(ulong a0, ulong a1, ulong offset)
        {
            var p = Period;
            // a1 < offset: no event yet (kHi = 0 below would count k = 0;
            // a CCR compare flagged CCxIF before the counter reached it: the
            // comparator trigger's TIM2 CC2IF at once, a "done" record of a
            // few samples after the edge).
            if(offset >= p || a1 <= a0 || a1 < offset) return 0;
            // k such that a0 < k*p + offset <= a1
            ulong kLo = a0 < offset ? 0 : (a0 - offset) / p + 1;
            ulong kHi = a1 < offset ? 0 : (a1 - offset) / p;
            return kHi >= kLo ? kHi - kLo + 1 : 0;
        }

        /// Fold the events since the last evaluation into SR (lazy flags).
        private void Evaluate(double now)
        {
            var a1 = Abs(now);
            if(a1 <= evalAbs) return;
            var a0 = evalAbs;
            evalAbs = a1;
            var updates = EventsBetween(a0, a1, 0);
            if(updates > 0 && (cr1 & 2) == 0) sr |= 1; // UIF unless UDIS
            for(var i = 0; i < 4; i++)
            {
                if(EventsBetween(a0, a1, ccr[i]) > 0 && IsOutputOrCompare(i)) sr |= 2u << i;
            }
            if(updates > 0)
            {
                if((cr1 & 8) != 0 && cen) // one pulse: the counter stopped at the update
                {
                    cen = false;
                    CloseSegment(now);
                    counting = false;
                    segStart = now;
                    absAtSegStart = 0;
                    evalAbs = 0;
                    generation++;
                    if(((cr2 >> 4) & 7) == 1) UpdateTrgo(now);
                }
                if(((cr2 >> 4) & 7) == 2) Pulse(); // MMS = update
            }
            if(((cr2 >> 4) & 7) == 3 && (sr & 2) != 0) Pulse(); // compare pulse (CC1IF)
            UpdateIrq(now);
            UpdateTrgo(now);
        }

        private bool IsOutputOrCompare(int ch)
        {
            var ccmr = ch < 2 ? ccmr1 : ccmr2;
            var sh = (ch & 1) * 8;
            return ((ccmr >> sh) & 3) == 0; // CCxS = output
        }

        private bool OcRef(int ch, double now)
        {
            var ccmr = ch < 2 ? ccmr1 : ccmr2;
            var sh = (ch & 1) * 8;
            if(((ccmr >> sh) & 3) != 0) return false;
            var mode = (ccmr >> (sh + 4)) & 7;
            var cnt = Counter(now);
            switch(mode)
            {
            case 4: return false; // force inactive
            case 5: return true;  // force active
            case 6: return cnt < ccr[ch]; // PWM1
            case 7: return cnt >= ccr[ch]; // PWM2
            default: return ocref[ch];
            }
        }

        private void UpdateTrgo(double now)
        {
            var mms = (cr2 >> 4) & 7;
            bool level;
            switch(mms)
            {
            case 1: level = cen; break;
            case 4: case 5: case 6: case 7: level = OcRef((int)(mms - 4), now); break;
            default: return; // pulse-type modes handled by Pulse()
            }
            if(TRGO.IsSet != level)
            {
                TRGO.Set(level);
                TRGO2.Set(level);
            }
        }

        private void Pulse()
        {
            TRGO.Set(true); TRGO2.Set(true);
            TRGO.Set(false); TRGO2.Set(false);
        }

        private void UpdateIrq(double now)
        {
            var upd = (sr & 1) != 0 && (dier & 1) != 0;
            var cc = (sr & 0x1E & dier) != 0;
            var trg = (sr & 0x40) != 0 && (dier & 0x40) != 0;
            if(advanced)
            {
                UpdateInterrupt.Set(upd);
                CaptureCompareInterrupt.Set(cc);
                TriggerInterrupt.Set(trg);
            }
            IRQ.Set(upd || cc || trg);
        }

        /// Schedule the next event somebody depends on (interrupt, one-pulse
        /// stop, TRGO level change for a slave).
        private void Schedule(double now)
        {
            if(!counting) return;
            var mms = (cr2 >> 4) & 7;
            var trgoUsed = TRGO.IsConnected || TRGO2.IsConnected;
            var a = Abs(now);
            var p = Period;
            ulong? next = null;
            void Consider(ulong offset)
            {
                if(offset >= p) return;
                var k = a < offset ? 0 : (a - offset) / p + 1;
                var cand = k * p + offset;
                if(next == null || cand < next) next = cand;
            }
            if((dier & 1) != 0 || (cr1 & 8) != 0 || (mms == 2 && trgoUsed) || (mms >= 4 && trgoUsed))
            {
                Consider(0);
            }
            for(var i = 0; i < 4; i++)
            {
                if((dier & (2u << i)) != 0 || (mms == 3 && i == 0 && trgoUsed) || (mms == 4 + (uint)i && trgoUsed))
                {
                    Consider(ccr[i]);
                }
            }
            if(next == null) return;
            var dt = (next.Value - a) * SecondsPerCount;
            var gen = ++generation;
            if(dt < 1e-7) dt = 1e-7;
            // Nanoseconds (was microseconds: every timer interrupt came up to
            // 0.5 us off its update, a ~1 us "entry latency" of the AD2 stream
            // ISR that the device does not have; opendm40 TODO "76.8 kS/s").
            machine.ScheduleAction(TimeInterval.FromNanoseconds((ulong)Math.Max(1, Math.Round(dt * 1e9))), _ =>
            {
                if(gen != generation) return;
                var t = Now();
                Evaluate(t);
                Schedule(t);
            });
        }

        // ---- feeder API ----
        /// Update events since reset that happened at or before virtual time t.
        public ulong UpdateEventsUntil(double t)
        {
            if(segments.Count == 0) return updatesBeforeSeg;
            var s = segments[segments.Count - 1];
            if(s.End != null) return updatesBeforeSeg;
            var c = (ulong)Math.Floor((t - s.Start) / s.SecondsPerCount + 1e-9);
            var abs = s.AbsStart + c;
            return s.UpdatesBefore + abs / s.Period - s.AbsStart / s.Period;
        }

        /// Virtual time of update event n (0-based), or -1 if not (yet) known.
        public double TimeOfUpdate(ulong n)
        {
            for(var i = segments.Count - 1; i >= 0; i--)
            {
                var s = segments[i];
                if(n < s.UpdatesBefore) continue;
                var k = n - s.UpdatesBefore; // k-th update inside the segment
                var target = (s.AbsStart / s.Period + k + 1) * s.Period;
                var t = s.Start + (target - s.AbsStart) * s.SecondsPerCount;
                if(s.End != null && t > s.End.Value) return -1;
                return t;
            }
            return -1;
        }

        public bool Counting => counting;
        public double SampleRate => counting ? 1.0 / (SecondsPerCount * Period) : 0;

        private class Segment
        {
            public double Start;
            public double? End;
            public ulong AbsStart, Period, Updates, UpdatesBefore;
            public double SecondsPerCount;
        }

        private readonly IMachine machine;
        private readonly ulong frequency;
        private readonly bool advanced;
        private uint cr1, cr2, smcr, dier, sr, ccmr1, ccmr2, ccer, psc, arr, bdtr;
        private readonly uint[] ccr = new uint[4];
        private readonly bool[] ti = new bool[4];
        private readonly bool[] itr = new bool[4];
        private readonly bool[] ocref = new bool[4];
        private bool cen, gate, counting;
        private double segStart;
        private ulong absAtSegStart, evalAbs, updatesBeforeSeg;
        private ulong generation;
        private readonly List<Segment> segments = new List<Segment>();
    }
}
