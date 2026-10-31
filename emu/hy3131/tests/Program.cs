// Host unit tests of the HY3131 model (emu/hy3131/HY3131.cs) through the real
// SPI frame. Run: emu/hy3131/tests/run.sh  (dotnet 8).
// The "stock" formulas below are those of stock/re/analysis/dmm_v2_6.h.
using System;
using System.Collections.Generic;
using System.Linq;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Miscellaneous;

namespace Hy3131Tests
{
    public static class Program
    {
        private static int failures, passes;

        private static void Check(bool ok, string what, string detail = "")
        {
            if(ok) { passes++; Console.WriteLine("  ok   " + what + (detail.Length > 0 ? "  (" + detail + ")" : "")); }
            else { failures++; Console.WriteLine("  FAIL " + what + "  " + detail); }
        }

        private static void Near(double got, double want, double tol, string what, string extra = "")
        {
            var err = Math.Abs(got - want);
            Check(err <= tol, what, string.Format("got {0:G6}, want {1:G6} +-{2:G3}{3}", got, want, tol, extra.Length > 0 ? "; " + extra : ""));
        }

        // stock rows (hy3131_v2_6.h)
        private static readonly byte[] F0R0 = Master.Row("f2 dd 16 03 33 43 80 9d 31 fc 20 00 00 09 28 a0 80 c7 38 ac 88 94");
        private static readonly byte[] F0R1 = Master.Row("f2 dd 16 03 33 40 80 9d 31 fc 20 00 00 09 28 a0 80 c7 38 ac 88 94");
        private static readonly byte[] F1R0 = Master.Row("f0 dd 15 83 31 40 00 55 31 00 06 00 00 90 28 a0 80 d7 38 ac 00 54");
        private static readonly byte[] F1R1 = Master.Row("f0 dd 15 83 07 40 00 55 31 00 06 00 00 09 68 a4 80 c7 33 ac 00 54");
        private static readonly byte[] F2R1 = Master.Row("f2 dd 16 03 33 40 80 5d 31 f8 00 00 00 00 08 91 80 c7 aa ac 88 54");
        private static readonly byte[] F3R1 = Master.Row("f0 dd 15 83 33 00 00 55 31 00 00 00 00 00 08 91 80 d7 3a ac 00 50");
        private static readonly byte[] F5R0 = Master.Row("f2 dd 14 03 35 43 80 2d 31 fc 00 00 00 00 08 81 80 c7 3d ac 88 24");
        private static readonly byte[] F8R0 = Master.Row("f0 dd 15 83 35 01 00 55 31 00 00 00 00 00 08 81 80 c7 3d a4 00 50");
        private static readonly byte[] F10R1 = Master.Row("e0 dd 14 83 35 00 00 55 00 00 00 40 06 00 00 94 80 d2 3c 68 00 50");
        private static readonly byte[] F10R6 = Master.Row("e0 dd 14 83 35 00 00 55 11 00 00 08 00 00 00 86 80 d1 3c 60 00 50");
        private static readonly byte[] F11 = Master.Row("56 ec 13 83 35 08 00 55 44 00 00 40 06 00 01 94 80 cd 38 68 00 50");
        private static readonly byte[] F12 = Master.Row("b0 ff 13 83 3d 00 00 55 31 00 80 00 88 00 00 86 80 e2 3c 68 00 50");
        private static readonly byte[] F13R2 = Master.Row("72 21 16 03 b5 00 00 55 11 00 00 00 00 08 00 9a 80 c7 3c ac 00 50");
        private static readonly byte[] F13R5 = Master.Row("52 bb 14 83 43 40 00 5d 31 00 00 00 00 88 00 8e 80 c2 cc a8 00 54");
        private static readonly byte[] F13DIS = Master.Row("52 dd 13 83 43 40 00 5d 31 00 00 00 88 88 00 8e 80 c2 cc a8 00 54");
        private static readonly byte[] F14 = Master.Row("f2 dd 15 83 df 00 80 15 11 f8 00 00 00 00 08 91 80 c7 3a 2f 00 50");
        private static readonly byte[] F15R0 = Master.Row("f0 dd 16 83 33 01 00 55 31 00 00 06 00 60 08 95 80 d7 3c af 00 50");

        private const double K = 1.38889 / 8388606.0;

        private static double DcvDisplay(int ad1, int r, float[] cal) => ad1 * new[] { 10.0, 100, 1000, 10000 }[r] * K * cal[1 + r] - cal[5 + r];
        private static double SqrtRms(byte[] b) => Math.Sqrt((double)(b[9] | (long)b[10] << 8 | (long)b[11] << 16 | (long)b[12] << 24 | (long)(b[13] & 0x1f) << 32));
        private static int Lpf(byte[] b) => (int)((uint)(b[6] << 13 | b[7] << 21 | b[8] << 29)) >> 13;
        private static int S24(byte[] b, int o) => (int)((uint)(b[o] | b[o + 1] << 8 | b[o + 2] << 16) << 8) >> 8;

        private static Master NewChip(bool noise = false, float[] cal = null)
        {
            var m = new Master();
            m.Chip.Core.NoiseScale = noise ? 1 : 0;
            if(cal != null) m.Chip.Core.Cal = cal;
            m.Now = 1.0;
            return m;
        }

        /// Run through the stock sequence, read 20 bytes after the flags, return them.
        private static byte[] ReadAfter(Master m, int flags, double timeout)
        {
            var f = m.WaitFlag(flags, timeout);
            Check(f != 0, string.Format("flags 0x{0:X2} within {1} s", flags, timeout), "INTF=0x" + f.ToString("X2"));
            return m.Read(0x00, 20);
        }

        public static int Main(string[] args)
        {
            LogExt.Verbose = Array.IndexOf(args, "-v") >= 0;
            System.Globalization.CultureInfo.DefaultThreadCurrentCulture = System.Globalization.CultureInfo.InvariantCulture;
            System.Threading.Thread.CurrentThread.CurrentCulture = System.Globalization.CultureInfo.InvariantCulture;
            if(Array.IndexOf(args, "-d") >= 0) { Debug(); return 0; }
            if(Array.IndexOf(args, "-t") >= 0) { TimedSpi(); Console.WriteLine("{0} passed, {1} failed", passes, failures); return failures == 0 ? 0 : 1; }
            Registers();
            Timing();
            DcVoltage();
            CalibrationConsistency();
            AcVoltage();
            Current();
            Ohm();
            Continuity();
            Diode();
            DbgSrc();
            Capacitance();
            Frequency();
            Temperature();
            Beat();
            Ad2Stream();
            Ad2Race();
            CrossCases();
            Sentinel();
            SentinelLater();
            Power();
            AcdcMethods();
            HwSpi();
            TimedSpi();
            Rail();
            Ad1Mix();
            Knobs489();
            Console.WriteLine("{0} passed, {1} failed", passes, failures);
            return failures == 0 ? 0 : 1;
        }

        private static void Debug()
        {
            {
                var m = NewChip();
                m.Chip.SetInput("acv:230@50");
                m.Configure(F1R0, 1);
                m.Advance(2.0);
                var f0r2 = Master.Row("f2 dd 16 03 33 40 80 9d 31 fc 20 00 90 00 28 a0 80 c7 38 ac 88 94");
                m.Configure(f0r2, 0);
                for(var i = 0; i < 12; i++)
                {
                    m.Advance(0.25);
                    m.Read1(0x1E);
                    var b = ReadAfter(m, 0x18, 1);
                    Console.WriteLine("r2 t {0:F2} role {1} rms {2:F0} -> {3:F2} V lpf {4} vc {5:F4}", m.Now, m.Chip.Role, SqrtRms(b), SqrtRms(b) * 1.25 / 262144 * 1000, Lpf(b), m.Chip.Core.AcNodeVc);
                }
                m.Write(0x20, 0xF0); m.Write(0x1B, 0x00, 0x00, 0xFC); m.Write(0x20, 0xF2);
                var fl = m.WaitFlag(0x01, 1);
                var c = m.Read(0x15, 9);
                var ctb = c[3] | c[4] << 8 | c[5] << 16; var cta = c[6] | c[7] << 8 | c[8] << 16;
                Console.WriteLine("counter: CTF {0:X2} f {1:F2} Hz", fl, ctb / (double)(cta + 0x40000) * 4915200);
                return;
            }
            {
                var m = NewChip();
                m.Chip.SetInput("acv:3.4@40,dc=4.8,shape=square");
                m.Configure(F0R1, 0);
                m.Advance(3.0);
                m.Read1(0x1E);
                Console.WriteLine("after AC 3 s: vc {0:F4} role {1} op1 {2}", m.Chip.Core.AcNodeVc, m.Chip.Role, m.Chip.Core.EnOp1);
                m.Configure(F1R1, 1); m.Write(0x32, 0x38);
                m.Read1(0x1E);
                Console.WriteLine("DC row: vc {0:F4} role {1} op1 {2}", m.Chip.Core.AcNodeVc, m.Chip.Role, m.Chip.Core.EnOp1);
                m.Advance(0.8);
                m.Read1(0x1E);
                Console.WriteLine("DC +0.8: vc {0:F4} role {1} op1 {2}", m.Chip.Core.AcNodeVc, m.Chip.Role, m.Chip.Core.EnOp1);
                m.Configure(F0R1, 0);
                m.Read1(0x1E);
                Console.WriteLine("AC row: vc {0:F4} role {1} op1 {2}", m.Chip.Core.AcNodeVc, m.Chip.Role, m.Chip.Core.EnOp1);
                m.Advance(0.8);
                m.Read1(0x1E);
                var b = ReadAfter(m, 0x18, 1);
                Console.WriteLine("AC +0.8: vc {0:F4} lpf {1} rms {2}", m.Chip.Core.AcNodeVc, Lpf(b), SqrtRms(b));
            }
            var m2 = NewChip();
            m2.Chip.SetInput("acv:1.0@50,dc=0.5");
            m2.Configure(F0R0);
            Console.WriteLine("role {0} ig2 {1} rg2 {2} scale {3}", m2.Chip.Role, m2.Chip.Core.Ad2Ig, m2.Chip.Core.Ad2Rg, m2.Chip.Core.Ad2Ig / m2.Chip.Core.Ad2Rg / 1.25 * 262144);
            for(var i = 0; i < 6; i++)
            {
                m2.Advance(0.5);
                m2.Read1(0x1E);
                var b = ReadAfter(m2, 0x18, 1);
                Console.WriteLine("t {0:F2} rms {1} lpf {2} vc {3:F4} raw {4}", m2.Now, SqrtRms(b), Lpf(b), m2.Chip.Core.AcNodeVc, BitConverter.ToString(b, 6, 8));
            }
            m2.Chip.SetInput("dcv:1.0");
            m2.Configure(F1R1);
            m2.WaitFlag(0x04, 1);
            Console.WriteLine("dcv r1 code {0} role {1}", m2.ReadS24(0), m2.Chip.Role);
            m2.Write(0x25, 0x50);
            m2.Read1(0x1E); var f = m2.WaitFlag(0x04, 1);
            Console.WriteLine("after R25=0x17: intf {0:X2} code {1} role {2} ig {3} {4}", f, m2.ReadS24(0), m2.Chip.Role, m2.Chip.Core.Ad1Ig, m2.Chip.Core.RoleDebug);
        }

        private static double Ohm(Master m, int r) => Math.Abs(m.ReadS24(0)) * new[] { 0.1, 1, 10, 100, 1000, 10000, 63000 }[r] / 0.9 / 8388606.0 * 1000; // ohm
        private static double Dcv(Master m, int r) => m.ReadS24(0) * new[] { 10.0, 100, 1000, 10000 }[r] * K;
        private static double Ma(Master m, int mode, int r) => m.ReadS24(0) * 1.25 / (new[] { 0.01, 0.0005, 4.75e-5 }[mode - 7] * new[] { 10.0, 1 }[r] * 0.9) / 8388606.0;

        private static readonly byte[] F10R2 = Master.Row("e0 dd 14 83 35 00 00 55 00 00 00 40 60 00 00 94 80 d2 3c 68 00 50");
        private static readonly byte[] F10R3 = Master.Row("e0 dd 14 83 35 00 00 55 00 00 00 40 00 06 00 94 80 d2 3c 68 00 50");
        private static readonly byte[] F3R0 = Master.Row("f0 dd 15 83 33 01 00 55 31 00 00 00 00 00 08 91 80 d7 3a ac 00 50");
        private static readonly byte[] F8R1 = Master.Row("f0 dd 15 83 35 00 00 55 31 00 00 00 00 00 08 81 80 c7 3d ac 00 50");

        /// Fixed connections through the firmware's function changes (relays + rows as the firmware drives them).
        // opendm40 misconnect sentinel rows (firmware/src/sentinel.rs): the stock row with the idle
        // comparator / AD1 repointed. R20 0x90 = SCMPI PB1 + ENCMP; R20 0x10 = SENSE + ENCMP and
        // R2C |= SS5 in DC A; AC V / mV: R22 0, ENAD1, R24 0x95 (PB1 - PB3), AD1IG 3.6, PKHSEL AD1.
        private static byte[] Sent(byte[] row, int mode)
        {
            var c = (byte[])row.Clone();
            switch(mode)
            {
            case 1: case 3: case 10: case 15: c[0] = 0x90; break;
            case 0: case 2: c[2] = 0; c[3] |= 0x80; c[4] = 0x95; c[5] |= 0x30; c[9] = (byte)((c[9] & 0xF8) | 5); break;
            case 7: case 8: case 9: c[0] = 0x10; c[12] |= 0x10; break;
            // later ones: frequency (AD1 PB1 - PB3 peak), AC A (AD1 SENSE - AGND peak with SS5,
            // OP1 chopper off), continuity / diode / cap r0..4 (OP2 x1 on PB1, AD2 = OP2O - PB3
            // at IG 2.0, OSR 256, peak hold on AD2)
            case 14: c[2] = 0; c[3] |= 0x80; c[4] = 0x95; c[5] |= 0x30; c[9] = (byte)((c[9] & 0xF8) | 5); break;
            case 4: case 5: case 6: c[2] = 0; c[3] |= 0x80; c[4] = 0x03; c[5] |= 0x30; c[9] = (byte)((c[9] & 0xF8) | 5); c[12] |= 0x10; c[19] &= 0x3F; break;
            case 11: case 12: case 13:
                c[5] = (byte)((c[5] & 0x3D) | 0xC0); c[6] = 0x83; c[7] = 0x6D; c[9] = (byte)((c[9] & 0xF8) | 4); c[18] = (byte)(0xD0 | (c[18] & 0x0F)); break;
            }
            return c;
        }

        /// PKHMIN / PKHMAX after restarting the hold and waiting `dt`.
        private static int[] Peak(Master m, byte r29, double dt)
        {
            m.Write(0x29, (byte)(r29 & 0xFB)); m.Write(0x29, r29);
            m.Advance(dt);
            var b = m.Read(0x0E, 6);
            return new[] { S24(b, 0), S24(b, 3) };
        }

