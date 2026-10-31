//
// 3PA1030 scope ADC path + trigger comparator (stock/re/analysis/
// scope_gen_v2_6.h, firmware/src/scope.rs, hwtrig.rs). The real chain: TIM1_CH4
// requests -> DMA2 stream 4 reads GPIOB->IDR (PB0..PB9, 10-bit) at the TIM1
// update rate (below 50 MS/s through the TIM8-latched word); the comparator
// compares the front-end node with DAC2 and drives PA15 = TIM2_CH1.
// Here: sample n is TIM1's update event n (GD32_Timer.UpdateEventsUntil /
// TimeOfUpdate, so TIM5 trigger start, one-pulse stop and TIM2 gating are
// exact); its value is the scripted waveform at that virtual time, scaled
// with the front-end range read from PC13/PB15/PD12/PD13 (code = 512 + V *
// counts/V). Samples are delivered in bulk (GD32_DMA.FeedBlock) every tick and
// on demand whenever the firmware reads a DMA2 stream register, so NDTR is
// exact. Comparator edges are found one tick ahead and PA15 is driven at the
// edge's own virtual time; level = DAC2 code mapped with the stock default
// calibration (DAC 1780 -> code 300, 2401 -> code 780), 2 codes hysteresis.
// The board's comparator output is INVERTED (PA15 high while signal < level;
// firmware/src/hwtrig.rs: a rising-edge trigger captures a falling PA15) and
// lags the crossing by about 2 us (the stock's trig_delay); both modelled.
// SetWave(shape, hz, vpp, offset): sine | square | triangle | ramp | dc | noise.
// SetBitsFile(path): a bit pattern from a watched file (serial decoder tests).
//
using System;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.DMA;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public class ScopeFeeder : IDoubleWordPeripheral, IKnownSize
    {
        public ScopeFeeder(IMachine machine, GD32_DMA dma, IGPIOReceiver portA, GD32_Timer sampleTimer, int stream = 4, ulong tickUs = 100)
        {
            this.machine = machine;
            this.dma = dma;
            this.portA = portA;
            this.timer = sampleTimer;
            this.stream = stream;
            this.tickUs = tickUs;
            SetWave("sine", 1000, 1.0, 0.0);
            Reset();
            dma.BeforeStreamRead = s => { if(s == stream) CatchUp(); };
            machine.ScheduleAction(TimeInterval.FromMicroseconds(tickUs), _ => Tick());
        }

        public void Reset()
        {
            lastAbove = false;
            Samples = 0;
            fed = 0; scanned = 0; lastGen = ulong.MaxValue;
        }

        public long Size => 0x100;
        public uint ReadDoubleWord(long offset) => offset == 0 ? (uint)LastCode : 0;
        public void WriteDoubleWord(long offset, uint value) { }

        public void SetWave(string shape, double hz, double vpp, double offset)
        {
            this.shape = shape.ToLowerInvariant();
            this.hz = hz;
            this.vpp = vpp;
            this.offset = offset;
            this.Log(LogLevel.Info, "wave {0} {1} Hz {2} Vpp offset {3} V", shape, hz, vpp, offset);
        }

        /// A bit pattern on the input (shape "bits") from a file: first line
        /// "<bit s> <V low> <V high> <repeat s>", then '0' / '1' characters
        /// (others ignored), each one bit long, '0' = V low, '1' = V high;
        /// after the last bit the line stays at its level; repeat > 0 starts
        /// the pattern again every that many seconds. The file is watched:
        /// a rewrite (new mtime, checked every 50 ms of virtual time) loads
        /// the new pattern from that moment, so a test can change the signal
        /// while the emulator runs (emu/decodetest.py: UART / Modbus bits).
        public void SetBitsFile(string path)
        {
            bitPath = path;
            bitMtime = DateTime.MinValue;
            LoadBits();
        }

        private void LoadBits()
        {
            DateTime m;
            string t;
            try
            {
                m = System.IO.File.GetLastWriteTimeUtc(bitPath);
                if(m == bitMtime) return;
                t = System.IO.File.ReadAllText(bitPath);
            }
            catch(Exception e)
            {
                this.Log(LogLevel.Warning, "bits: {0}", e.Message);
                return;
            }
            var nl = t.IndexOf('\n');
            if(nl < 0) return;
            var h = t.Substring(0, nl).Split(new[] { ' ', '\t' }, StringSplitOptions.RemoveEmptyEntries);
            if(h.Length < 4) return;
            var inv = System.Globalization.CultureInfo.InvariantCulture;
            var b = new System.Collections.Generic.List<bool>(t.Length);
            for(var i = nl + 1; i < t.Length; i++)
            {
                if(t[i] == '0' || t[i] == '1') b.Add(t[i] == '1');
            }
            bitMtime = m;
            bits = b.ToArray();
            bitSec = double.Parse(h[0], inv);
            bitLo = double.Parse(h[1], inv);
            bitHi = double.Parse(h[2], inv);
            bitRepeat = double.Parse(h[3], inv);
            bitT0 = Now();
            shape = "bits";
            this.Log(LogLevel.Info, "bits: {0} bits of {1} s from {2} s, {3} / {4} V, repeat {5} s", bits.Length, bitSec, bitT0, bitLo, bitHi, bitRepeat);
        }

        public bool Comparator { get; set; } = true;
        public int Hysteresis { get; set; } = 2;
        /// Comparator output polarity: true = PA15 high while signal < level (the board).
        public bool Inverted { get; set; } = true;
        /// Comparator latency, microseconds (stock trig_delay = 2 us at the fast rates).
        public double DelayUs { get; set; } = 2.0;
        public ulong Samples { get; private set; }
        public ulong Fallbacks { get; private set; }
        public int LastCode { get; private set; }
        public double Fs => timer.SampleRate;
        public int Range { get; private set; }
        public int TriggerLevel { get; private set; }

        private void Tick()
        {
            try
            {
                if(bitPath != null && ++bitCheck * tickUs >= 50000)
                {
                    bitCheck = 0;
                    CatchUp();
                    LoadBits();
                }
                Step();
            }
            catch(Exception e)
            {
                this.Log(LogLevel.Error, "feeder: {0}", e.Message);
            }
            machine.ScheduleAction(TimeInterval.FromMicroseconds(tickUs), _ => Tick());
        }

        private double Now() => machine.ElapsedVirtualTime.TimeElapsed.TotalSeconds;

        /// Feed everything due up to now (called every tick and before DMA reads).
        private void CatchUp()
        {
            if(!dma.Enabled(stream)) return;
            var gen = dma.EnableCount(stream);
            if(gen != lastGen)
            {
                // New acquisition: samples count from this stream enable.
                lastGen = gen;
                var now0 = Now();
                var base0 = timer.UpdateEventsUntil(now0);
                fedBase = base0;
                fed = 0;
                scanned = 0;
                UpdateRange();
                this.Log(LogLevel.Debug, "acquisition {0}: {1} S/s, range {2}, trigger code {3}, base {4}, timer counting {5}", gen, timer.SampleRate, Range, TriggerCode(), fedBase, timer.Counting);
            }
            var now = Now();
            var due = timer.UpdateEventsUntil(now) - fedBase;
            if(due <= fed) return;
            var n = (int)Math.Min(due - fed, 500000);
            if(block.Length < n) block = new ushort[n];
            for(var i = 0; i < n; i++)
            {
                var t = timer.TimeOfUpdate(fedBase + fed + (ulong)i);
                if(t < 0)
                {
                    Fallbacks++;
                    if(Fallbacks < 5) this.Log(LogLevel.Warning, "no time for update {0} (base {1}, fed {2}, due {3})", fedBase + fed + (ulong)i, fedBase, fed, due);
                    t = now;
                }
                block[i] = (ushort)Code(t);
            }
            var took = dma.FeedBlock(stream, block, 0, n);
            fed += (ulong)took;
            Samples += (ulong)took;
            if(n > 0) LastCode = block[n - 1];
        }

        private void Step()
        {
            CatchUp();
            if(!dma.Enabled(stream) || !Comparator) return;
            // Look one tick ahead for comparator crossings and schedule PA15 at
            // the exact virtual time of the crossing sample.
            var now = Now();
            var horizon = now + 2.0 * tickUs / 1e6;
            var level = TriggerCode();
            TriggerLevel = level;
            if(scanned < fed) scanned = fed;
            var guard = 0;
            while(guard++ < 1000000)
            {
                var t = timer.TimeOfUpdate(fedBase + scanned);
                if(t < 0 || t > horizon) break;
                var code = Code(t);
                scanned++;
                var above = lastAbove;
                if(!lastAbove && code > level + Hysteresis) above = true;
                else if(lastAbove && code < level - Hysteresis) above = false;
                if(above != lastAbove)
                {
                    lastAbove = above;
                    var delay = t + DelayUs * 1e-6 - now;
                    var lvl = Inverted ? !above : above;
                    if(delay <= 0)
                    {
                        portA.OnGPIO(15, lvl);
                    }
                    else
                    {
                        machine.ScheduleAction(TimeInterval.FromMicroseconds((ulong)Math.Max(1, Math.Round(delay * 1e6))), _ => portA.OnGPIO(15, lvl));
                    }
                }
            }
        }

        private int Code(double t)
        {
            var v = Volts(t);
            var code = (int)Math.Round(512.0 + v * countsPerVolt);
            if(code < 0) code = 0;
            if(code > 1023) code = 1023;
            return code;
        }

        private double Volts(double t)
        {
            if(shape == "bits")
            {
                if(bits.Length == 0) return bitHi;
                var dt = t - bitT0;
                if(dt < 0) return bits[0] ? bitHi : bitLo;
                if(bitRepeat > 0) dt %= bitRepeat;
                var i = (long)(dt / bitSec);
                var b = i < bits.Length ? bits[i] : bits[bits.Length - 1];
                return b ? bitHi : bitLo;
            }
            var ph = (t * hz) % 1.0;
            if(ph < 0) ph += 1.0;
            double x;
            switch(shape)
            {
            case "square": x = ph < 0.5 ? 1 : -1; break;
            case "triangle": x = ph < 0.5 ? 4 * ph - 1 : 3 - 4 * ph; break;
            case "ramp": x = 2 * ph - 1; break;
            case "dc": x = 0; break;
            case "noise": x = 2 * rng.NextDouble() - 1; break;
            default: x = Math.Sin(2 * Math.PI * ph); break;
            }
            return offset + x * vpp / 2;
        }

        // Front-end range from PC13/PB15/PD12/PD13 (stock scope_range_gpio table).
        private void UpdateRange()
        {
            var bus = machine.SystemBus;
            var pc13 = (bus.ReadDoubleWord(GPIOC + 0x14) >> 13) & 1;
            var pb15 = (bus.ReadDoubleWord(GPIOB + 0x14) >> 15) & 1;
            var pd12 = (bus.ReadDoubleWord(GPIOD + 0x14) >> 12) & 1;
            var pd13 = (bus.ReadDoubleWord(GPIOD + 0x14) >> 13) & 1;
            var r = 10;
            for(var i = 0; i < RangeGpio.Length; i++)
            {
                if(RangeGpio[i][0] == pc13 && RangeGpio[i][1] == pb15 && RangeGpio[i][2] == pd12 && RangeGpio[i][3] == pd13)
                {
                    r = i;
                    break;
                }
            }
            Range = r;
            countsPerVolt = CountsPerVolt[r];
        }

        private int TriggerCode()
        {
            var dac = (int)(machine.SystemBus.ReadDoubleWord(DAC_DHR12R2) & 0xFFF);
            return 300 + (int)((dac - 1780) * (780.0 - 300.0) / (2401.0 - 1780.0));
        }

        private static readonly uint[][] RangeGpio =
        {
            new uint[] { 1, 1, 1, 0 }, new uint[] { 1, 1, 1, 1 }, new uint[] { 1, 0, 1, 1 }, new uint[] { 1, 0, 0, 1 },
            new uint[] { 1, 0, 1, 0 }, new uint[] { 0, 1, 1, 0 }, new uint[] { 0, 1, 1, 1 }, new uint[] { 0, 1, 0, 1 },
            new uint[] { 0, 0, 0, 1 }, new uint[] { 0, 0, 1, 0 }, new uint[] { 0, 0, 0, 0 },
        };
        private static readonly double[] CountsPerVolt =
            { 10800, 5375, 2150, 1080, 537.5, 215, 108, 53.75, 21.5, 11, 5.5 };

        private const ulong GPIOB = 0x40020400;
        private const ulong GPIOC = 0x40020800;
        private const ulong GPIOD = 0x40020C00;
        private const ulong DAC_DHR12R2 = 0x40007414;

        private readonly IMachine machine;
        private readonly GD32_DMA dma;
        private readonly IGPIOReceiver portA;
        private readonly GD32_Timer timer;
        private readonly int stream;
        private readonly ulong tickUs;
        private readonly Random rng = new Random(1);
        private string shape;
        private double hz, vpp, offset, countsPerVolt = 108;
        private bool lastAbove;
        private ushort[] block = new ushort[4096];
        private ulong lastGen, fedBase, fed, scanned;
        private bool[] bits = new bool[0];
        private double bitSec = 1e-3, bitLo, bitHi = 3.3, bitRepeat, bitT0;
        private string bitPath;
        private DateTime bitMtime;
        private ulong bitCheck;
    }
}
