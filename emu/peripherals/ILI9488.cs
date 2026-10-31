//
// ILI9488 TFT controller on the DM40: 16-bit 8080 bus on FSMC bank 1 (NE1).
// Register select = FSMC A16 = sysbus offset bit 17 (0x6001FFFE command,
// 0x60020000 data). Models the commands the stock firmware and opendm40 use:
// 0x2A/0x2B window, 0x2C RAM write, 0x2E RAM read (RGB666 read-back packing),
// 0x36 MADCTL, 0x3A COLMOD, 0xD3 ID read (0x009488), 0x11/0x29 and the rest
// as no-ops with parameters. GRAM is the physical 320x480 panel; Screenshot
// writes it rotated to the 480x320 landscape view the user sees.
// `Trace` logs every RAM write burst (window, start / end virtual time,
// run-length coded pixels) for emu/lcdtrace.py: the frames the panel shows
// while a screen is drawn.
//
using System;
using System.IO;
using System.IO.Compression;
using System.IO.MemoryMappedFiles;
using System.Threading;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Video
{
    public class ILI9488 : IWordPeripheral, IDoubleWordPeripheral, IBytePeripheral, IKnownSize
    {
        public ILI9488(IMachine machine)
        {
            this.machine = machine;
            gram = new ushort[PanelW * PanelH];
            Reset();
        }

        public void Reset()
        {
            Array.Clear(gram, 0, gram.Length);
            cmd = 0; argc = 0; readIdx = 0;
            xs = 0; xe = PanelW - 1; ys = 0; ye = PanelH - 1;
            col = 0; row = 0;
            madctl = 0; colmod = 0x66;
            PixelsWritten = 0; Frames = 0; Commands = 0;
            sleeping = true; displayOn = false;
        }

        public long Size => 0x04000000;

        // ---- bus ----
        public ushort ReadWord(long offset)
        {
            if((offset & RsBit) == 0)
            {
                return 0;
            }
            return ReadData();
        }

        public void WriteWord(long offset, ushort value)
        {
            if((offset & RsBit) == 0)
            {
                Command((byte)value);
            }
            else
            {
                Data(value);
            }
        }

        public uint ReadDoubleWord(long offset)
        {
            var lo = ReadWord(offset);
            var hi = ReadWord(offset + 2);
            return (uint)lo | ((uint)hi << 16);
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            WriteWord(offset, (ushort)value);
            WriteWord(offset + 2, (ushort)(value >> 16));
        }

        public byte ReadByte(long offset)
        {
            return (byte)ReadWord(offset & ~1L);
        }

        public void WriteByte(long offset, byte value)
        {
            WriteWord(offset & ~1L, value);
        }

        // ---- monitor API ----
        /// Save the landscape 480x320 view as PNG.
        public void Screenshot(string path)
        {
            var w = ViewW; var h = ViewH;
            var rgb = new byte[w * h * 3];
            for(var oy = 0; oy < h; oy++)
            {
                for(var ox = 0; ox < w; ox++)
                {
                    int px, py;
                    switch(Rotation & 3)
                    {
                    case 0: px = oy; py = PanelH - 1 - ox; break;
                    case 1: px = PanelW - 1 - oy; py = ox; break;
                    case 2: px = oy; py = ox; break;
                    default: px = PanelW - 1 - oy; py = PanelH - 1 - ox; break;
                    }
                    var c = gram[py * PanelW + px];
                    var o = (oy * w + ox) * 3;
                    var r = (c >> 11) & 0x1F; var g = (c >> 5) & 0x3F; var b = c & 0x1F;
                    rgb[o] = (byte)((r << 3) | (r >> 2));
                    rgb[o + 1] = (byte)((g << 2) | (g >> 4));
                    rgb[o + 2] = (byte)((b << 3) | (b >> 2));
                }
            }
            File.WriteAllBytes(path, Png.Encode(w, h, rgb));
            this.Log(LogLevel.Info, "screenshot {0} (frames {1}, pixels {2}, t={3})", path, Frames, PixelsWritten, machine.ElapsedVirtualTime.TimeElapsed);
        }

        /// Live view for emu/live.py: a memory-mapped file refreshed `fps` times per
        /// second (wall clock) by a host timer. Layout: "DM40LIVE", u32 seq, u32 w,
        /// u32 h, u32 pad, u64 virtual time in us, u64 pixels written, u64 RAMWR
        /// count, then w*h RGB565 (LE) of the landscape view. Torn frames are possible
        /// and harmless (the next refresh fixes them).
        public void LiveFile(string path, int fps)
        {
            var size = LiveHeader + ViewW * ViewH * 2;
            liveFile = MemoryMappedFile.CreateFromFile(path, FileMode.Create, null, size);
            liveView = liveFile.CreateViewAccessor(0, size);
            var magic = System.Text.Encoding.ASCII.GetBytes("DM40LIVE");
            liveView.WriteArray(0, magic, 0, 8);
            liveView.Write(12, (uint)ViewW);
            liveView.Write(16, (uint)ViewH);
            liveTimer = new Timer(_ => LiveUpdate(), null, 0, Math.Max(10, 1000 / Math.Max(1, fps)));
            this.Log(LogLevel.Info, "live view in {0} at {1} fps", path, fps);
        }

        private void LiveUpdate()
        {
            try
            {
                for(var oy = 0; oy < ViewH; oy++)
                {
                    for(var ox = 0; ox < ViewW; ox++)
                    {
                        liveBuf[oy * ViewW + ox] = gram[ViewIndex(ox, oy)];
                    }
                }
                liveView.WriteArray(LiveHeader, liveBuf, 0, liveBuf.Length);
                liveView.Write(24, (ulong)(machine.ElapsedVirtualTime.TimeElapsed.TotalSeconds * 1e6));
                liveView.Write(32, PixelsWritten);
                liveView.Write(40, Frames);
                liveView.Write(8, ++liveSeq);
            }
            catch(Exception)
            {
                // machine disposed while the timer fired
            }
        }

        private int ViewIndex(int ox, int oy)
        {
            int px, py;
            switch(Rotation & 3)
            {
            case 0: px = oy; py = PanelH - 1 - ox; break;
            case 1: px = PanelW - 1 - oy; py = ox; break;
            case 2: px = oy; py = ox; break;
            default: px = PanelW - 1 - oy; py = PanelH - 1 - ox; break;
            }
            return py * PanelW + px;
        }

        /// Save the raw physical GRAM (320x480 RGB565 LE) for debugging.
        public void DumpGram(string path)
        {
            var b = new byte[gram.Length * 2];
            Buffer.BlockCopy(gram, 0, b, 0, b.Length);
            File.WriteAllBytes(path, b);
        }

        /// Write a PNG to dir every n RAMWR commands (0 = off).
        public void AutoShot(string dir, int everyFrames)
        {
            autoDir = dir; autoEvery = everyFrames;
            if(everyFrames > 0) Directory.CreateDirectory(dir);
        }

        /// Log every RAMWR burst to `path` (empty = stop). Record: "W", f64 t0,
        /// f64 t1 (virtual s: the RAMWR command, the next command), u16 xs xe
        /// ys ye (controller column / page window), u8 MADCTL, u32 runs, then
        /// runs x (u32 count, u16 RGB565). emu/lcdtrace.py replays it.
        public void Trace(string path)
        {
            TraceFlush();
            traceOut?.Dispose();
            traceOut = null;
            if(!string.IsNullOrEmpty(path))
            {
                traceOut = new BinaryWriter(new BufferedStream(File.Create(path), 1 << 20));
            }
            this.Log(LogLevel.Info, "trace {0} at t={1}", string.IsNullOrEmpty(path) ? "off" : path, machine.ElapsedVirtualTime.TimeElapsed);
        }

        private double TraceNow()
        {
            if(machine.SystemBus.TryGetCurrentCPU(out var cpu)) cpu.SyncTime();
            return machine.ElapsedVirtualTime.TimeElapsed.TotalSeconds;
        }

        private void TraceFlush()
        {
            if(traceOut == null || !traceOpen)
            {
                return;
            }
            traceOpen = false;
            if(traceRunVal >= 0)
            {
                traceRuns.Add(((uint)traceRunN, (ushort)traceRunVal));
            }
            var w = traceOut;
            w.Write((byte)'W');
            w.Write(traceT0);
            w.Write(TraceNow());
            w.Write((ushort)traceWin[0]); w.Write((ushort)traceWin[1]);
            w.Write((ushort)traceWin[2]); w.Write((ushort)traceWin[3]);
            w.Write(traceWinMad);
            w.Write((uint)traceRuns.Count);
            foreach(var r in traceRuns)
            {
                w.Write(r.Item1); w.Write(r.Item2);
            }
            traceRuns.Clear();
            w.Flush();
        }

        private void TracePixel(ushort v)
        {
            if(v == traceRunVal)
            {
                traceRunN++;
                return;
            }
            if(traceRunVal >= 0)
            {
                traceRuns.Add(((uint)traceRunN, (ushort)traceRunVal));
            }
            traceRunVal = v;
            traceRunN = 1;
        }

        private BinaryWriter traceOut;
        private bool traceOpen;
        private double traceT0;
        private readonly int[] traceWin = new int[4];
        private byte traceWinMad;
        private int traceRunVal = -1;
        private long traceRunN;
        private readonly System.Collections.Generic.List<(uint, ushort)> traceRuns = new System.Collections.Generic.List<(uint, ushort)>();

        public int Rotation { get; set; } = 3;
        public ulong PixelsWritten { get; private set; }
        public ulong Frames { get; private set; }
        public ulong Commands { get; private set; }
        public byte Madctl => madctl;
        public bool DisplayOn => displayOn && !sleeping;

        // ---- controller ----
        private void Command(byte c)
        {
            Commands++;
            if(Commands <= 4)
            {
                this.Log(LogLevel.Info, "first commands: 0x{0:X2} at t={1}", c, machine.ElapsedVirtualTime.TimeElapsed);
            }
            cmd = c; argc = 0; readIdx = 0;
            if(traceOut != null)
            {
                TraceFlush();
                if(c == 0x2C || c == 0x3C)
                {
                    traceOpen = true;
                    traceT0 = TraceNow();
                    traceWin[0] = xs; traceWin[1] = xe; traceWin[2] = ys; traceWin[3] = ye;
                    traceWinMad = madctl;
                    traceRunVal = -1;
                    traceRunN = 0;
                }
            }
            switch(c)
            {
            case 0x2C: // RAMWR
            case 0x3C: // RAMWR continue
                col = xs; row = ys;
                Frames++;
                if(autoEvery > 0 && Frames % (ulong)autoEvery == 0)
                {
                    Screenshot(Path.Combine(autoDir, string.Format("frame-{0:D6}.png", Frames)));
                }
                break;
            case 0x2E: // RAMRD
                col = xs; row = ys;
                break;
            case 0x11: sleeping = false; break;
            case 0x10: sleeping = true; break;
            case 0x29: displayOn = true; break;
            case 0x28: displayOn = false; break;
            case 0x01: Reset(); break;
            }
        }

        private void Data(ushort v)
        {
            switch(cmd)
            {
            case 0x2A: // column address set
                SetAddr(ref xs, ref xe, v);
                break;
            case 0x2B: // page address set
                SetAddr(ref ys, ref ye, v);
                break;
            case 0x2C:
            case 0x3C:
                WritePixel(v);
                break;
            case 0x36:
                if(argc++ == 0) madctl = (byte)v;
                break;
            case 0x3A:
                if(argc++ == 0) colmod = (byte)v;
                break;
            default:
                argc++;
                break;
            }
        }

        private void SetAddr(ref int s, ref int e, ushort v)
        {
            switch(argc++)
            {
            case 0: s = (s & 0xFF) | ((v & 0xFF) << 8); break;
            case 1: s = (s & 0xFF00) | (v & 0xFF); break;
            case 2: e = (e & 0xFF) | ((v & 0xFF) << 8); break;
            case 3: e = (e & 0xFF00) | (v & 0xFF); col = xs; row = ys; break;
            }
        }

        private bool MV => (madctl & 0x20) != 0;
        private bool MX => (madctl & 0x40) != 0;
        private bool MY => (madctl & 0x80) != 0;

        private int Index(int c, int r)
        {
            int x, y;
            if(MV) { x = r; y = c; } else { x = c; y = r; }
            if(MX) x = PanelW - 1 - x;
            if(MY) y = PanelH - 1 - y;
            if(x < 0 || x >= PanelW || y < 0 || y >= PanelH)
            {
                return -1;
            }
            return y * PanelW + x;
        }

        private void Advance()
        {
            col++;
            if(col > xe)
            {
                col = xs;
                row++;
                if(row > ye)
                {
                    row = ys;
                }
            }
        }

        private void WritePixel(ushort v)
        {
            var i = Index(col, row);
            if(i >= 0)
            {
                gram[i] = v;
            }
            PixelsWritten++;
            if(traceOpen)
            {
                TracePixel(v);
            }
            Advance();
        }

        private ushort ReadData()
        {
            switch(cmd)
            {
            case 0xD3: // ID: dummy, 0x00, 0x94, 0x88
                switch(readIdx++)
                {
                case 0: return 0;
                case 1: return 0x00;
                case 2: return 0x94;
                default: return 0x88;
                }
            case 0x04: // RDDID (dummy, then 3 bytes)
                switch(readIdx++)
                {
                case 0: return 0;
                case 1: return 0x54;
                case 2: return 0x80;
                default: return 0x66;
                }
            case 0x0A: // RDDPM
                return (ushort)(readIdx++ == 0 ? 0 : 0x9C);
            case 0x2E: // RAMRD: dummy word, then RGB666 bytes packed 2 per word
                if(readIdx++ == 0)
                {
                    rdHave = 0;
                    return 0;
                }
                return ReadPixelWord();
            default:
                readIdx++;
                return 0;
            }
        }

        // Byte stream R,G,B per pixel (6 significant bits, left aligned), two bytes per word.
        private ushort ReadPixelWord()
        {
            var hi = NextReadByte();
            var lo = NextReadByte();
            return (ushort)((hi << 8) | lo);
        }

        private byte NextReadByte()
        {
            if(rdHave == 0)
            {
                var i = Index(col, row);
                var c = i >= 0 ? gram[i] : (ushort)0;
                rdBytes[0] = (byte)((c >> 8) & 0xF8);
                rdBytes[1] = (byte)((c >> 3) & 0xFC);
                rdBytes[2] = (byte)((c << 3) & 0xF8);
                rdHave = 3;
                Advance();
            }
            var b = rdBytes[3 - rdHave];
            rdHave--;
            return b;
        }

        private const int LiveHeader = 64;
        private MemoryMappedFile liveFile;
        private MemoryMappedViewAccessor liveView;
        private Timer liveTimer;
        private uint liveSeq;
        private readonly ushort[] liveBuf = new ushort[480 * 320];

        private const int PanelW = 320;
        private const int PanelH = 480;
        private const int ViewW = 480;
        private const int ViewH = 320;
        private const long RsBit = 0x20000;

        private readonly IMachine machine;
        private readonly ushort[] gram;
        private byte cmd, madctl, colmod;
        private int argc, readIdx, rdHave;
        private readonly byte[] rdBytes = new byte[3];
        private int xs, xe, ys, ye, col, row;
        private bool sleeping, displayOn;
        private string autoDir;
        private int autoEvery;
    }

    // Minimal PNG encoder (RGB8, no filtering) using the .NET zlib stream.
    public static class Png
    {
        public static byte[] Encode(int w, int h, byte[] rgb)
        {
            var raw = new byte[(w * 3 + 1) * h];
            for(var y = 0; y < h; y++)
            {
                raw[y * (w * 3 + 1)] = 0;
                Buffer.BlockCopy(rgb, y * w * 3, raw, y * (w * 3 + 1) + 1, w * 3);
            }
            byte[] z;
            using(var ms = new MemoryStream())
            {
                using(var zs = new ZLibStream(ms, CompressionLevel.Fastest, true))
                {
                    zs.Write(raw, 0, raw.Length);
                }
                z = ms.ToArray();
            }
            using(var o = new MemoryStream())
            {
                o.Write(new byte[] { 0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A }, 0, 8);
                var ihdr = new byte[13];
                WriteBE(ihdr, 0, (uint)w);
                WriteBE(ihdr, 4, (uint)h);
                ihdr[8] = 8; ihdr[9] = 2; ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;
                Chunk(o, "IHDR", ihdr);
                Chunk(o, "IDAT", z);
                Chunk(o, "IEND", new byte[0]);
                return o.ToArray();
            }
        }

        private static void Chunk(Stream o, string type, byte[] data)
        {
            var len = new byte[4];
            WriteBE(len, 0, (uint)data.Length);
            o.Write(len, 0, 4);
            var t = System.Text.Encoding.ASCII.GetBytes(type);
            o.Write(t, 0, 4);
            o.Write(data, 0, data.Length);
            var crc = Crc32(t, 0xFFFFFFFF);
            crc = Crc32(data, crc) ^ 0xFFFFFFFF;
            var c = new byte[4];
            WriteBE(c, 0, crc);
            o.Write(c, 0, 4);
        }

        private static void WriteBE(byte[] b, int o, uint v)
        {
            b[o] = (byte)(v >> 24); b[o + 1] = (byte)(v >> 16); b[o + 2] = (byte)(v >> 8); b[o + 3] = (byte)v;
        }

        private static uint Crc32(byte[] data, uint crc)
        {
            if(table == null)
            {
                table = new uint[256];
                for(uint n = 0; n < 256; n++)
                {
                    var c = n;
                    for(var k = 0; k < 8; k++)
                    {
                        c = (c & 1) != 0 ? 0xEDB88320 ^ (c >> 1) : c >> 1;
                    }
                    table[n] = c;
                }
            }
            foreach(var b in data)
            {
                crc = table[(crc ^ b) & 0xFF] ^ (crc >> 8);
            }
            return crc;
        }

        private static uint[] table;
    }
}