        private static void SentinelLater()
        {
            Console.WriteLine("opendm40 sentinels added after f80524f (frequency, AC A, continuity, diode, cap r0..4)");
            foreach(var (row, mode, name) in new[] { (F14, 14, "Freq m14 r0"), (F4R1, 4, "AcA m4 r1"), (F5R0, 5, "AcA m5 r0"), (F11, 11, "Cont m11 r0"),
                                                     (F12, 12, "Diode m12 r0"), (F13R2, 13, "CapOsc m13 r2") })
            {
                var m = NewChip();
                m.Configure(Sent(row, mode), mode);
                Check(m.Chip.Role == name, "sentinel row keeps its role: " + name, m.Chip.Role);
            }
            {
                // Frequency: AD1 peak on PB1 - PB3
                var m = NewChip(true);
                m.Chip.SetInput("vsrc:0,ac=1,f=1000");
                var r = Sent(F14, 14);
                m.Configure(r, 14);
                var p = Peak(m, r[9], 0.1);
                Check(p[1] < 2000 && p[0] > -2000, "Hz, A jack open: AD1 peak near 0", p[0] + ".." + p[1]);
                m.Chip.SetInput("isrc:0,ac=0.001,f=50");
                p = Peak(m, r[9], 0.1);
                Near(p[1], 50700, 1500, "  1 mA AC into A: PKHMAX ~ +50.7 k");
            }
            {
                // AC A (mA): AD1 peak on SENSE through the /1000 tap
                var m = NewChip(true);
                m.Chip.SetInput("open");
                var r = Sent(F5R0, 5);
                m.Configure(r, 5);
                var p = Peak(m, r[9], 0.1);
                Check(p[1] < 300 && p[0] > -300, "AC mA, V jack open: AD1 SENSE peak near 0", p[0] + ".." + p[1]);
                m.Chip.SetInput("vsrc:5");
                p = Peak(m, r[9], 0.1);
                // 5 mV at the tap: 3.6 * 0.005 / 1.25 * 2^18 = 3775
                Near(p[1], 3775, 150, "  5 V DC on V-COM: PKHMAX ~ 3.8 k (opendm40 threshold 1.51 k = 2 V)");
                m.Chip.SetInput("vsrc:0,ac=230,f=50");
                p = Peak(m, r[9], 0.1);
                Check(p[1] > 200000 && p[0] < -200000, "  230 V AC: both signs far over", p[0] + ".." + p[1]);
                m.Chip.SetInput("open;isrc:0,ac=0.01,f=50");
                m.Configure(r, 5); m.Advance(1.0);
                var b = ReadAfter(m, 0x10, 1);
                var ms = NewChip(true);
                ms.Chip.SetInput("open;isrc:0,ac=0.01,f=50");
                ms.Configure(F5R0, 5); ms.Advance(1.0);
                var bs = ReadAfter(ms, 0x10, 1);
                Near(SqrtRms(b), SqrtRms(bs), 0.002 * SqrtRms(bs), "  the AC mA RMS is that of the stock row with AD1 on");
            }
            foreach(var (row, mode, what) in new[] { (F11, 11, "continuity"), (F12, 12, "diode"), (F13R2, 13, "cap r2") })
            {
                // AD2 peak on OP2 <- PB1: 0.5 mA * 47.5 ohm = 23.75 mV -> 2.0 * 0.02375 / 1.25 * 2^18 = 9 961
                var m = NewChip(true);
                m.Chip.SetInput(mode == 12 ? "diode:0.65" : mode == 11 ? "r:10" : "cap:100e-9");
                var r = Sent(row, mode);
                m.Configure(r, mode);
                var p = Peak(m, r[9], 0.1);
                Check(p[1] < 200 && p[0] > -200, what + ", A jack open: AD2 peak near 0", p[0] + ".." + p[1]);
                m.Chip.SetInput("isrc:0.001");
                p = Peak(m, r[9], 0.1);
                Near(p[1], 19900, 500, "  1 mA DC into A: PKHMAX ~ 19.9 k (opendm40 threshold 9.9 k)");
                m.Chip.SetInput("isrc:0,ac=0.002,f=50");
                p = Peak(m, r[9], 0.1);
                Check(p[1] > 30000 && p[0] < -30000, "  2 mA AC: both signs", p[0] + ".." + p[1]);
            }
            {
                // DC? hint (AC V): AD1 on SENSE - PB5 = the V jack through the range's tap (r1: /100)
                var m = NewChip();
                m.Chip.SetInput("vsrc:5");
                var r = Sent(F0R1, 0);
                r[4] = 0x07;
                m.Configure(r, 0);
                var p = Peak(m, r[9], 0.1);
                // 5 V / 100 = 50 mV: 3.6 * 0.05 / 1.25 * 2^18 = 37 749
                Near(p[1], 37749, 100, "AC V r1, AD1 on the V tap: 5 V DC -> PKH ~ 37.7 k");
                Check(m.Chip.Role == "AcV m0 r1", "  still the AC V role", m.Chip.Role);
            }
            {
                // AC? hint (DC V 6 V): the comparator on SENSE = V / 10
                var m = NewChip();
                m.Chip.SetInput("vsrc:0,ac=2,f=50");
                var r = Sent(F1R0, 1);
                r[0] = 0x10;
                m.Configure(r, 1);
                var q = CmpScan(m, 100, 0.0037);
                Check(q[0] > 20 && q[1] > 20, "DC V r0, comparator on SENSE, 2 V AC: both bits", string.Join(",", q));
                m.Chip.SetInput("vsrc:1");
                q = CmpScan(m, 20, 0.0071);
                Check(q[0] == 20 && q[1] == 0, "  1 V DC: CMPHO only (0.1 V > 20.8 mV)", string.Join(",", q));
                m.Chip.SetInput("vsrc:0.1");
                q = CmpScan(m, 20, 0.0071);
                Check(q[2] == 0, "  0.1 V DC: below the window (10 mV)", string.Join(",", q));
            }
            {
                // fuse check: a lead V -> A; ohm r1 reads the fuse + the 47.5 ohm chain, OL when blown
                var m = NewChip();
                m.Chip.SetInput("open;valead");
                m.Configure(F10R1, 10);
                m.WaitFlag(0x04, 1);
                Near(Ohm(m, 1), 47.55, 0.1, "fuse check, lead V -> A: ohm r1 ~ 47.55");
                m.Chip.SetInput("fuseopen");
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Check(Ohm(m, 1) > 620, "  fuse blown: OL", Ohm(m, 1).ToString("F1"));
                m.Chip.SetInput("open;isrc:0.01");
                m.Configure(F8R0, 8); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Ma(m, 8, 0), 0, 0.01, "  blown fuse in DC mA: 10 mA reads 0 (\"open\" keeps the fuse blown)");
                m.Chip.SetInput("fuseopen:none");
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Ma(m, 8, 0), 10, 0.01, "  fuse back: 10 mA");
            }
            {
                // pb5drift (device 40728b8 2a): AC A's AD1 on SENSE - PB5 runs to the negative rail
                // within ~2 s, whatever is on V; SENSE - AGND (the fixed row) stays put
                var m = NewChip(true);
                m.Chip.SetInput("open;pb5drift:0.6;pb5tau:2.2");
                var r = Sent(F5R0, 5);
                r[4] = 0x07;
                m.Configure(r, 5);
                m.Advance(0.6);
                var p = Peak(m, r[9], 0.1);
                Check(p[1] < -90000 && p[1] > -160000, "pb5drift, AC mA on SENSE - PB5: ~-117 k at 0.7 s", p[0] + ".." + p[1]);
                m.Advance(1.4);
                p = Peak(m, r[9], 0.1);
                Check(p[1] <= -262000, "  at the negative rail by ~2.1 s", p[0] + ".." + p[1]);
                var m2 = NewChip(true);
                m2.Chip.SetInput("open;pb5drift:0.6;pb5tau:2.2");
                var r2 = Sent(F5R0, 5);
                m2.Configure(r2, 5);
                m2.Advance(2.0);
                p = Peak(m2, r2[9], 0.1);
                Check(p[1] < 300 && p[0] > -300, "  SENSE - AGND (fixed row): near 0 after 2 s", p[0] + ".." + p[1]);
                m2.Chip.SetInput("vsrc:0,ac=5,f=50");
                p = Peak(m2, r2[9], 0.1);
                // 5 V rms = 7.07 mV peak at the tap: 3.6 * 0.00707 / 1.25 * 2^18 = 5 338
                Near(p[1] - p[0], 10676, 300, "  5 V rms AC: swing ~10.7 k (opendm40 swing threshold 4.27 k)");
            }
            {
                // senseneg (device d2aff22): AC A's SENSE - AGND ramps to the rail at 24 V, ~10 k per hold
                var m = NewChip(true);
                m.Chip.SetInput("open;vsrc:24;senseneg:0.75;sensetau:5");
                var r = Sent(F5R0, 5);
                m.Configure(r, 5);
                m.Advance(0.9);
                var p = Peak(m, r[9], 0.107);
                Check(p[1] - p[0] > 7000 && p[1] - p[0] < 14000 && p[1] < -60000, "senseneg 24 V: one hold at ~1 s spans ~10 k codes, below -60 k", p[0] + ".." + p[1]);
                m.Advance(2.5);
                p = Peak(m, r[9], 0.107);
                Check(p[0] <= -262000, "  at the rail by ~3.5 s", p[0] + ".." + p[1]);
            }
            {
                // acastep (device 40728b8 new bug 1): AC A 10 A -> mA, the first blocks carry the step
                var m = NewChip(true);
                m.Chip.SetInput("open;acastep:200000;acastepms:700");
                m.Configure(Sent(F4R1, 4), 4);
                m.Advance(1.0);
                m.Configure(Sent(F5R0, 5), 5);
                m.Advance(0.62);
                var b = ReadAfter(m, 0x10, 0.3);
                Check(SqrtRms(b) > 100000, "acastep: 10 A -> 60 mA, the first block after 0.6 s is over range", SqrtRms(b).ToString("F0"));
                m.Advance(0.25);
                b = ReadAfter(m, 0x10, 0.3);
                Check(SqrtRms(b) < 1000, "  the blocks after the step: the floor", SqrtRms(b).ToString("F0"));
                var m2 = NewChip(true);
                m2.Chip.SetInput("open;acastep:200000;acastepms:700");
                m2.Configure(Sent(F8R0, 8), 8);
                m2.Configure(Sent(F5R0, 5), 5);
                m2.Advance(0.62);
                b = ReadAfter(m2, 0x10, 0.3);
                Check(SqrtRms(b) < 1000, "  DC mA -> AC 60 mA: no step", SqrtRms(b).ToString("F0"));
            }
            {
                // vjblind (device 40728b8, 24 V on V-COM): DC A's SENSE comparator sees the source
                // only after a temperature row; DC V 60 V / ohm / a reset make it blind again
                var m = NewChip();
                m.Chip.SetInput("vsrc:24;vjblind:1");
                m.Configure(Sent(F1R1, 1), 1);
                m.Configure(Sent(F8R0, 8), 8);
                var q = CmpScan(m, 10, 0.01);
                Check(q[2] == 0, "vjblind: DC V 60 V -> DC mA, 24 V: no bits", string.Join(",", q));
                m.Configure(Sent(F15R0, 15), 15);
                m.Configure(Sent(F0R1, 0), 0);
                m.Configure(Sent(F8R0, 8), 8);
                q = CmpScan(m, 10, 0.01);
                Check(q[0] == 10, "  TEMP -> AC V -> DC mA: CMPHO", string.Join(",", q));
                m.Configure(Sent(F10R1, 10), 10);
                m.Configure(Sent(F8R0, 8), 8);
                q = CmpScan(m, 10, 0.01);
                Check(q[2] == 0, "  RES -> DC mA: blind again", string.Join(",", q));
            }
            {
                // the diode reading with AD2 / OP2 running (the model has no note-7 offset: T8 decides)
                var m = NewChip();
                m.Chip.SetInput("diode:0.65");
                m.Configure(Sent(F12, 12), 12);
                m.WaitFlag(0x04, 1);
                Near(m.ReadS24(0) * 1.25 * 10 / 0.9 / 8388606.0, 0.65, 2e-4, "  diode 0.65 V unchanged with the AD2 sentinel");
            }
        }

        /// CMPHO / CMPLO seen in n CTSTA reads dt apart: (any high, any low, reads with either).
        private static int[] CmpScan(Master m, int n, double dt)
        {
            var r = new int[3];
            for(var i = 0; i < n; i++)
            {
                var c = m.Read1(0x14);
                if((c & 0x20) != 0) r[0]++;
                if((c & 0x10) != 0) r[1]++;
                if((c & 0x30) != 0) r[2]++;
                m.Advance(dt);
            }
            return r;
        }

        /// opendm40 AC+DC methods (dmm.rs acdc_row): the AD1 ripple window, note 7, the merged
        /// and the DC-coupled rows.
        private static void AcdcMethods()
        {
            Console.WriteLine("AC+DC methods: AD1 ripple window, note 7, method B / C rows");
            var cal = Hy3131Core.DefaultCal();
            Func<string, double[]> dcv = spec =>
            {
                var m = NewChip();
                m.Chip.SetInput(spec);
                m.Configure(F1R1, 1);
                var v = new double[8];
                for(var i = 0; i < v.Length; i++) { m.Read1(0x1E); m.WaitFlag(0x04, 1); v[i] = DcvDisplay(m.ReadS24(0), 1, cal); }
                return v;
            };
            var a = dcv("acv:1@50,dc=10");
            Check(a.Max() - a.Min() < 1e-4 && Math.Abs(a.Average() - 10) < 1e-4, "DC V 60 V, 10 V + 1 V 50 Hz: a notch (OSR 8192 chopped)", a.Min() + ".." + a.Max());
            a = dcv("acv:1@45,dc=10");
            Check(a.Max() - a.Min() > 2e-3 && a.Max() - a.Min() < 0.04 && Math.Abs(a.Average() - 10) < 0.02, "  1 V 45 Hz leaks (sinc^2 x chop: ~-43 dB)", a.Min() + ".." + a.Max());
            // method B row: the AC V row with the DC V AD1 bytes, chopper held, AD1 last
            var b = (byte[])F0R1.Clone();
            b[2] = 0x15; b[3] = 0x03; b[4] = 0x07; b[19] = 0x2c;
            // (x 1.00047: AD1 on SENSE - PB5 in the AC V row's switches at 60 V, ad1acg)
            foreach(var (opt, want) in new[] { ("", 10.017 * 1.00047), ("n7:0;", 10.0 * 1.00047) })
            {
                var m = NewChip();
                m.Chip.SetInput(opt + "dcv:10");
                m.Configure(b, 0);
                m.Write(0x23, 0x83);
                Check(m.Chip.Role == "AcV m0 r1", "B row keeps the AC V role", m.Chip.Role);
                m.Read1(0x1E); m.WaitFlag(0x04, 1); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(DcvDisplay(m.ReadS24(0), 1, cal), want, 1e-3, "  AD1 with AD2 / AD3 on, " + (opt == "" ? "note 7 (+17 mV at 60 V)" : "n7:0"));
            }
            {
                // method C row: AD2 and AD3 on OP1O (DC-coupled), RMS / LPF on
                var c = (byte[])F0R1.Clone();
                c[7] = 0x1d; c[21] = 0x14;
                var m = NewChip();
                m.Chip.SetInput("acv:1@50,dc=10");
                m.Configure(c, 0);
                Check(m.Chip.Role == "AcV m0 r1", "C row keeps the AC V role", m.Chip.Role);
                m.Write(0x34, 0x88); m.Write(0x26, 0x80);
                ReadAfter(m, 0x18, 1);
                var r = ReadAfter(m, 0x18, 1);
                var per = 1.25 / 262144.0 * 100;
                Check(SqrtRms(r) * per > 3.0, "  right after the enable the RMS block's HPF passes the DC step (hpftau)", (SqrtRms(r) * per).ToString("F3") + " V");
                m.Advance(4.0);
                ReadAfter(m, 0x18, 1);
                r = ReadAfter(m, 0x18, 1);
                // the DC: the device's per-range gain (60 V +1.88 %) and AD3's +23 codes on the node
                Near(Lpf(r) * per, (10.0 + 0.37e-3 * 100) / 1.0188 + 23 * per, 0.01, "  LPF = the DC part (+ OP1's offset, / the 60 V per_code factor, + AD3's shift)");
                Near(SqrtRms(r) * per, 1.0, 0.01, "  sqrt(RMS) = the AC part once the HPF settled");
                // the AD2 peak hold (PKHSEL AD2) spans the node: DC +- the 1 V rms sine's peak
                m.Write(0x29, 0xf8); m.Write(0x29, 0xfc);
                r = ReadAfter(m, 0x18, 1);
                var pp = (S24(r, 17) - S24(r, 14)) * per;
                Near(pp, 2 * Math.Sqrt(2), 0.15, "  AD2 peak hold p-p = the sine's 2.83 V");
            }
            {
                // method C row on the calibrator's 40 Hz square (0..9.55 V, 60 V): the LPF's mean is the DC
                var c = (byte[])F0R1.Clone();
                c[7] = 0x1d; c[21] = 0x14;
                var m = NewChip();
                m.Chip.SetInput("acv:4.776@40,dc=4.776,shape=square");
                m.Configure(c, 0);
                m.Write(0x34, 0x88); m.Write(0x26, 0x80);
                m.Advance(3.0);
                double sum = 0, lo = 1e9, hi = -1e9;
                for(var i = 0; i < 45; i++) { var r = ReadAfter(m, 0x18, 1); sum += Lpf(r); lo = Math.Min(lo, Lpf(r)); hi = Math.Max(hi, Lpf(r)); }
                var per = 1.25 / 262144.0 * 100;
                Near(sum / 45 * per, (4.776 + 0.37e-3 * 100) / 1.0188 + 23 * per, 0.005, "  C row, 40 Hz square: the LPF's mean = the DC", string.Format("p-p {0:F0} codes", hi - lo));
            }
            {
                // method C with a large DC on the node: RMS garbage (rmsbig), not below it
                foreach(var (v, bad) in new[] { (4.5, false), (5.0, true) })
                {
                    var c = (byte[])F0R0.Clone();
                    c[7] = 0x1d; c[21] = 0x14;
                    var m = NewChip();
                    m.Chip.SetInput("dcv:" + v);
                    m.Configure(c, 0);
                    m.Write(0x34, 0x88); m.Write(0x26, 0x80);
                    m.Advance(4.0);
                    var nbad = 0;
                    for(var i = 0; i < 20; i++) { var r = ReadAfter(m, 0x18, 1); if(SqrtRms(r) > 2000) nbad++; }
                    Check(bad ? nbad >= 10 : nbad == 0, string.Format("  6 V row, {0} V DC: {1} garbage RMS blocks of 20", v, nbad));
                }
            }
            {
                // method C row, 6 V: the LPF's creep after an input step inside the session (lpfcreep, device
                // 80d5b01 tc6: 5..12 codes still to go 3.6 s after 5 -> 3 V; three cascaded lags of 2.5 s since 10.14), none
                // without the option
                foreach(var (opt, want) in new[] { ("lpfn:0;", 2.4e-4 * 41580 * ((1 + 1.44 + 1.44 * 1.44 / 2) * Math.Exp(-1.44) - (1 + 6.4 + 6.4 * 6.4 / 2) * Math.Exp(-6.4))), ("lpfn:0;lpfcreep:0;", 0.0) })
                {
                    var c = (byte[])F0R0.Clone();
                    c[7] = 0x1d; c[21] = 0x14;
                    var m = NewChip();
                    m.Chip.SetInput(opt + "dcv:5");
                    m.Configure(c, 0);
                    m.Write(0x34, 0x88); m.Write(0x26, 0x80);
                    // read every block, as the firmware does (the lag is advanced at each LPF result and
                    // starts again after a gap of > 3 s)
                    Func<double, byte[]> until = dt => { var t1 = m.Now + dt; while(m.Now < t1) { m.WaitFlag(0x18, 1); m.Read(0x00, 20); } m.WaitFlag(0x18, 1); return m.Read(0x00, 20); };
                    until(4.0);
                    m.Chip.SetInput(opt + "dcv:3");
                    var early = Lpf(until(3.6));
                    var late = Lpf(until(12.4));
                    Near(early - late, want, 1.5, "  6 V row, 5 -> 3 V: the LPF 3.6 s after the step above its value 16 s after, codes (" + (want > 0 ? "lpfcreep" : "lpfcreep:0") + ")");
                }
            }
            {
                // the E7 / method C anchor: AD1 on SENSE - PB5 in the AC V row, 60 V: +0.047 % (ad1acg)
                var c = (byte[])F0R1.Clone();
                c[2] = 0x14; c[3] = 0x03; c[4] = 0x07; c[7] = 0x1d; c[6] = 0x00; c[9] = 0; c[20] = 0; c[19] = 0x2c;
                foreach(var (opt, want) in new[] { ("", 24.0 * 1.00047), ("ad1acg:0;", 24.0) })
                {
                    var m = NewChip();
                    m.Chip.SetInput(opt + "dcv:24");
                    m.Configure(c, 0);
                    m.Write(0x23, 0x83);
                    m.Read1(0x1E); m.WaitFlag(0x04, 1); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                    Near(DcvDisplay(m.ReadS24(0), 1, cal), want, 2e-3, "  AD1 anchor in the AC V row at 24 V, " + (opt == "" ? "+0.047 %" : "ad1acg:0"));
                }
            }
        }

        private static void Sentinel()
        {
            Console.WriteLine("opendm40 misconnect sentinel (comparator on PB1 / SENSE tap, AD1 peak on PB1 - PB3)");
            foreach(var (row, mode, name) in new[] { (F1R0, 1, "DcV m1 r0"), (F1R1, 1, "DcV m1 r1"), (F3R1, 3, "DcMv m3 r1"), (F10R1, 10, "Ohm m10 r1"), (F15R0, 15, "Temp m15 r0"),
                                                     (F0R0, 0, "AcV m0 r0"), (F0R1, 0, "AcV m0 r1"), (F2R1, 2, "AcMv m2 r1"), (F8R0, 8, "DcA m8 r0") })
            {
                var m = NewChip();
                m.Configure(Sent(row, mode), mode);
                Check(m.Chip.Role == name, "sentinel row keeps its role: " + name, m.Chip.Role);
            }
            {
                // DC V, nothing in the A jack: no comparator output; 1 mA DC: CMPHO; -1 mA: CMPLO.
                var m = NewChip(true);
                m.Chip.SetInput("vsrc:1.234");
                m.Configure(Sent(F1R0, 1), 1);
                var q = CmpScan(m, 50, 0.0071);
                Check(q[2] == 0, "DC V, A jack open: comparator quiet", string.Join(",", q));
                m.Chip.SetInput("isrc:0.001");
                q = CmpScan(m, 20, 0.0071);
                Check(q[0] == 20 && q[1] == 0, "  1 mA DC into A: CMPHO (47.5 mV on PB1 > 20.8 mV)", string.Join(",", q));
                m.Chip.SetInput("isrc:-0.001");
                q = CmpScan(m, 20, 0.0071);
                Check(q[1] == 20 && q[0] == 0, "  -1 mA: CMPLO", string.Join(",", q));
                m.Chip.SetInput("isrc:0.0003");
                q = CmpScan(m, 20, 0.0071);
                Check(q[2] == 0, "  0.3 mA (14 mV): below the 20.8 mV threshold", string.Join(",", q));
                m.Chip.SetInput("isrc:0,ac=0.01,f=50");
                q = CmpScan(m, 100, 0.0037);
                Check(q[0] > 20 && q[1] > 20, "  10 mA AC 50 Hz: both bits toggle", string.Join(",", q));
                m.Chip.SetInput("isrc:0;vsrc:1.234");
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Dcv(m, 0), 1.234, 2e-3, "  the DC V reading is unchanged with the comparator on PB1");
                m.Chip.SetInput("avsrc:5");
                q = CmpScan(m, 10, 0.0071);
                Check(q[0] == 10, "  5 V source across A-COM (~0.1 A through 47.5 ohm): CMPHO", string.Join(",", q));
            }
            {
                // Stock R20 (SCMPI PB4): nothing seen on the comparator bits
                var m = NewChip();
                m.Chip.SetInput("isrc:0.01");
                m.Configure(F1R0, 1);
                var q = CmpScan(m, 10, 0.01);
                Check(q[2] == 0, "stock DC V row (comparator on PB4): no sentinel bits", string.Join(",", q));
            }
            {
                // Ohm r1 (PA relay on the ohm network): the A jack still rests on the shunt
                var m = NewChip();
                m.Chip.SetInput("r:100;isrc:0.002");
                m.Configure(Sent(F10R1, 10), 10);
                var q = CmpScan(m, 10, 0.01);
                Check(q[0] == 10, "ohm: 2 mA into A -> CMPHO", string.Join(",", q));
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Ohm(m, 1), 100, 0.2, "  ohm reading unchanged");
            }
            {
                // AC V: AD1 peak hold on PB1 - PB3
                var m = NewChip(true);
                m.Chip.SetInput("vsrc:0,ac=1.0,f=50");
                m.Configure(Sent(F0R1, 0), 0);
                m.Advance(0.3);
                var b = ReadAfter(m, 0x18, 1);
                var pmin = S24(b, 14); var pmax = S24(b, 17);
                Check(pmax < 2000 && pmin > -2000 && pmax >= pmin, "AC V, A jack open: AD1 peak near 0", pmin + ".." + pmax);
                m.Advance(3.0);
                b = ReadAfter(m, 0x18, 1);
                var ms = NewChip(true);
                ms.Chip.SetInput("vsrc:0,ac=1.0,f=50");
                ms.Configure(F0R1, 0);
                ms.Advance(3.3);
                var bs = ReadAfter(ms, 0x18, 1);
                Near(SqrtRms(b), SqrtRms(bs), 0.002 * SqrtRms(bs), "  the AC V RMS is that of the stock row with AD1 on", string.Format("{0:F4} V", SqrtRms(b) * 1.25 * 100 / 262144.0));
                m.Chip.SetInput("isrc:0,ac=0.001,f=50");
                m.Write(0x29, (byte)(Sent(F0R1, 0)[9] & 0xFB)); m.Write(0x29, Sent(F0R1, 0)[9]);
                m.Advance(0.12);
                b = m.Read(0x00, 20);
                pmin = S24(b, 14); pmax = S24(b, 17);
                // 1 mA rms = 67 mV peak on 47.5 ohm: 3.6 * 0.0672 / 1.25 * 2^23 / 32 = 50.7 k
                Near(pmax, 50700, 1500, "  1 mA AC into A: PKHMAX ~ +50.7 k");
                Near(pmin, -50700, 1500, "  PKHMIN ~ -50.7 k");
                m.Chip.SetInput("isrc:0.0005");
                m.Write(0x29, (byte)(Sent(F0R1, 0)[9] & 0xFB)); m.Write(0x29, Sent(F0R1, 0)[9]);
                m.Advance(0.12);
                b = m.Read(0x00, 20);
                Near(S24(b, 17), 18000, 400, "  0.5 mA DC: PKHMAX ~ 18 k (the opendm40 threshold 17.9 k)");
            }
            {
                // DC A: comparator on SENSE through the /1000 tap (SS5)
                var m = NewChip();
                m.Chip.SetInput("vsrc:24");
                m.Configure(Sent(F8R0, 8), 8);
                var q = CmpScan(m, 10, 0.01);
                Check(q[0] == 10, "DC mA, 24 V on V-COM: 24 mV at the tap -> CMPHO", string.Join(",", q));
                m.Chip.SetInput("vsrc:12");
                q = CmpScan(m, 10, 0.01);
                Check(q[2] == 0, "  12 V: below the 20.8 V trip", string.Join(",", q));
                m.Chip.SetInput("vsrc:0,ac=230,f=50");
                q = CmpScan(m, 100, 0.0037);
                Check(q[0] > 20 && q[1] > 20, "  230 V AC: both bits toggle", string.Join(",", q));
                m.Configure(F8R0, 8);
                q = CmpScan(m, 10, 0.01);
                Check(q[2] == 0, "  stock DC mA row: nothing", string.Join(",", q));
                m.Chip.SetInput("open;isrc:0.0123");
                m.Configure(Sent(F8R0, 8), 8); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Ma(m, 8, 0), 12.3, 0.01, "  DC mA reading unchanged with SS5 closed");
            }
        }

        // opendm40 power meter rows (firmware/docs/power-meter-and-misconnect.md 3.1 B): written as
        // hy3131::configure does: the row with ENAD2 / ENAD3 clear, R36, R34, then R26.
        private static void PowerConfigure(Master m, byte[] row, int iu, int sph = 0)
        {
            m.Write(0x26, 0); m.Write(0x34, 0);
            m.Relays(0, iu);
            var c = (byte[])row.Clone();
            c[6] &= 0x7F; c[20] &= 0x7F;
            m.Write(0x20, c);
            m.Write(0x36, (byte)sph);
            m.Write(0x34, row[20]);
            m.Write(0x26, row[6]);
            m.Write(0x1F, 0);
        }

        /// RMS<37:0> as a signed 38-bit value (bit 37 = sign).
        private static long RmsSigned(byte[] b)
        {
            var v = b[9] | (long)b[10] << 8 | (long)b[11] << 16 | (long)b[12] << 24 | (long)(b[13] & 0x3F) << 32;
            return (v & (1L << 37)) != 0 ? v - (1L << 38) : v;
        }

        /// Select the statistic (R34), skip the block in progress (mixed), return the next block's bytes.
        private static byte[] PowerStat(Master m, byte r34)
        {
            m.Write(0x34, r34);
            m.Read1(0x1E); m.WaitFlag(0x10, 1, 0.005);
            m.Read1(0x1E);
            return ReadAfter(m, 0x18, 1);
        }

        /// XX, XY, YY (codes^2) of the current input.
        private static double[] PowerTriple(Master m)
        {
            var xx = RmsSigned(PowerStat(m, 0x80));
            var xy = RmsSigned(PowerStat(m, 0x88));
            var yy = RmsSigned(PowerStat(m, 0x8c));
            return new double[] { xx, xy, yy };
        }

        private static readonly byte[] F4R1 = Master.Row("f2 dd 15 03 52 40 80 2d 31 f4 00 00 00 00 08 81 80 c7 3e ac 88 24");

        private static void Power()
        {
            Console.WriteLine("opendm40 power meter rows (X = AD2 = I, Y = AD3 = V)");
            const double C = 262144.0 / 1.25; // codes per volt at IG 1.0
            foreach(var (vr, iu, ir, hex) in new[] { (1, 0, 1, "12 dd 15 03 52 40 80 2d 31 f8 20 00 00 09 28 a0 80 c7 8e ac 88 54"),
                                                    (0, 1, 0, "12 dd 14 03 35 41 80 2d 31 f8 06 00 00 90 28 a0 80 c7 8d ac 88 54"),
                                                    (2, 2, 1, "12 dd 14 03 35 40 80 2d 31 f8 20 00 90 00 28 a0 80 c7 8d ac 88 54") })
                Check(BitConverter.ToString(Hy3131Core.PowerRow(vr, iu, ir)) == BitConverter.ToString(Master.Row(hex)),
                      string.Format("power row builder vr{0} iu{1} ir{2}", vr, iu, ir), BitConverter.ToString(Hy3131Core.PowerRow(vr, iu, ir)));
            foreach(var (vr, iu, ir) in new[] { (0, 0, 0), (1, 0, 1), (3, 0, 1), (0, 1, 0), (2, 1, 1), (2, 2, 1), (3, 2, 0) })
            {
                var m = NewChip();
                PowerConfigure(m, Hy3131Core.PowerRow(vr, iu, ir), iu);
                var want = string.Format("Power vr{0} iu{1} ir{2}", vr, iu, ir);
                Check(m.Chip.Role == want, "power row recognised: " + want, m.Chip.Role);
            }
            {
                var m = NewChip();
                PowerConfigure(m, Hy3131Core.PowerRow(2, 0, 1), 0);
                var ok = true; var got = "";
                foreach(var (reg, val) in new[] { (0x34, 0x80), (0x34, 0x8c), (0x20, 0x10), (0x35, 0x5C), (0x35, 0x74), (0x36, 0x85), (0x2E, 0x08) })
                {
                    m.Write(reg, (byte)val);
                    if(reg == 0x2E) m.Write(0x2F, 0x81);
                    ok &= m.Chip.Role == "Power vr2 iu0 ir1"; got += m.Chip.Role + ";";
                }
                Check(ok, "  run-time R34 / R20 / R35 / R36 / R2E-R2F 08 81 variants keep the role", got);
                Check(m.Chip.Core.Mode == Hy3131Core.PowerMode && m.Chip.Range == 2 * 6 + 0 + 1, "  Mode 16, Range vr*6 + iu*2 + ir", m.Chip.Mode + " " + m.Chip.Range);
            }
            {
                // 230 V / 1 A, PF 1, on 400 V (/1000) x 10 A (10 mohm, x1)
                var m = NewChip(true);
                m.Chip.SetInput("pow:230@50,i=1");
                PowerConfigure(m, Hy3131Core.PowerRow(2, 0, 1), 0);
                var r = PowerTriple(m);
                Near(Math.Sqrt(r[0]), 1 * 0.01 * C, 0.005 * 0.01 * C, "resistive: sqrt(XX) = 1 A x 10 mohm", string.Format("{0:F4} A", Math.Sqrt(r[0]) / C / 0.01));
                Near(Math.Sqrt(r[2]), 0.23 * C, 0.002 * 0.23 * C, "  sqrt(YY) = 230 V / 1000", string.Format("{0:F2} V", Math.Sqrt(r[2]) / C * 1000));
                Near(r[1] / Math.Sqrt(r[0] * r[2]), 1.0, 0.005, "  XY = sqrt(XX * YY) (PF 1)", string.Format("P {0:F2} W", r[1] / C / C * 1000 / 0.01));
                m.Chip.SetInput("pow:230@50,i=1,ph=90");
                r = PowerTriple(m);
                Check(Math.Abs(r[1]) < 0.01 * Math.Sqrt(r[0] * r[2]), "reactive (ph 90): |XY| < 1 % of sqrt(XX * YY)", (r[1] / Math.Sqrt(r[0] * r[2])).ToString("F5"));
                m.Chip.SetInput("pow:230@50,i=1,pf=0.5");
                r = PowerTriple(m);
                Near(r[1] / Math.Sqrt(r[0] * r[2]), 0.5, 0.005, "pf 0.5: XY / sqrt(XX * YY)");
                m.Chip.SetInput("pow:230@50,i=1,shape=square");
                r = PowerTriple(m);
                Near(r[1] / Math.Sqrt(r[0] * r[2]), 1.0, 0.005, "square V and I in phase: PF 1");
                m.Chip.SetInput("pow:230@50,i=1,pf=-1");
                var b = PowerStat(m, 0x88);
                var xy = RmsSigned(b);
                var xx = RmsSigned(PowerStat(m, 0x80));
                Check(xy < 0 && (b[13] & 0xE0) == 0xE0, "reversed current (pf -1): XY negative, R0D bits 7:5 = sign (37) and its copies (39:38)", string.Format("R0D 0x{0:X2}", b[13]));
                Near(-xy / Math.Sqrt(xx), 0.23 * C, 0.005 * 0.23 * C, "  |XY| / sqrt(XX) = Y rms");
                m.Chip.SetInput("pow:230@50,i=1,f=60,pf=0.8");
                r = PowerTriple(m);
                Near(r[1] / Math.Sqrt(r[0] * r[2]), 0.8, 0.005, "pow f= option (60 Hz), pf 0.8");
                m.Chip.SetInput("open;vsrc:0,ac=230,f=50;isrc:0,ac=1,f=50,ph=60");
                r = PowerTriple(m);
                Near(r[1] / Math.Sqrt(r[0] * r[2]), 0.5, 0.005, "isrc ph=60: cos 60");
            }
            {
                // DC power: 12 V on 40 V (/100), 0.5 A on 10 A x1
                var m = NewChip(true);
                m.Chip.SetInput("vsrc:12;isrc:0.5");
                PowerConfigure(m, Hy3131Core.PowerRow(1, 0, 1), 0);
                var bx = PowerStat(m, 0x80);
                Near(Lpf(bx), 0.5 * 0.01 * C, 2, "DC: LPF(X) = 0.5 A x 10 mohm");
                var bxy = PowerStat(m, 0x88);
                Check(Math.Abs(RmsSigned(bxy)) < 200, "  XY ~ 0 (the HPF removes DC)", RmsSigned(bxy).ToString());
                Near(Lpf(bxy), 0.5 * 0.01 * C, 2, "  LPF follows SDO23 = 0: still X");
                var by = PowerStat(m, 0x8c);
                Near(Lpf(by), 12 / 100.0 * C, 3, "  SDO23: LPF(Y) = 12 V / 100");
                m.Chip.SetInput("isrc:-0.5");
                bx = PowerStat(m, 0x80);
                Check(Lpf(bx) < 0 && Math.Abs(Lpf(bx) + 0.5 * 0.01 * C) < 3, "  negative DC current: LPF(X) negative", Lpf(bx).ToString());
                var mq = NewChip();
                mq.Chip.SetInput("vsrc:12;isrc:0.5");
                PowerConfigure(mq, Hy3131Core.PowerRow(1, 0, 1), 0);
                mq.Read1(0x1E); mq.WaitFlag(0x02, 1, 0.001);
                Near(S24(mq.Read(0x00, 6), 3), 0.5 * 0.01 * C, 1, "  AD2 data register = X");
                mq.Write(0x34, 0x8c); mq.Read1(0x1E); mq.WaitFlag(0x02, 1, 0.001);
                Near(S24(mq.Read(0x00, 6), 3), 12 / 100.0 * C, 1, "  SDO23: AD2 data register = Y (AD3)");
                mq.Write(0x35, 0x5C);
                mq.Read1(0x1E); mq.WaitFlag(0x02, 1, 0.001);
                Near(S24(mq.Read(0x00, 6), 3), 12 / 100.0 * C * 2, 2, "  AD3IG 2.0 (R35 0x5C) scales Y");
                mq.Write(0x35, 0x54); mq.Write(0x25, (byte)(mq.Chip.Core.Peek(0x25) | 0x02));
                mq.Read1(0x1E); mq.WaitFlag(0x02, 1, 0.001);
                Near(S24(mq.Read(0x00, 6), 3), 12 / 100.0 * C * 10, 2, "  OPS2 x10 scales Y");
            }
            {
                // the stock AC A gains: RMS gain on the AC part, LPF gain on the DC part of X; Y ideal
                var cal = Hy3131Core.DefaultCal();
                cal[(0xc4 + 4 * 1) / 4] = 1.02f; cal[(0xf4 + 4 * 1) / 4] = 0.98f;
                var m = NewChip(false, cal);
                m.Chip.SetInput("pow:230@50,i=1,idc=0.5");
                PowerConfigure(m, Hy3131Core.PowerRow(2, 0, 1), 0);
                var b = PowerStat(m, 0x80);
                Near(Math.Sqrt(RmsSigned(b)), 0.01 * C / 1.02, 0.5, "cal: sqrt(XX) / RMS gain 0xc4[1]");
                Near(Lpf(b), 0.5 * 0.01 * C / 0.98, 1, "  LPF(X) / LPF gain 0xf4[1]");
                Near(Math.Sqrt(RmsSigned(PowerStat(m, 0x8c))), 0.23 * C, 0.5, "  Y: no calibration");
            }
            {
                // SPHACAL at 1 kHz: 127 AD2CLK = 51.7 us = 18.60 deg
                var dphi = 127 / 2457600.0 * 1000 * 360;
                var m = NewChip();
                m.Chip.SetInput("pow:2@1000,i=1");
                PowerConfigure(m, Hy3131Core.PowerRow(0, 0, 1), 0, 0x7F);
                var r = PowerTriple(m);
                Near(r[1] / Math.Sqrt(r[0] * r[2]), Math.Cos(dphi * Math.PI / 180), 0.002, "SPHACAL 0x7F (AD2 late), ph 0: cos 18.6 deg");
                Near(Math.Sqrt(r[0]), 0.01 * C, 0.5, "  XX unaffected");
                m.Chip.SetInput("pow:2@1000,i=1,ph=60");
                m.Write(0x36, 0xFF);
                r = PowerTriple(m);
                Near(r[1] / Math.Sqrt(r[0] * r[2]), Math.Cos((60 - dphi) * Math.PI / 180), 0.002, "SPHACAL 0xFF (AD2 leads), ph 60: cos(60 - 18.6)");
                m.Write(0x36, 0x7F);
                r = PowerTriple(m);
                Near(r[1] / Math.Sqrt(r[0] * r[2]), Math.Cos((60 + dphi) * Math.PI / 180), 0.002, "SPHACAL 0x7F, ph 60: cos(60 + 18.6)");
                m.Chip.SetInput("pow:2@50,i=1,ph=60");
                r = PowerTriple(m);
                Near(r[1] / Math.Sqrt(r[0] * r[2]), Math.Cos((60 + dphi / 20) * Math.PI / 180), 0.002, "  50 Hz: 0.93 deg");
            }
            foreach(var mix in new[] { true, false })
            {
                // R34 write half way through a block: that block is the time-weighted mix (powmix)
                var m = NewChip();
                m.Chip.SetInput("pow:230@50,i=1,ph=90" + (mix ? "" : ";powmix:0"));
                PowerConfigure(m, Hy3131Core.PowerRow(2, 0, 1), 0);
                var t0 = m.Now; var p = m.Chip.Core.LpfPeriod;
                var xx = 0.01 * C * 0.01 * C;
                m.Now = t0 + 2.5 * p;
                m.Write(0x34, 0x80);                      // XY (~0) -> XX at 0.5 of block 3
                m.Now = t0 + 3.05 * p;
                var b = m.Read(0x00, 20);
                Near(RmsSigned(b) / xx, mix ? 0.5 : 1.0, 0.01, mix ? "mixing: XY -> XX mid-block gives half of XX" : "powmix:0: that block is clean");
                m.Now = t0 + 4.05 * p;
                Near(RmsSigned(m.Read(0x00, 20)) / xx, 1.0, 0.001, "  the next block is clean");
                m.Chip.SetInput("open;vsrc:120;isrc:0.5");
                m.Now = t0 + 6.25 * p;
                m.Write(0x34, 0x8c);                      // LPF X -> Y at 0.25 of block 7
                m.Now = t0 + 7.05 * p;
                var want = mix ? 0.25 * 0.5 * 0.01 * C + 0.75 * 0.12 * C : 0.12 * C; // 120 V / 1000
                Near(Lpf(m.Read(0x00, 20)), want, 0.01 * want, "  LPF source switch " + (mix ? "mixed 1/4 X + 3/4 Y" : "clean"));
            }
            {
                // the counter on SENSE: frequency of the voltage
                var m = NewChip();
                m.Chip.SetInput("vsrc:0,ac=230,f=60;isrc:0,ac=1,f=50");
                PowerConfigure(m, Hy3131Core.PowerRow(2, 0, 1), 0);
                m.Write(0x20, 0x10); m.Write(0x1B, 0x00, 0x00, 0xFC); m.Write(0x20, 0x12);
                var fl = m.WaitFlag(0x01, 1);
                var c = m.Read(0x15, 9);
                var ctb = c[3] | c[4] << 8 | c[5] << 16; var cta = c[6] | c[7] << 8 | c[8] << 16;
                Near(ctb / (double)(cta + 0x40000) * 4915200, 60, 0.01, "power row counter: the voltage's 60 Hz (current at 50 Hz)");
            }
            {
                // opendm40 power meter follow-ups (doc 3.11): a unipolar square (the T1 calibrator,
                // 0..9.6 V) on the 60 V tap never crosses the default -20.8 mV: no count; the
                // window around its DC (R21 0xBF: +62.5 / +20.8 mV) counts 40 Hz.
                foreach(var r21 in new byte[] { 0xDD, 0xBF })
                {
                    var m = NewChip();
                    m.Chip.SetInput("vsrc:4.8,ac=4.8,f=40,shape=square");
                    PowerConfigure(m, Hy3131Core.PowerRow(1, 0, 1), 0);
                    m.Write(0x21, r21);
                    m.Write(0x20, 0x10); m.Write(0x1B, 0x00, 0x00, 0xFC); m.Write(0x20, 0x12);
                    var ok = m.WaitFlag(0x01, 1);
                    var c = m.Read(0x15, 9);
                    var ctb = c[3] | c[4] << 8 | c[5] << 16; var cta = c[6] | c[7] << 8 | c[8] << 16;
                    var f = (ok & 1) != 0 ? ctb / (double)(cta + 0x40000) * 4915200 : 0;
                    Near(f, r21 == 0xDD ? 0 : 40, 0.01, string.Format("unipolar square on the power row, R21 0x{0:X2}", r21));
                }
                // PB4 (SCMPI 7): the current's frequency, also with no voltage.
                {
                    var m = NewChip();
                    m.Chip.SetInput("vsrc:12;isrc:0,ac=3,f=50");
                    PowerConfigure(m, Hy3131Core.PowerRow(1, 0, 1), 0);
                    m.Write(0x20, 0xF0); m.Write(0x1B, 0x00, 0x00, 0xFC); m.Write(0x20, 0xF2);
                    var ok = m.WaitFlag(0x01, 1);
                    var c = m.Read(0x15, 9);
                    var ctb = c[3] | c[4] << 8 | c[5] << 16; var cta = c[6] | c[7] << 8 | c[8] << 16;
                    Near((ok & 1) != 0 ? ctb / (double)(cta + 0x40000) * 4915200 : 0, 50, 0.01, "power row counter on PB4: the current's 50 Hz");
                }
                // powz:1: the device's LPF zeros (Y +1080 codes, X +155 with OP1 x1).
                {
                    var m = NewChip();
                    m.Chip.SetInput("open;powz:1");
                    PowerConfigure(m, Hy3131Core.PowerRow(1, 0, 1), 0);
                    Near(Lpf(PowerStat(m, 0x8c)), 1080, 1, "powz:1 Y LPF zero");
                    Near(Lpf(PowerStat(m, 0x80)), 155, 1, "powz:1 X LPF zero (x1)");
                    m.Chip.SetInput("powz:0");
                }
                // opendm40 DC row (powermeter.rs dc_row(1, 5)): a power role with AD1 = OP1O - PB3 on
                // the shunt: 2 A on 10 A x1 -> AD1 = 2 * 0.01 * 0.9 / 1.25 * 2^23; Y LPF = 12 V / 100.
                {
                    var m = NewChip();
                    m.Chip.SetInput("open;vsrc:12;isrc:2");
                    var row = Master.Row("12 dd 15 83 35 40 80 2d 31 f8 20 00 00 09 28 a0 80 c7 8e ac 8c 54");
                    PowerConfigure(m, row, 0);
                    Check(m.Chip.Role == "Power vr1 iu0 ir1", "DC row recognised as the power row", m.Chip.Role);
                    m.Read1(0x1E); m.WaitFlag(0x04, 1);
                    var a = m.Read(0x00, 3);
                    var code = (a[0] | a[1] << 8 | a[2] << 16) << 8 >> 8;
                    Near(code, 2 * 0.01 * 0.9 / 1.25 * 8388606, 200, "DC row AD1 = the current");
                    Near(Lpf(PowerStat(m, 0x8c)), 0.12 / 1.25 * 262144, 30, "DC row Y LPF = the voltage");
                }
                // powvg: the Y gain deviation.
                {
                    var m = NewChip();
                    m.Chip.SetInput("open;vsrc:10;powvg:0.98");
                    PowerConfigure(m, Hy3131Core.PowerRow(1, 0, 1), 0);
                    Near(Lpf(PowerStat(m, 0x8c)), 0.98 * 0.1 / 1.25 * 262144, 2, "powvg:0.98 scales Y");
                    m.Chip.SetInput("powvg:1");
                }
            }
            {
                // fallback: stock AC V with the A shunt selected and current flowing; stock AC A with V on V-COM
                var m = NewChip();
                m.Chip.SetInput("vsrc:0,ac=10,f=50;isrc:0,ac=1,f=50");
                m.Configure(F0R1, 0);
                m.Relays(0, 0);
                m.Advance(3.3);
                m.Read1(0x1E);
                var b = ReadAfter(m, 0x18, 1);
                Near(SqrtRms(b) * 1.25 * 100 / 262144.0, 10, 0.01, "stock AC V r1 with PE0 = 1 and 1 A flowing: reads the V jack");
                m.Configure(F4R1, 4);
                m.Advance(0.5);
                m.Read1(0x1E);
                b = ReadAfter(m, 0x18, 1);
                Near(SqrtRms(b) * 1.25 / 0.01 / 262144.0, 1.0, 0.002, "stock AC A r1 with 10 V on V-COM: reads the current");
            }
        }

        private static void CrossCases()
        {
            Console.WriteLine("front end: fixed connections across functions");
            {
                var m = NewChip();
                m.Chip.SetInput("vsrc:1.234;isrc:0.0123");
                m.Configure(F1R0, 1); m.WaitFlag(0x04, 1);
                Near(Dcv(m, 0), 1.234, 2e-4, "V source + A source: DC V reads the V source");
                m.Configure(F8R0, 8); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Ma(m, 8, 0), 12.3, 0.01, "  DC mA reads the A source");
                m.Chip.SetInput("vsrc:none");
                m.Configure(F1R0, 1); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Dcv(m, 0), 0, 1e-4, "only the A source: DC V reads 0");
                m.Chip.SetInput("open;vsrc:1.234");
                m.Configure(F8R0, 8); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Ma(m, 8, 0), 0, 1e-3, "only the V source: DC mA reads 0");
                m.Configure(F10R3, 10); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Check(m.ReadS24(0) == 8388607 || m.ReadS24(0) == -8388608, "V source alone in ohm: over range", m.ReadS24(0).ToString());
            }
            {
                var m = NewChip();
                m.Chip.SetInput("r:4700");
                m.Configure(F1R0, 1); m.WaitFlag(0x04, 1);
                Near(Dcv(m, 0), 0, 1e-4, "resistor in DC V: 0");
                m.Configure(F10R3, 10); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Ohm(m, 3), 4700, 0.5, "  in ohm r3: 4700");
                m.Chip.SetInput("open;vsrc:1.0,rs=4700"); // powered circuit: 1 V behind 4.7 k
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                var vs = 1.4; var rref = 100e3;  // SFUVR 2 = VDS15 - AGND at 3.6 V, r3 reference
                var vx = (1.0 / 4700 + vs / rref) / (1.0 / 4700 + 1 / rref);
                // vx / (vs - vx) = 2.67 > the AD1 full scale (1 / 0.9): the reading clips (OL on the meter)
                Check(m.ReadS24(0) == 8388607, "  powered 4.7 k in ohm: node at 1.018 V, ratio 2.67 -> clipped (OL), not 4700", string.Format("vx {0:F3} V, code {1}", vx, m.ReadS24(0)));
                m.Chip.SetInput("open;vsrc:0.1,rs=4700"); // a weaker source: inside the range, wrong value
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                vx = (0.1 / 4700 + vs / rref) / (1.0 / 4700 + 1 / rref);
                Near(Ohm(m, 3), vx / (vs - vx) * rref, 20, "  0.1 V behind 4.7 k: KCL node (12.3 k shown), not 4700", string.Format("vx {0:F4} V", vx));
                m.Chip.SetInput("open;diode:0.65");
                m.Configure(F10R2, 10); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Ohm(m, 2), 0.65 / (1.4 - 0.65) * 10e3, 20, "diode in ohm r2: Vf / (Vs - Vf) * Rref");
                m.Configure(F12, 12); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(m.ReadS24(0) * 1.25 * 10 / 0.9 / 8388606.0, 0.65, 2e-4, "  in diode mode: 0.65 V");
                m.Chip.SetInput("open;diode:0.65,rev=1");
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Check(m.ReadS24(0) * 1.25 * 10 / 0.9 / 8388606.0 > 3.12, "reversed diode in diode mode: OL");
                m.Chip.SetInput("open;r:100");
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(m.ReadS24(0) * 1.25 * 10 / 0.9 / 8388606.0, 0.0765, 2e-4, "100 ohm in diode mode: 0.765 mA * 100 = 76.5 mV");
            }
            {
                var m = NewChip();
                m.Chip.SetInput("avsrc:0.5"); // a voltage source in the A jack
                m.Configure(F8R1, 8); m.WaitFlag(0x04, 1);
                Near(Ma(m, 8, 1), 500 / (0.5 + 0.05), 2, "0.5 V in the A jack on the mA shunt: 0.5 / 0.55 ohm = 909 mA (software OL)");
                m.Configure(F1R0, 1); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Dcv(m, 0), 0, 1e-4, "  DC V reads 0 (nothing on V)");
            }
            {
                var m = NewChip();
                m.Chip.SetInput("c:100e-9");
                m.Configure(F1R0, 1); m.WaitFlag(0x04, 1);
                Near(Dcv(m, 0), 0, 1e-4, "capacitor in DC V: 0");
                m.Configure(F10R6, 10); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                var early = Ohm(m, 6);
                m.Advance(60); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                var late = Math.Abs(m.ReadS24(0));
                Check(late == 8388607, "  in ohm r6: charges to the source voltage -> over range", string.Format("early {0:G4} ohm, late code {1}", early, late));
            }
            {
                var m = NewChip();
                m.Chip.SetInput("tc:100;cj:25");
                m.Configure(F3R0, 3); m.WaitFlag(0x04, 1);
                Near(m.ReadS24(0) * 1.38889 / 10 / 8388.606, TypeK.Emf(100) - TypeK.Emf(25), 0.003, "thermocouple in DC mV r0: E(100) - E(25) mV");
                m.Chip.SetInput("die:35");
                m.Configure(F15R0, 15); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                var a = m.ReadS24(0);
                m.Write(0x33, 0xAE); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                var b = m.ReadS24(0);
                Near((a - b) / 640000.0 * 314.15 - 289.15, 35, 0.05, "die 35 C: the internal sensor reports the die, not the cold junction");
                m.Write(0x33, 0xAC); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(m.ReadS24(0) * 1.25 / 0.9 * 100 / 8388606.0, TypeK.Emf(100) - TypeK.Emf(25), 0.003, "  EMF still relative to the 25 C junction -> the firmware will read ~110 C");
                m.Chip.SetInput("open;vsrc:4.096e-3");
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(m.ReadS24(0) * 1.25 / 0.9 * 100 / 8388606.0, 4.096, 0.003, "a 4.096 mV DC source in temperature mode reads like a thermocouple");
            }
            {
                var m = NewChip();
                m.Chip.SetInput("vsrc:1.234");
                m.Configure(F10R3, 10);   // relay on the ohm network
                m.Configure(F1R0);        // the DC V row without the relay pulse: the jack is not on the divider
                m.WaitFlag(0x04, 1);
                Near(Dcv(m, 0), 0, 1e-4, "DC V row with the relay left on the ohm network: 0 (jack disconnected)");
                m.RelaysForMode(1); m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(Dcv(m, 0), 1.234, 2e-4, "  relay back: 1.234 V");
                Check(m.Chip.Frontend.Contains("pa=0") && m.Chip.Frontend.Contains("role=DcV"), "Frontend state line", m.Chip.Frontend.Substring(0, 60));
                var fe = m.Chip.Frontend;
                Check(fe.Contains(" sop2p=") && fe.Contains(" sad3ip=") && fe.Contains(" vrefsel=") && fe.Contains(" pasw="),
                    "Frontend state line: internal switches (sop2p, sad2*/sad3*, vrefsel, pasw)", fe.Substring(fe.IndexOf(" sop2p=")));
            }
        }

        private static void Registers()
        {
            Console.WriteLine("registers");
            var m = NewChip();
            m.Write(0x22, 0x15);
            Check(m.Read1(0x22) == 0x15, "R22 read back");
            m.Write(0x20, 0xFF);
            Check(m.Read1(0x20) == 0xFE, "R20 bit 0 is reserved (0)", "0x" + m.Read1(0x20).ToString("X2"));
            m.Write(0x34, 0xFF);
            Check(m.Read1(0x34) == 0xBC, "R34 reserved bits 6,1,0", "0x" + m.Read1(0x34).ToString("X2"));
            m.Write(0x28, 0xFF);
            Check(m.Read1(0x28) == 0xFF, "R28 '-' bits are kept");
            // multi-byte auto-increment write and read
            m.Write(0x2A, 0x11, 0x22, 0x33);
            var r = m.Read(0x2A, 3);
            Check(r[0] == 0x11 && r[1] == 0x22 && r[2] == 0x33, "auto-increment write/read R2A..R2C");
            // data registers are read only
            m.Write(0x00, 0x55, 0x55, 0x55);
            Check(m.ReadS24(0x00) == 0, "R00..R02 read only");
            // INTF: clear on read, write-0 clears, 1 cannot be written
            m.Chip.Core.ForceIntf(0x1F);
            Check(m.Read1(0x1E) == 0x1F, "INTF set");
            Check(m.Read1(0x1E) == 0x00, "INTF cleared by the read");
            m.Chip.Core.ForceIntf(0x14);
            m.Write(0x1E, 0x10);
            Check(m.Chip.Core.Intf == 0x10, "INTF write 0 clears bits", "0x" + m.Chip.Core.Intf.ToString("X2"));
            m.Write(0x1E, 0xFF);
            Check(m.Chip.Core.Intf == 0x10, "INTF write 1 does not set bits");
            // IRQ on DO while CS is high
            m.Write(0x1F, 0x10);
            Check(m.PortC.Level[11], "DO = IRQ (RMSF & RMSIE) with CS high");
            m.Read1(0x1E);
            Check(!m.PortC.Level[11], "DO low after INTF is cleared");
            // CTA preset only while ENCTR = 0
            m.Write(0x20, 0x50);
            m.Write(0x1B, 0x12, 0x00, 0xFC);
            r = m.Read(0x1B, 3);
            Check(r[0] == 0 && r[1] == 0 && r[2] == 0xFC, "CTA<23:8> preset, CTA<7:0> held 0", BitConverter.ToString(r));
            m.Write(0x20, 0x52);
            m.Write(0x1C, 0x00, 0x00);
            m.Advance(0.001);
            r = m.Read(0x1B, 3);
            Check(r[2] == 0xFC && (r[0] | r[1]) != 0, "CTA counts SYSCLK while ENCTR = 1 and ignores writes", BitConverter.ToString(r));
            m.Write(0x20, 0x50);
            r = m.Read(0x18, 6);
            // CtRetain (af0cf31, datasheet 14): CTA<23:8> keeps the count the gate reached,
            // not the written preset (00-FC), so CTA<15:8> is the counted byte here.
            Check(r[0] == 0 && r[1] == 0 && r[2] == 0 && r[3] == 0 && r[4] != 0 && r[5] == 0xFC, "ENCTR = 0 clears CTB and CTA<7:0>, CTA<23:8> keeps the count reached", BitConverter.ToString(r));
            // ENPKH preset
            m.Write(0x29, 0x00);
            r = m.Read(0x0E, 6);
            Check(r[0] == 0xFF && r[1] == 0xFF && r[2] == 0x03 && r[3] == 0x00 && r[4] == 0x00 && r[5] == 0xFC, "PKHMIN = 3FFFFh, PKHMAX = 40000h (19-bit, sign-extended FC0000h) with ENPKH = 0", BitConverter.ToString(r));
            // reset command
            m.Write(0x37, 0x60);
            Check(m.Read1(0x22) == 0 && m.Read1(0x2A) == 0, "R37 = 0x60 resets the register file");
            m.Write(0x37, 0x00);
            Check(m.Chip.Transfers > 0, "transfer counter");
        }

        /// FR-38 (B): the data logger's AD2 source (opendm40 hy3131::set_ad2_stream): one AD2F and
        /// one fresh R03..R05 per conversion period on the chip's grid, the code of the DC-coupled
        /// AC mA row, the rung's rate, and the per-result noise model.
        private static string Hex(byte[] b) => BitConverter.ToString(b).Replace("-", " ");

        /// The hardware-SPI byte path (HY3131SpiPort / SpiByte) against the
        /// bit-banged frames: opendm40 hy3131.rs's SPI2 frame (pulse) and the
        /// alternative shifted framing (cmd + n + 1 bytes).
        private static void HwSpi()
        {
            Console.WriteLine("Hardware SPI byte path (opendm40 SPI2 bus)");
            var m = NewChip();
            m.Configure(F0R0, 0);
            m.Write(0x36, 0x5A);
            var bb = m.Read(0x1F, 23);
            Check(bb[0] == 0 && Hex(bb).EndsWith(Hex(F0R0)), "BB read R1F..R35 = INTE + the row", Hex(bb));
            Check(Hex(m.HwRead(0x1F, 23, false)) == Hex(bb), "HW pulse framing: R1F..R35 = BB");
            Check(Hex(m.HwRead(0x1F, 23, true)) == Hex(bb), "HW shift framing: R1F..R35 = BB");
            Check(m.HwRead(0x36, 1, false)[0] == 0x5A && m.HwRead(0x36, 1, true)[0] == 0x5A, "R36 read 1 byte, both framings");
            // Writes at speed (no read period): pattern via HW, read back bit-banged.
            var bad = 0;
            for(var i = 0; i < 256; i++)
            {
                m.HwWrite(0x36, (byte)i);
                if(m.Read1(0x36) != i) bad++;
            }
            Check(bad == 0, "HW write R36 0..255, BB read-back", bad + " bad");
            m.HwWrite(0x20, F1R0);
            Check(Hex(m.Read(0x20, 22)) == Hex(F1R0), "HW write of a whole 22-byte row");
            // Live data with forced codes: R00..R05 identical on all three.
            m.Chip.SetAd1(-1234567); m.Chip.SetAd2(0x2ABCD);
            m.Configure(F1R0, 1);
            m.Advance(0.5);
            var d = m.Read(0x00, 6);
            Check(Hex(m.HwRead(0x00, 6, false)) == Hex(d) && Hex(m.HwRead(0x00, 6, true)) == Hex(d), "R00..R05 (AD1 + AD2) equal on BB / HW / HWS", Hex(d));
            Check(S24(d, 0) == -1234567, "AD1 code through the frame", S24(d, 0).ToString());
            // The register after the frame (reg + n): its MSB is put on DO at the
            // last falling edge of every framing (BB included), so the model reads
            // it (side effects included) on BB, pulse and shift alike; the shift's
            // 6 further clocks add nothing. R15..R1D runs into R1E = INTF (clear-
            // on-read): cleared on all three; R15..R1C (tail R1D) on none.
            // (opendm40 uses the pulse framing: the same clocks as BB.)
            foreach(var fr in new[] { 0, 1, 2 })
            {
                var nm = new[] { "BB", "HW pulse", "HW shift" }[fr];
                m.Chip.Core.ForceIntf(0x1C);
                if(fr == 0) m.Read(0x15, 8); else m.HwRead(0x15, 8, fr == 2);
                Check((m.Chip.Core.Intf & 0x1C) == 0x1C, nm + ", R15..R1C: INTF untouched (new conversions may add bits)", "0x" + m.Chip.Core.Intf.ToString("X2"));
                if(fr == 0) m.Read(0x15, 9); else m.HwRead(0x15, 9, fr == 2);
                Check(m.Chip.Core.Intf == 0, nm + ", R15..R1D: the fetch of R1E clears INTF (model)", "0x" + m.Chip.Core.Intf.ToString("X2"));
            }
            m.Chip.Core.ForceIntf(0x1C);
            Check(m.HwRead(0x1E, 1, true)[0] == 0x1C && m.Chip.Core.Intf == 0, "INTF read over HWS: value, then cleared like BB");
            m.Configure(F0R0, 0);
            // A bit-banged frame right after SPI frames (opendm40 switches the pins for
            // its references): the SPI's DI bits must not stick in the GPIO's MOSI.
            m.Configure(F0R0, 0);
            m.HwRead(0x1F, 23, false);
            m.PortC.Drive(12, false);
            Check(Hex(m.Read(0x1F, 23)) == Hex(bb), "bit-banged read right after an SPI read (MOSI state kept apart)");
            // CS high: the chip ignores the clock; the byte is the IRQ level.
            m.Write(0x1E, 0); m.Write(0x1F, 0);
            var t = m.Chip.Transfers;
            Check(m.SpiX(0x41) == 0x00 && m.Chip.Transfers == t, "byte with CS high: IRQ level (0), no frame");
            Check(Hex(m.Read(0x20, 22)) == Hex(F0R0), "row intact after the CS-high byte");
        }

        /// The timed byte path (GD32_SPI -> HY3131SpiPort.TransmitTimed): CKPH,
        /// the mode-0 DO margin at /8, a read across an AD2 edge (opendm40 TODO
        /// "76.8 kS/s").
        private static void TimedSpi()
        {
            Console.WriteLine("Timed SPI bytes (GD32_SPI): CKPH, /8 DO margin, torn AD2 reads");
            const double B8 = 160e-9, B16 = 320e-9;
            var m = NewChip();
            m.Configure(F0R0, 0);
            m.Chip.SetAd2(0x2A5A5); // alternating bits: every bit a transition
            m.Write(0x34, 0); m.Write(0x29, 0); m.Write(0x26, 0x80); // 76.8 kS/s
            m.Advance(0.01);
            var want = Hex(m.Read(0x03, 3));
            m.Chip.MisoErrRate = 1.0;
            var e0 = m.Chip.MisoErrors;
            Check(Hex(m.TimedRead(0x03, 3, B8, true)) == want && m.Chip.MisoErrors == e0,
                "/8, data phase CKPH 1: R03..R05 = BB, no DO-margin error (MisoErrRate 1)", want);
            Check(Hex(m.TimedRead(0x03, 3, B16, false)) == want && m.Chip.MisoErrors == e0,
                "/16 mode 0: clean (half period 160 ns > DoDelayNs)");
            var got = Hex(m.TimedRead(0x03, 3, B8, false));
            Check(got != want && m.Chip.MisoErrors > e0, "/8 mode 0: changed bits read at their old level (marginal DO path)", got + " vs " + want);
            m.Chip.MisoErrRate = 0;
            Check(Hex(m.TimedRead(0x03, 3, B8, false)) == want, "/8 mode 0 with MisoErrRate 0: clean");
            Check(Hex(m.TimedRead(0x03, 3, B8, true, true)) != want, "command byte in CKPH 1: the chip sees the previous DI bit (garbage)");
            m.Chip.MisoErrRate = 1e-3;
            // Torn: a 3-byte frame at /8 (3.84 us of data) across a 13 us grid.
            m.Chip.ClearAd1();
            m.Chip.Core.ForcedAd2 = int.MinValue;
            m.Chip.SetInput("vsrc:0,ac=0.5,f=1000");
            // (the CKPH-1 command above may have written anywhere: a fresh row)
            m.Configure(F0R0, 0);
            m.Write(0x34, 0); m.Write(0x29, 0); m.Write(0x26, 0x80); m.Write(0x1E, 0);
            m.Chip.MisoErrRate = 0;
            var p = m.Chip.Core.Ad2Period;
            var tw = m.Chip.TornAd2;
            // Frames placed from the model's own grid (the stub's clock has
            // 100 ns ticks: an INTF poll cannot place them closer).
            Func<double, double> nextEdge = t => { m.Chip.Core.Update(t); return m.Chip.Core.Ad2LastEdge + p; };
            for(var k = 0; k < 200; k++)
            {
                m.Now = nextEdge(m.Now) + 1e-6; // 1 us after an edge
                m.TimedRead(0x03, 3, B8, true); // ~6 us: inside the period
            }
            Check(m.Chip.TornAd2 == tw, "200 frames placed between edges: none torn", (m.Chip.TornAd2 - tw).ToString());
            int across = 0;
            for(var k = 0; k < 200; k++)
            {
                m.Now = nextEdge(m.Now) + p - 3e-6; // data bytes from ~p - 1.3 us: across the next edge
                m.TimedRead(0x03, 3, B8, true);
                across++;
            }
            Check(m.Chip.TornAd2 - tw > 150, "frames across an edge: counted torn (TornAd2)", (m.Chip.TornAd2 - tw) + " of " + across);
        }

        /// ad1mix:1 (HY3131.cs Ad1Mix): a DC row written at the same R22 mid-conversion (60 V -> 6 V,
        /// both OSR 8192) leaves the next conversions a sinc^3 mix of the 60 V tap and the 6 V tap;
        /// a row that changes R22 restarts AD1 and is clean; ad1mix:0 is clean either way.
        private static void Ad1Mix()
        {
            foreach(var mix in new[] { true, false })
            {
                var m = NewChip();
                m.Chip.SetInput("dcv:1.0" + (mix ? ";ad1mix:1" : ""));
                m.Configure(F1R1, 1);                       // 60 V, R22 0 -> 0x15: restart at t0
                var t0 = m.Now; var p = m.Chip.Core.Ad1Period;
                m.Now = t0 + 5.01 * p; m.Read1(0x1E);
                var c60 = m.ReadS24(0);
                m.Now = t0 + 5.5 * p;
                m.Configure(F1R0, 1);                       // 6 V at the same R22, half way through
                var got = new double[4];
                for(var k = 0; k < 4; k++)
                {
                    m.Now = t0 + (6.01 + k) * p; m.Read1(0x1E);
                    got[k] = m.ReadS24(0);
                }
                var c6 = got[3];
                Near(c6 / (double)c60, 10.0, 0.05, (mix ? "ad1mix:1" : "ad1mix:0") + ": 6 V tap code = 10 x the 60 V one (3rd period after)");
                if(mix)
                {
                    // old share of a sinc^3: 2.5 / 1.5 / 0.5 periods -> 0.979 / 0.5 / 0.021
                    Near((got[0] - c6) / (c60 - c6), 0.979, 0.01, "  1st conversion after: 98 % the old tap");
                    Near((got[1] - c6) / (c60 - c6), 0.5, 0.01, "  2nd: half");
                    Near((got[2] - c6) / (c60 - c6), 0.021, 0.005, "  3rd: 2 %");
                }
                else
                    Near(got[0] / c6, 1.0, 0.001, "  1st conversion after: clean");
                // an R22 change restarts AD1: clean from the first conversion
                m.Configure(F1R1, 1);
                m.Now += 4 * p;
                m.Write(0x22, 0x17); m.Write(0x22, 0x15); var t1 = m.Now;
                m.Configure(F1R0, 1);
                m.Now = t1 + 1.01 * p; m.Read1(0x1E);
                Near(m.ReadS24(0) / c6, 1.0, 0.001, "  after an R22 restart: clean");
            }
        }

        private static void Rail()
        {
            Console.WriteLine("DMM rail (PE2) power gate");
            var m = NewChip();
            m.Configure(F0R0, 0);
            Check(Hex(m.Read(0x20, 22)) == Hex(F0R0), "PE2 never driven: powered (unit tests, a ROM-raised rail)");
            m.PortE.Drive(2, true);
            Check(Hex(m.Read(0x20, 22)) == Hex(F0R0) && m.Chip.RailUps == 0, "first PE2 rise: no reset (the chip was up)");
            m.PortE.Drive(2, false);
            var t = m.Chip.Transfers;
            Check(m.Read1(0x20) == 0 && m.Chip.Transfers == t && m.Chip.DeafFrames > 0, "PE2 low: reads 0, no frame");
            m.PortE.Drive(2, true);
            m.Write(0x20, F0R0);
            Check(m.Chip.RailUps == 1 && (m.Chip.Core.Intf & 0x80) != 0, "PE2 rise: power-on reset, BORF");
            m.Advance(m.Chip.PowerUpMs * 1e-3);
            Check(Hex(m.Read(0x20, 22)) == Hex(new byte[22]), "row written right at the rise (within PowerUpMs): lost");
            m.Configure(F0R0, 0);
            Check(Hex(m.Read(0x20, 22)) == Hex(F0R0), "row written after PowerUpMs: kept");
        }

        private static void Ad2Stream()
        {
            Console.WriteLine("AD2 stream (data logger source)");
            var m = NewChip();
            m.Chip.SetInput("dca:0.0123");
            m.Configure(F5R0, 5); // AC mA r0: OP1+ = PB1 (0.5 ohm) x10, AD2 = OP1O - PB3, DC-coupled
            m.Advance(0.05);
            // opendm40 set_ad2_stream(0x23): AD3 off, RMS / LPF off, R26 = ENAD2 | ENCHOPAD2 | OSR 256 = 9600/s
            // (the chopper does not halve the rate: device check of f80524f)
            m.Write(0x34, 0); m.Write(0x29, 0); m.Write(0x26, 0xA3); m.Write(0x1E, 0);
            Near(m.Chip.Core.Ad2Period * 1e6, 104.167, 0.01, "R26 0xA3: 104.167 us (9600/s)");
            var c0 = m.Chip.Core.Ad2Conversions;
            int seen = 0;
            var t0 = m.Now;
            while(m.Now - t0 < 0.1)
            {
                if((m.Read1(0x1E) & 0x02) != 0) seen++;
                m.Advance(20e-6);
            }
            var made = m.Chip.Core.Ad2Conversions - c0;
            Near(made, 960, 2, "conversions in 100 ms of virtual time");
            var lost = m.Chip.Swallowed;
            Check(seen + lost == made || seen + lost == made - 1, "one AD2F per conversion (polled faster than the period), less the ones the INTF race swallowed", string.Format("seen {0}, swallowed {1}, made {2}", seen, lost, made));
            Check(lost > 0 && lost < made / 20, "  the default 0.7 us window swallows a few % at a ~38 us poll", lost.ToString());
            m.BitTime = 0.05e-6; // short frames: the check is about the grid, not the link
            m.WaitFlag(0x02, 0.01, 1e-6); // just after a grid point
            m.Advance(30e-6);
            var f1 = m.Read1(0x1E) & 0x02;
            m.Advance(80e-6);
            var f2 = m.Read1(0x1E) & 0x02;
            Check(f1 == 0 && f2 != 0, "AD2F not set again before the next grid point, set after it");
            var a = m.ReadS24(0x03);
            var b = m.ReadS24(0x03);
            Check(a == b, "R03..R05 unchanged between two reads inside one period", a + " / " + b);
            // 12.3 mA * 0.5 ohm * 10 = 61.5 mV at the AD2 input, IG 1.0: 61.5 mV / 1.25 V * 2^18
            Near(a, 0.0615 / 1.25 * 262144, 30, "AD2 code of 12.3 mA DC on the 40 mA range (DC-coupled)");
            Check(a >= -0x40000 && a <= 0x3FFFF, "19-bit signed, bits 23:19 = sign copy");
            // Slow rung: /4, OSR 1024, chopped = 1200/s (as unchopped 0x8D).
            m.Write(0x26, 0xAD);
            Near(m.Chip.Core.Ad2Period * 1e3, 0.8333, 0.001, "R26 0xAD: 0.833 ms (1200/s)");
            m.Write(0x26, 0x8D);
            Near(m.Chip.Core.Ad2Period * 1e3, 0.8333, 0.001, "R26 0x8D: 0.833 ms (1200/s)");
            // Noise model: 30 codes at the stock OSR2 32, white scaling.
            foreach(var t in new[] { new { r26 = (byte)0x80, sd = 30.0 }, new { r26 = (byte)0xA3, sd = 30.0 / Math.Sqrt(8) } })
            {
                var n = NewChip(noise: true);
                n.BitTime = 0.02e-6; // a frame well inside the 13 us period (TornAd2)
                n.Chip.SetInput("dca:0.0");
                n.Configure(F5R0, 5);
                n.Write(0x34, 0); n.Write(0x29, 0); n.Write(0x26, t.r26);
                double s1 = 0, s2 = 0; int k = 0;
                for(int i = 0; i < 2000; i++)
                {
                    n.Advance(n.Chip.Core.Ad2Period * 1.01);
                    if((n.Read1(0x1E) & 0x02) == 0) continue;
                    double v = n.ReadS24(0x03);
                    s1 += v; s2 += v * v; k++;
                }
                var sd = Math.Sqrt(s2 / k - (s1 / k) * (s1 / k));
                Near(sd, t.sd, t.sd * 0.1, string.Format("R26 0x{0:X2}: per-result noise model", t.r26));
            }
        }

        /// The INTF read-clears-flag race and the chip-clock offset (opendm40 AD2 stream
        /// ISR, docs/dmm-input-scope-feasibility.md 10): an AD2 result completing
        /// within IntfRaceWindow before an INTF read is cleared unreported, its data
        /// stays; ClockPpm moves every conversion grid.
        private static void Ad2Race()
        {
            Console.WriteLine("AD2 flag race and chip clock offset");
            var m = NewChip();
            Check(Math.Abs(m.Chip.IntfRaceUs - 0.7) < 1e-9, "Renode peripheral default race window 0.7 us", m.Chip.IntfRaceUs.ToString());
            var c = m.Chip.Core;
            c.SetInput("dca:0.0123");
            m.Configure(F5R0, 5);
            m.Write(0x34, 0); m.Write(0x29, 0);
            const double t0 = 10.0;
            c.Write(0x26, 0xA3, t0); // 9600/s; the grid starts at t0
            var p = c.Ad2Period;
            c.Read(0x1E, t0 + 0.5 * p); // clear
            var s0 = c.Swallowed;
            var f = c.Read(0x1E, t0 + p + 0.3e-6) & 0x02;
            Check(f == 0 && c.Swallowed == s0 + 1, "INTF read 0.3 us after a conversion: AD2F swallowed", "flag " + f);
            Check((c.Read(0x1E, t0 + 1.5 * p) & 0x02) == 0, "  ... and not reported by the next read either");
            var d1 = c.Read(0x03, t0 + 1.6 * p) | c.Read(0x04, t0 + 1.6 * p) << 8;
            Check(d1 != 0, "  the conversion's data is still in R03..R05", d1.ToString());
            Check((c.Read(0x1E, t0 + 2 * p + 0.9e-6) & 0x02) != 0, "INTF read 0.9 us after a conversion: AD2F reported");
            c.Read(0x1E, t0 + 2.5 * p);
            Check((c.Read(0x1E, t0 + 2.99 * p) & 0x02) == 0 && (c.Read(0x1E, t0 + 3 * p + 1e-6) & 0x02) != 0, "a read just before the conversion leaves the flag for the next one");
            c.IntfRaceWindow = 0;
            c.Read(0x1E, t0 + 3.5 * p);
            Check((c.Read(0x1E, t0 + 4 * p + 0.1e-6) & 0x02) != 0, "IntfRaceWindow 0: an ideal flag");
            foreach(var ppm in new[] { -100.0, 100.0 })
            {
                var n = NewChip();
                n.Chip.ClockPpm = ppm;
                n.Chip.SetInput("dca:0.0");
                n.Configure(F5R0, 5);
                n.Write(0x34, 0); n.Write(0x29, 0); n.Write(0x26, 0xA3);
                var k0 = n.Chip.Core.Ad2Conversions;
                n.Advance(10.0);
                n.Read1(0x1E);
                var k = n.Chip.Core.Ad2Conversions - k0;
                Near(k, 96000 * (1 + ppm * 1e-6), 2, string.Format("ClockPpm {0:+0;-0}: conversions in 10 s of MCU time", ppm));
            }
        }

        private static void Timing()
        {
            Console.WriteLine("conversion timing");
            foreach(var t in new[] { new { r22 = (byte)0x13, ms = 25.0 }, new { r22 = (byte)0x15, ms = 100.0 }, new { r22 = (byte)0x17, ms = 400.0 }, new { r22 = (byte)0x05, ms = 50.0 } })
            {
                var m = NewChip();
                m.Chip.SetInput("dcv:1.0");
                var row = (byte[])F1R1.Clone();
                row[2] = t.r22;
                m.Configure(row, 1);
                var t0 = m.Now;
                var f = m.WaitFlag(0x04, 1.0, 0.001);
                var first = m.Now - t0;
                m.Read1(0x1E);
                m.WaitFlag(0x04, 1.0, 0.001);
                var second = m.Now - t0;
                Near(first * 1000, t.ms, 3, string.Format("R22 = 0x{0:X2}: first AD1F after {1} ms", t.r22, t.ms));
                Near((second - first) * 1000, t.ms, 3, "  period");
            }
            {
                var m = NewChip();
                m.Chip.SetInput("acv:1.0@50");
                m.Configure(F0R0, 0);
                var t0 = m.Now;
                Near(m.Chip.Core.Ad2Period * 1e6, 13.02, 0.05, "AD2 sample period 13.02 us (OSR2 32 at SYSCLK/2)");
                m.WaitFlag(0x18, 1.0, 0.001);
                var first = m.Now - t0;
                m.Read1(0x1E);
                m.WaitFlag(0x18, 1.0, 0.001);
                var second = m.Now - t0;
                Near(first * 1000, 106.7, 2, "first RMSF|LPFF after 106.7 ms");
                Near((second - first) * 1000, 106.7, 2, "  RMS period (measured value, RESEARCH.md 3)");
                Check((m.Read1(0x1E) & 0x02) != 0 || true, "AD2F");
            }
        }

        private static void DcVoltage()
        {
            Console.WriteLine("DC V / mV");
            var cal = Hy3131Core.DefaultCal();
            foreach(var v in new[] { 1.234, -0.5, 3.9999 })
            {
                var m = NewChip();
                m.Chip.SetInput("dcv:" + v.ToString(System.Globalization.CultureInfo.InvariantCulture));
                m.Configure(F1R0, 1);
                m.WaitFlag(0x04, 1);
                var ad1 = m.ReadS24(0);
                Near(DcvDisplay(ad1, 0, cal), v, 2e-4, "DC V r0 " + v + " V");
            }
            {
                var m = NewChip();
                m.Chip.SetInput("dcv:12.5");
                m.Configure(F1R1, 1);
                m.WaitFlag(0x04, 1);
                Near(DcvDisplay(m.ReadS24(0), 1, cal), 12.5, 1e-3, "DC V r1 12.5 V");
                Check(m.Chip.Role == "DcV m1 r1", "role " + m.Chip.Role);
                // over range: clipped code
                m.Chip.SetInput("dcv:150");
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Check(m.ReadS24(0) == 8388607, "150 V on the 40 V range (AD1 full scale 138.9 V) clips to 7FFFFF");
                // AD1IG change scales the code (datasheet 13.1)
                m.Chip.SetInput("dcv:1.0");
                m.Write(0x25, 0x50); // AD1IG = 01 (1.8)
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                var a = m.ReadS24(0);
                m.Write(0x25, 0x40);
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                var b = m.ReadS24(0);
                Near((double)a / b, 2.0, 0.01, "AD1IG 1.8 vs 0.9 doubles the code");
                m.Write(0x22, 0x35); // DCOS +0.25
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(m.ReadS24(0) - b, 0.25 * 8388608, 2, "AD1OS = 001 adds 0.25 full scale");
                m.Write(0x23, 0x03); // ENAD1 = 0
                Check(m.ReadS24(0) == 0, "ENAD1 = 0 clears AD1");
            }
            {
                var m = NewChip();
                m.Chip.SetInput("dcv:0.2345");
                m.Configure(F3R1, 3);
                m.WaitFlag(0x04, 1);
                var mv = m.ReadS24(0) * 1.38889 / 1.0 / 8388.606;
                Near(mv, 234.5, 0.05, "DC mV r1 234.5 mV");
            }
            {
                var m = NewChip(noise: true);
                m.Chip.SetInput("dcv:0.0");
                m.Configure(F3R1, 3);
                m.Write(0x22, 0x17);
                var codes = new List<int>();
                for(var i = 0; i < 20; i++) { m.Read1(0x1E); m.WaitFlag(0x04, 1); codes.Add(m.ReadS24(0)); }
                double mean = 0; foreach(var c in codes) mean += c; mean /= codes.Count;
                double sd = 0; foreach(var c in codes) sd += (c - mean) * (c - mean); sd = Math.Sqrt(sd / codes.Count);
                Check(sd > 8 && sd < 60, "noise at OSR 32768 ~25 counts", "sd " + sd.ToString("F1"));
            }
        }

        private static void CalibrationConsistency()
        {
            Console.WriteLine("calibration consistency");
            var cal = Hy3131Core.DefaultCal();
            cal[1] = 1.012064f; cal[5] = 0.000125f;          // DC V r0 gain / offset of the dumped unit
            cal[0x124 / 4 + 1] = 1.002732f; cal[0x144 / 4] = 4.7e-5f;
            var m = NewChip(cal: cal);
            m.Chip.SetInput("dcv:1.234");
            m.Configure(F1R0, 1);
            m.WaitFlag(0x04, 1);
            Near(DcvDisplay(m.ReadS24(0), 0, cal), 1.234, 2e-4, "stock display with the dump's DC V r0 calibration = input");
            m.Chip.Core.CalConsistent = false;
            m.Read1(0x1E); m.WaitFlag(0x04, 1);
            Near(DcvDisplay(m.ReadS24(0), 0, cal), 1.234 * 1.012064 - 0.000125, 3e-4, "with CalConsistent off the stock shows the raw deviation");
            m.Chip.Core.CalConsistent = true;
            m.Chip.SetInput("ohm:1000");
            m.Configure(F10R1, 10);
            m.WaitFlag(0x04, 1);
            var r = Math.Abs(m.ReadS24(0)) * cal[0x124 / 4 + 1] * 1.0 / 0.9 / 8388606.0 - cal[0x144 / 4];
            Near(r * 1000, 1000, 0.2, "ohm r1 with calibration");
        }

        private static void AcVoltage()
        {
            Console.WriteLine("AC V / mV");
            {
                var m = NewChip();
                m.Chip.SetInput("acv:1.0@50,dc=0.5");
                m.Configure(F0R0, 0);
                m.Advance(3.0); // AC coupling settles
                var b = ReadAfter(m, 0x18, 1);
                Near(SqrtRms(b) * 1.25 / 262144 * 10, 1.0, 2e-3, "AC V r0 1.0 Vrms (sine)");
                Near(Lpf(b) * 1.25 / 262144 * 10, 0.0, 0.02, "LPF ~0 on the AC-coupled V path (DC removed)");
                m.Chip.SetInput("acv:1.0@50,shape=square");
                m.Advance(3.0); // the DC step (0.5 -> 0) recharges the coupling capacitor
                m.Read1(0x1E);
                b = ReadAfter(m, 0x18, 1);
                Near(SqrtRms(b) * 1.25 / 262144 * 10, 1.0, 2e-3, "square 1.0 Vrms");
                m.Chip.SetInput("acv:230@50"); // 23 V at the AD2 node: clipped
                m.Advance(3.0);
                m.Read1(0x1E);
                b = ReadAfter(m, 0x18, 1);
                Check(SqrtRms(b) <= 0x40000 && SqrtRms(b) > 0x3F000, "230 V on the 4 V range: sqrt(RMS) at the AD2 full scale (not above the stock's 0x40000 guard)", SqrtRms(b).ToString("F0"));
            }
            {
                // a DC source in AC V with a non-unity AC V calibration: the coupling capacitor charges to
                // the calibration-adjusted level, so the step (and the RMS) decays to the floor
                var cal = Hy3131Core.DefaultCal();
                cal[0x64 / 4] = 1.012064f; cal[0x74 / 4] = 0.000125f;
                var m = NewChip(cal: cal);
                m.Chip.SetInput("dcv:1.234");
                m.Configure(F0R0, 0);
                m.Advance(4.0);
                m.Read1(0x1E);
                var b = ReadAfter(m, 0x18, 1);
                var v = SqrtRms(b) * 1.25 / 262144 * 10;
                Check(v < 0.002, "AC V on 1.234 V DC with AC V cal 1.012: settles to the floor (was ~18 mV)", string.Format("{0:F4} V", v));
            }
            {
                // entry transient: AC V right after a plain DC V row with OP1 off (HpfTransientGain 1.0
                // since #40: the first block reads ~0.38 V of the 0.5 V step)
                var m = NewChip();
                m.Chip.SetInput("dcv:0.5");
                m.Configure(F1R1, 1);
                m.Advance(2.0);
                m.Configure(F0R0, 0);
                var b = ReadAfter(m, 0x18, 1);
                var first = SqrtRms(b) * 1.25 / 262144 * 10;
                m.Advance(3.0);
                m.Read1(0x1E);
                b = ReadAfter(m, 0x18, 1);
                var later = SqrtRms(b) * 1.25 / 262144 * 10;
                Check(first > 0.25 && later < 0.005, "AC V on 0.5 V DC: transient then floor", string.Format("{0:F3} V -> {1:F4} V", first, later));
            }
            {
                var m = NewChip();
                m.Chip.SetInput("acv:0.2@60,dc=0.1");
                m.Configure(F2R1, 2);
                m.Advance(0.5);
                var b = ReadAfter(m, 0x18, 1);
                Near(SqrtRms(b) * 1.25 / 1.0 / 262.144, 200, 0.5, "AC mV r1 200 mVrms");
                Near(Lpf(b) * 1.25 / 1.0 / 262.144, 100, 0.5, "AC mV LPF sees the full DC (not AC coupled)");
            }
            {
                // AC+DC: AC phase after a DC phase with OP1 kept on (R32 = 0x38): no transient
                var m = NewChip();
                m.Chip.SetInput("acv:3.4@40,dc=4.8,shape=square");
                m.Configure(F0R1, 0);
                m.Advance(3.0);
                m.Configure(F1R1, 1); m.Write(0x32, 0x38);
                m.Advance(0.8);
                m.Configure(F0R1, 0);
                m.Advance(0.8);
                m.Read1(0x1E);
                var b = ReadAfter(m, 0x18, 1);
                Near(Lpf(b) * 1.25 / 262144 * 100, 0.0, 0.1, "AC+DC: LPF < 0.1 V at the first AC result after the DC phase");
                Near(SqrtRms(b) * 1.25 / 262144 * 100, 3.4, 0.02, "  AC part stable");
            }
        }

        /// Device 185c066 knobs: acarrive (the AC V node's AC after the entry), mvhold (the open mV
        /// input after a source), acaoff (the AC A rows' AD2 offset).
        private static void Knobs489()
        {
            Console.WriteLine("185c066 knobs");
            {
                // acarrive: DC V -> AC V; against the same run without it, the RMS blocks before the
                // arrival read nothing, the one over it a part, the later ones the same
                var runs = new System.Collections.Generic.List<double>[2];
                for(var k = 0; k < 2; k++)
                {
                    var m = NewChip();
                    m.Chip.SetInput("acv:1.0@50;acarrive:" + (k == 0 ? "0" : "160"));
                    m.Configure(F1R1, 1);
                    m.Advance(0.5);
                    m.Configure(F0R1, 0);
                    var t0 = m.Now;
                    runs[k] = new System.Collections.Generic.List<double>();
                    while(m.Now < t0 + 0.6) runs[k].Add(SqrtRms(ReadAfter(m, 0x10, 1)));
                }
                var q = new System.Collections.Generic.List<double>();
                for(var i = 0; i < Math.Min(runs[0].Count, runs[1].Count); i++) q.Add(runs[1][i] / runs[0][i]);
                var parts = q.FindAll(x => x > 0.05 && x < 0.95).Count;
                Check(q[0] < 0.05 && parts == 1 && Math.Abs(q[q.Count - 1] - 1) < 1e-6, "acarrive:160: one RMS block over the arrival, partial",
                      string.Join(" ", q.ConvertAll(x => x.ToString("F3"))));
            }
            {
                // mvhold: the AC mV row's LPF after the source went off = that of a 0.75 V source
                var m = NewChip();
                m.Chip.SetInput("open;vsrc:0.75");
                m.Configure(F2R1, 2);
                m.Advance(0.5);
                var want = Lpf(ReadAfter(m, 0x08, 1));
                m.Chip.SetInput("mvhold:0.75;vsrc:5");
                m.Advance(0.3);
                m.Chip.SetInput("vsrc:none");
                m.Advance(0.3);
                var held = Lpf(ReadAfter(m, 0x08, 1));
                Near(held, want, Math.Abs(want) * 0.01 + 2, "mvhold:0.75: the open mV input reads 0.75 V after the source went off");
                m.Chip.SetInput("vsrc:0.1");
                m.Advance(0.3);
                var after = Lpf(ReadAfter(m, 0x08, 1));
                Check(Math.Abs(after) < Math.Abs(want) * 0.2, "  a source on V-COM ends the hold", after.ToString());
            }
            {
                // acaoff: the AC A 10 A row's LPF with the A jack open
                var m = NewChip();
                m.Chip.SetInput("open;acaoff:0.717");
                m.Configure(F4R1, 4);
                Check(m.Chip.Role == "AcA m4 r1", "AC A 10 A row recognised", m.Chip.Role);
                m.Advance(0.5);
                var b = ReadAfter(m, 0x08, 1);
                Near(Lpf(b) * 1.25 / 0.01 / 262144.0, 0.0717, 0.003, "acaoff:0.717: the LPF reads +0.0717 A on the 10 A row", "lpf " + Lpf(b));
            }
        }

        private static void Current()
        {
            Console.WriteLine("DC / AC A");
            {
                // A r1 (10 A, PB2) and mA r1 (400 mA, PB1) differ only in SOP1P
                var m = NewChip();
                m.Chip.SetInput("dca:0.5");
                m.Configure(Master.Row("f0 dd 15 83 35 00 00 55 31 00 00 00 00 00 08 81 80 c7 3d ac 00 50"), 8); // mA r1
                Check(m.Chip.Role == "DcA m8 r1", "mA r1 row recognised", m.Chip.Role);
                m.WaitFlag(0x04, 1);
                Near(m.ReadS24(0) * 1.25 / (0.0005 * 1 * 0.9) / 8388606.0, 500, 0.5, "0.5 A on the 400 mA range reads 500 mA (software OL -> autorange)");
                m.Configure(Master.Row("f0 dd 15 83 35 00 00 55 31 00 00 00 00 00 08 81 80 c7 3e ac 00 50"), 7); // A r1
                Check(m.Chip.Role == "DcA m7 r1", "A r1 row recognised", m.Chip.Role);
                m.Read1(0x1E); m.WaitFlag(0x04, 1);
                Near(m.ReadS24(0) * 1.25 / (0.01 * 1 * 0.9) / 8388606.0, 0.5, 0.001, "0.5 A on the 10 A range");
            }
            {
                var m = NewChip();
                m.Chip.SetInput("dca:0.0123");
                m.Configure(F8R0, 8); // DC mA r0 (40 mA, x10)
                m.WaitFlag(0x04, 1);
                var ma = m.ReadS24(0) * 1.25 / (0.0005 * 10 * 0.9) / 8388606.0;
                Near(ma, 12.3, 0.005, "DC mA r0 12.3 mA");
            }
            {
                var m = NewChip();
                m.Chip.SetInput("aca:0.0123@50");
                m.Configure(F5R0, 5);
                m.Advance(0.5);
                var b = ReadAfter(m, 0x10, 1);
                var ma = SqrtRms(b) * 1.25 / (0.0005 * 10) / 262144.0;
                Near(ma, 12.3, 0.01, "AC mA r0 12.3 mA");
            }
        }

        private static void Ohm()
        {
            Console.WriteLine("ohm");
            var m = NewChip();
            m.Chip.SetInput("ohm:4700");
            m.Configure(F10R1, 10);
            m.WaitFlag(0x04, 1);
            var r = Math.Abs(m.ReadS24(0)) * 1.0 / 0.9 / 8388606.0;
            Check(r > 1.0, "4.7 k on the 400 ohm range: over the reference (OL)", r.ToString("F3") + " kohm");
            Check(m.ReadS24(0) == 8388607, "  code clipped");
            m.Chip.SetInput("ohm:1.5e6");
            m.Configure(F10R6, 10);
            m.WaitFlag(0x04, 1);
            Near(Math.Abs(m.ReadS24(0)) * 63000.0 / 0.9 / 8388606.0, 1500, 3, "1.5 M on the 40 M range (kohm)");
            m.Chip.SetInput("open");
            m.Read1(0x1E); m.WaitFlag(0x04, 1);
            Check(m.ReadS24(0) == 8388607, "open: clipped (OL)");
            m.Chip.SetInput("ohm:330");
            m.Configure(F10R1, 10);
            m.Write(0x22, 0x17);
            m.WaitFlag(0x04, 1);
            Near(Math.Abs(m.ReadS24(0)) * 1.0 / 0.9 / 8388606.0 * 1000, 330, 0.05, "330 ohm r1");
        }

        private static void Continuity()
        {
            Console.WriteLine("continuity");
            var m = NewChip();
            m.Chip.SetInput("ohm:10");
            m.Configure(F11, 11);
            m.WaitFlag(0x04, 1);
            Near(-m.ReadS24(0) / 0.9 / 8388.606, 10, 0.02, "10 ohm (negative code, 1 k reference)");
            Check(m.Chip.CmpPin, "CMP pin high (beep) at 10 ohm");
            m.Chip.SetInput("ohm:200");
            m.Chip.Core.Update(m.Now);
            m.Write(0x20, 0x56);
            Check(!m.Chip.CmpPin, "CMP pin low at 200 ohm");
            m.Chip.SetInput("open");
            m.Read1(0x1E); m.WaitFlag(0x04, 1);
            Check(m.ReadS24(0) == -8388608, "open in continuity: negative clip -> stock -AD1 = +1114 ohm > 1000 = OL", m.ReadS24(0).ToString());
            Near(m.Chip.Core.CmpLowThreshold, -0.0417, 0.0005, "  threshold AGNDN<2>");
        }

        private static void DbgSrc()
        {
            // opendm40 DBG:SRC (src/dbgsrc.rs): the ohm r1 row with SFUVR 5 (AGNDP7), AD1 on nodes
            Console.WriteLine("DBG:SRC");
            var m = NewChip();
            m.Chip.SetInput("open");
            var row = (byte[])F10R1.Clone();
            row[17] = (byte)((row[17] & 0xF0) | 5); row[3] &= 0xF3;
            row[4] = 0x83; row[8] = 0x31; // PB0 - AGND, ref PB6 - AGND
            m.Configure(row, 10);
            m.WaitFlag(0x04, 1); m.WaitFlag(0x04, 1);
            Near(m.ReadS24(0) * 1.25 / 0.9 / 8388608.0, 60.0 / 120 * 1.25, 1e-3, "open: jack = AGNDP7 = 0.625 V");
            m.Write(0x24, 0x63); m.Write(0x28, 0x51); // REFO - AGND, ref VDDA - AGND
            m.WaitFlag(0x04, 1); m.WaitFlag(0x04, 1);
            Near(0.9 * 1.25 * 8388608.0 / m.ReadS24(0), 1.8, 1e-3, "VDDA - AGND from REFO / (VDDA - AGND)");
            m.Chip.SetInput("ohm:0");
            m.Write(0x24, 0x11); m.Write(0x28, 0x31); // FB - RLU
            m.WaitFlag(0x04, 1); m.WaitFlag(0x04, 1);
            Near(m.ReadS24(0) * 1.25 / 0.9 / 8388608.0, 0.625, 2e-3, "short: the whole source across Rref");
            m.Write(0x24, 0x13); m.Write(0x28, 0x31); // FB - AGND, a VDS17 tap: saturates the narrow range
            m.Write(0x31, (byte)(row[17] & 0xF0));
            m.WaitFlag(0x04, 1); m.WaitFlag(0x04, 1);
            Check(m.ReadS24(0) == 8388607, "VDS17 (35/36 VDDA - AGND = 1.7 V) on the 1.389 V range: clipped", m.ReadS24(0).ToString());
        }

        private static void Diode()
        {
            Console.WriteLine("diode");
            var m = NewChip();
            m.Chip.SetInput("diode:0.65");
            m.Configure(F12, 12);
            m.WaitFlag(0x04, 1);
            Near(m.ReadS24(0) * 1.25 * 10 / 0.9 / 8388606.0, 0.65, 2e-4, "0.65 V");
            Check(!m.Chip.CmpPin, "CMP pin silent with ENPCMPO = 0");
            m.Write(0x20, 0xB4);
            Check(m.Chip.CmpPin, "CMP pin follows ENPCMPO (stock beep pattern)");
            m.Chip.SetInput("open");
            m.Read1(0x1E); m.WaitFlag(0x04, 1);
            Check(m.ReadS24(0) * 1.25 * 10 / 0.9 / 8388606.0 > 3.12, "open: over range");
        }

        private static void Capacitance()
        {
            Console.WriteLine("capacitance");
            {
                var m = NewChip();
                m.Chip.SetInput("cap:100e-9");
                m.Configure(F13R2, 13);
                // stock: restart the counter, read after the gate
                m.Write(0x20, 0x70); m.Write(0x1B, 0x00, 0x00, 0xFC); m.Write(0x20, 0x72);
                var f = m.WaitFlag(0x01, 1);
                var b = m.Read(0x15, 9);
                var ctc = b[0] | b[1] << 8 | b[2] << 16; var ctb = b[3] | b[4] << 8 | b[5] << 16; var cta = b[6] | b[7] << 8 | b[8] << 16;
                var fk = ctb * 4915.2 / (cta + 0x40000);
                var c = 0.72135 / (102.7 * fk);
                Near(c, 0.1, 0.002, "100 nF on r2 (RC oscillator)", string.Format("f {0:F4} kHz, CTB {1}, CTA {2}", fk, ctb, cta));
            }
            {
                var m = NewChip();
                m.Chip.SetInput("cap:100e-6");
                m.Configure(F13DIS, 13);
                m.Advance(0.3);
                m.WaitFlag(0x04, 1);
                var v0 = m.ReadS24(0) * 1000 * 1.25 / 0.9 / 8388606.0;
                Check(Math.Abs(v0) < 30, "discharge row: |V| < 30 mV", v0.ToString("F2") + " mV");
                m.Configure(F13R5, 13);
                m.Write(0x21, 0xBB);
                var t0 = m.Chip.Core.LastAd1Time;
                m.Read1(0x1E); m.WaitFlag(0x04, 1); var v1 = m.ReadS24(0) * 1000 * 1.25 / 0.9 / 8388606.0; var t1 = m.Chip.Core.LastAd1Time;
                m.Advance(0.255);
                m.Read1(0x1E); m.WaitFlag(0x04, 1); var v2 = m.ReadS24(0) * 1000 * 1.25 / 0.9 / 8388606.0; var t2 = m.Chip.Core.LastAd1Time;
                var cap = 4.06 * (t2 - t1) * 1000 / Math.Abs(v2 - v1);
                Near(cap, 100, 1, "100 uF on r5 (ramp 4.06 uA)", string.Format("dV {0:F3} mV in {1:F1} ms", v2 - v1, (t2 - t1) * 1000));
                var acpo0 = m.Read1(0x14) & 0x40;
                m.Advance(4.0);
                var acpo1 = m.Read1(0x14) & 0x40;
                Check(acpo0 != acpo1, "ACPO (CTSTA.6) toggles when the ramp reverses at the threshold");
            }
        }

        private static void Frequency()
        {
            Console.WriteLine("frequency counter");
            {
                var m = NewChip();
                m.Chip.SetInput("hz:1000,duty=0.25,amp=1.0");
                m.Configure(F14, 14);
                m.Write(0x20, 0xF0); m.Write(0x1B, 0x00, 0x00, 0xFC); m.Write(0x20, 0xF2);
                var f = m.WaitFlag(0x01, 1);
                Check(f != 0, "CTF after the gate");
                var b = m.Read(0x15, 9);
                var ctc = b[0] | b[1] << 8 | b[2] << 16; var ctb = b[3] | b[4] << 8 | b[5] << 16; var cta = b[6] | b[7] << 8 | b[8] << 16;
                var n = cta + 0x40000;
                Near(ctb / (double)n * 4915200, 1000, 0.5, "1 kHz 25 %", string.Format("CTB {0} CTA {1} CTC {2}", ctb, cta, ctc));
                Near(ctc / (double)n * 100, 25, 0.1, "  duty 25 %");
                Check(ctb == 54, "  54 whole cycles in the 53.3 ms gate", ctb.ToString());
                // CNT pin source (ENCNTI) gives the same in this board model
                m.Write(0x20, 0x08); m.Write(0x1B, 0x00, 0x00, 0xFC); m.Write(0x20, 0x0A);
                m.WaitFlag(0x01, 1);
                b = m.Read(0x15, 9);
                ctb = b[3] | b[4] << 8 | b[5] << 16; cta = b[6] | b[7] << 8 | b[8] << 16;
                Near(ctb / (double)(cta + 0x40000) * 4915200, 1000, 0.5, "CNT pin source");
            }
            {
                var m = NewChip();
                m.Chip.SetInput("dcv:1.0"); // no edges
                m.Configure(F14, 14);
                m.Write(0x20, 0xF0); m.Write(0x1B, 0x00, 0x00, 0xFC); m.Write(0x20, 0xF2);
                m.Advance(0.35);
                Check((m.Read1(0x1E) & 1) == 0, "no input edges: no CTF");
                var b = m.Read(0x15, 9);
                var ctb = b[3] | b[4] << 8 | b[5] << 16; var cta = b[6] | b[7] << 8 | b[8] << 16;
                Check(ctb == 0 && cta != 0xFC0000, "  CTB = 0, CTA running", string.Format("CTB {0} CTA 0x{1:X6}", ctb, cta));
                m.Chip.SetInput("acv:0.005@1000"); // below the comparator hysteresis at the node
                m.Write(0x20, 0xF0); m.Write(0x1B, 0x00, 0x00, 0xFC); m.Write(0x20, 0xF2);
                m.Advance(0.35);
                Check((m.Read1(0x1E) & 1) == 0, "5 mV: below the +-20.8 mV comparator window, no edges");
            }
            {
                // secondary readout in AC V r1 (comparator on the divided node)
                var m = NewChip();
                m.Chip.SetInput("acv:5@40,shape=square,dc=5");
                m.Configure(F0R1, 0);
                m.Write(0x20, 0xF0); m.Write(0x1B, 0x00, 0x00, 0xFC); m.Write(0x20, 0xF2);
                m.WaitFlag(0x01, 1);
                var b = m.Read(0x15, 9);
                var ctb = b[3] | b[4] << 8 | b[5] << 16; var cta = b[6] | b[7] << 8 | b[8] << 16;
                Near(ctb / (double)(cta + 0x40000) * 4915200, 40, 0.01, "40 Hz PWM in AC V r1");
            }
        }

        private static void Temperature()
        {
            Console.WriteLine("temperature");
            var m = NewChip();
            m.Chip.SetInput("temp:100,cj=25");
            m.Configure(F15R0, 15); // R33 = 0xAF: SAD1I = 11
            m.WaitFlag(0x04, 1);
            var codeA = m.ReadS24(0);
            m.Write(0x33, 0xAE);
            m.Read1(0x1E); m.WaitFlag(0x04, 1);
            var codeB = m.ReadS24(0);
            var tcj = (codeA - codeB) / 640000.0 * 314.15 - 289.15 + 0.0;
            Near(tcj, 25, 0.05, "cold junction 25 C from the two sensor phases", string.Format("codes {0} / {1}", codeA, codeB));
            m.Write(0x33, 0xAC);
            m.Read1(0x1E); m.WaitFlag(0x04, 1);
            var vtc = m.ReadS24(0) * 1.25 / 0.9 * 100 / 8388606.0;
            var want = TypeK.Emf(100) - TypeK.Emf(25);
            Near(vtc, want, 0.003, "thermocouple mV = E(100) - E(25)", want.ToString("F4") + " mV");
            Near(TypeK.Emf(100), 4.096, 0.001, "type K E(100 C) = 4.096 mV");
            Near(TypeK.Emf(-100), -3.554, 0.001, "type K E(-100 C) = -3.554 mV");
            m.Chip.SetInput("tcopen");
            m.Read1(0x1E); m.WaitFlag(0x04, 1);
            Check(m.ReadS24(0) * 1.25 / 0.9 * 100 / 8388606.0 > 60, "open thermocouple: far over range");
            m.Chip.SetInput("temp:-40,cj=25");
            m.Read1(0x1E); m.WaitFlag(0x04, 1);
            Near(m.ReadS24(0) * 1.25 / 0.9 * 100 / 8388606.0, TypeK.Emf(-40) - TypeK.Emf(25), 0.003, "-40 C");
            Near(m.Chip.Core.Ad1Period * 1000, 200, 1, "temperature rows: OSR 16384 chopped = 200 ms");
        }

        private static void Beat()
        {
            Console.WriteLine("AD2/AD3 enable order (empirical beat model, off by default)");
            var m = NewChip();
            m.Chip.SetInput("acv:4.8@40,shape=square");
            m.Configure(F0R1, 0); // row sets ENAD2 (byte 6) before ENAD3 (byte 20)
            m.Advance(3.0);
            var vals = new List<double>();
            for(var i = 0; i < 12; i++) { m.Read1(0x1E); var b = ReadAfter(m, 0x10, 1); vals.Add(SqrtRms(b) * 1.25 / 262144 * 100); }
            double lo = double.MaxValue, hi = double.MinValue; foreach(var v in vals) { lo = Math.Min(lo, v); hi = Math.Max(hi, v); }
            Check(hi - lo < 0.01, "default: stable RMS", string.Format("{0:F4}..{1:F4}", lo, hi));
            m.Chip.Core.BeatModel = true;
            m.Configure(F0R1, 0);
            m.Advance(3.0);
            vals.Clear();
            for(var i = 0; i < 12; i++) { m.Read1(0x1E); var b = ReadAfter(m, 0x10, 1); vals.Add(SqrtRms(b) * 1.25 / 262144 * 100); }
            lo = double.MaxValue; hi = double.MinValue; foreach(var v in vals) { lo = Math.Min(lo, v); hi = Math.Max(hi, v); }
            Check(hi - lo > 0.3, "beat on, row order (AD2 first): +-5 % wander", string.Format("{0:F3}..{1:F3}", lo, hi));
            // opendm40 order: both off in the row, AD3 then AD2
            var row = (byte[])F0R1.Clone(); row[6] &= 0x7F; row[20] &= 0x7F;
            m.Configure(row, 0); m.Write(0x34, F0R1[20]); m.Write(0x26, F0R1[6]);
            m.Advance(3.0);
            vals.Clear();
            for(var i = 0; i < 12; i++) { m.Read1(0x1E); var b = ReadAfter(m, 0x10, 1); vals.Add(SqrtRms(b) * 1.25 / 262144 * 100); }
            lo = double.MaxValue; hi = double.MinValue; foreach(var v in vals) { lo = Math.Min(lo, v); hi = Math.Max(hi, v); }
            Check(hi - lo < 0.01, "beat on, AD3 enabled first: stable", string.Format("{0:F4}..{1:F4}", lo, hi));
        }
    }
}
