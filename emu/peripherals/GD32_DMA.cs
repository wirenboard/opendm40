//
// STM32F4/GD32F4 DMA controller (8 streams) with request-driven transfers:
// every rising edge on request input N moves ONE data unit on stream N
// (peripheral-to-memory or memory-to-peripheral), memory-to-memory streams
// transfer everything when enabled. NDTR counts down and reads back, HTIF/TCIF
// are raised (TCIF also when software clears EN), circular streams reload. This replaces Renode's STM32DMA for the
// DM40 because the stock firmware enables its I2C TX channel before the
// address phase (STM32DMA would push the data at once), the stock scope and
// opendm40's BT UART use circular streams, and the scope feeder needs to drive
// one sample per timer request.
//
using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;

namespace Antmicro.Renode.Peripherals.DMA
{
    public class GD32_DMA : IDoubleWordPeripheral, IKnownSize, IGPIOReceiver, INumberedGPIOOutput
    {
        public GD32_DMA(IMachine machine)
        {
            this.machine = machine;
            var conns = new Dictionary<int, IGPIO>();
            for(var i = 0; i < 8; i++)
            {
                conns[i] = new GPIO();
                streams[i] = new Stream(this, i);
            }
            Connections = new ReadOnlyDictionary<int, IGPIO>(conns);
            Reset();
        }

        public void Reset()
        {
            foreach(var s in streams) s.Reset();
            UpdateInterrupts();
        }

        public long Size => 0x400;
        public IReadOnlyDictionary<int, IGPIO> Connections { get; }

        /// Request on stream `number` (rising edge = one data unit).
        public void OnGPIO(int number, bool value)
        {
            if(number < 0 || number > 7 || !value) return;
            streams[number].Request();
        }

        /// Monitor/feeder API: perform one data unit on a stream now.
        public void Kick(int stream)
        {
            streams[stream].Request();
        }

        /// Feeder API: one peripheral-to-memory unit with `value` as the
        /// peripheral data (the ADC sample) instead of reading PAR.
        /// Feeder API, bulk: `n` 16-bit peripheral values from buf[off..] into a
        /// peripheral-to-memory stream; returns how many were taken (the stream may
        /// stop at the end of a non-circular transfer).
        public int FeedBlock(int stream, ushort[] buf, int off, int n)
        {
            return streams[stream].FeedBlock(buf, off, n);
        }

        public void Feed(int stream, uint value)
        {
            streams[stream].Request(value, true);
        }

        public uint Remaining(int stream) => streams[stream].remaining;
        /// Number of times the stream was enabled (acquisition generation).
        public ulong EnableCount(int stream) => streams[stream].enableCount;
        public bool Enabled(int stream) => streams[stream].enabled;

        /// Called with the stream number before a stream register access (the
        /// scope feeder catches up the sample stream before NDTR is read and
        /// before the stream is disabled).
        public Action<int> BeforeStreamRead;

        /// Called with the stream number after a peripheral stream was enabled:
        /// a peripheral whose DMA request is a level already high (an SPI's TBE)
        /// requests then (GD32_SPI).
        public Action<int> StreamEnabled;

        public uint ReadDoubleWord(long offset)
        {
            if(offset >= 0x10 && offset < 0x10 + 8 * 0x18 && BeforeStreamRead != null)
            {
                BeforeStreamRead((int)((offset - 0x10) / 0x18));
            }
            if(offset == 0x00) return StatusWord(0);
            if(offset == 0x04) return StatusWord(4);
            if(offset == 0x08 || offset == 0x0C) return 0;
            if(offset >= 0x10 && offset < 0x10 + 8 * 0x18)
            {
                var s = (int)((offset - 0x10) / 0x18);
                var r = (offset - 0x10) % 0x18;
                return streams[s].Read(r);
            }
            return 0;
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            if(offset >= 0x10 && offset < 0x10 + 8 * 0x18 && BeforeStreamRead != null && (offset - 0x10) % 0x18 == 0)
            {
                BeforeStreamRead((int)((offset - 0x10) / 0x18)); // CR write: EN may be cleared
            }
            if(offset == 0x08) { ClearFlags(0, value); return; }
            if(offset == 0x0C) { ClearFlags(4, value); return; }
            if(offset >= 0x10 && offset < 0x10 + 8 * 0x18)
            {
                var s = (int)((offset - 0x10) / 0x18);
                var r = (offset - 0x10) % 0x18;
                streams[s].Write(r, value);
            }
        }

        private static readonly int[] FlagShift = { 0, 6, 16, 22 };

        private uint StatusWord(int first)
        {
            uint v = 0;
            for(var i = 0; i < 4; i++)
            {
                v |= (uint)streams[first + i].flags << FlagShift[i];
            }
            return v;
        }

        private void ClearFlags(int first, uint value)
        {
            for(var i = 0; i < 4; i++)
            {
                streams[first + i].flags &= (byte)~((value >> FlagShift[i]) & 0x3D);
            }
            UpdateInterrupts();
        }

        private void UpdateInterrupts()
        {
            for(var i = 0; i < 8; i++)
            {
                Connections[i].Set(streams[i].IrqPending);
            }
        }

        private class Stream
        {
            public Stream(GD32_DMA parent, int id) { this.parent = parent; this.id = id; }

            public void Reset()
            {
                cr = 0; ndtr = 0; par = 0; m0ar = 0; m1ar = 0; fcr = 0x21; flags = 0;
                remaining = 0; count = 0; memOff = 0; perOff = 0; enabled = false;
            }

            public uint Read(long r)
            {
                switch(r)
                {
                case 0x00: return (cr & ~1u) | (enabled ? 1u : 0u);
                case 0x04: return remaining;
                case 0x08: return par;
                case 0x0C: return m0ar;
                case 0x10: return m1ar;
                case 0x14: return fcr;
                default: return 0;
                }
            }

            public void Write(long r, uint value)
            {
                switch(r)
                {
                case 0x00:
                    {
                        var wasEnabled = enabled;
                        cr = value & ~1u;
                        var en = (value & 1) != 0;
                        if(en && !wasEnabled)
                        {
                            Enable();
                            if(enabled) parent.StreamEnabled?.Invoke(id);
                        }
                        else if(!en && wasEnabled)
                        {
                            // A stream disabled by software mid-transfer
                            // raises its transfer-complete flag (RM0090
                            // "DMA transfer suspension"; the DM40's GD32F4
                            // does too: a scope ring stopped mid-lap reads
                            // TCIF4 set, device 2026-10-26).
                            enabled = false;
                            flags |= TCIF;
                        }
                        parent.UpdateInterrupts();
                    }
                    break;
                case 0x04: ndtr = value & 0xFFFF; break;
                case 0x08: par = value; break;
                case 0x0C: m0ar = value; break;
                case 0x10: m1ar = value; break;
                case 0x14: fcr = (value & 0x87) | 0x20; break;
                }
            }

            private void Enable()
            {
                enableCount++;
                count = ndtr;
                remaining = ndtr;
                memOff = 0; perOff = 0;
                enabled = true;
                if(remaining == 0)
                {
                    parent.Log(LogLevel.Warning, "stream {0} enabled with NDTR = 0", id);
                    enabled = false;
                    return;
                }
                if(Dir == 2) // memory to memory: all at once
                {
                    while(enabled && remaining > 0)
                    {
                        Request();
                    }
                }
            }

            public void Request()
            {
                Request(0, false);
            }

            public int FeedBlock(ushort[] buf, int off, int n)
            {
                var psize = 1 << (int)((cr >> 11) & 3);
                var msize = 1 << (int)((cr >> 13) & 3);
                var minc = (cr & (1 << 10)) != 0;
                if(psize != 2 || msize != 2 || !minc || Dir != 0)
                {
                    var k = 0;
                    for(; k < n && enabled; k++)
                    {
                        Request(buf[off + k], true);
                    }
                    return k;
                }
                var taken = 0;
                var bus = parent.machine.SystemBus;
                while(n > 0 && enabled && remaining > 0)
                {
                    var chunk = (int)Math.Min((uint)n, remaining);
                    var bytes = new byte[chunk * 2];
                    Buffer.BlockCopy(buf, (off + taken) * 2, bytes, 0, chunk * 2);
                    bus.WriteBytes(bytes, (CurrentTarget ? m1ar : m0ar) + memOff);
                    memOff += (uint)(chunk * 2);
                    var before = remaining;
                    remaining -= (uint)chunk;
                    taken += chunk;
                    n -= chunk;
                    if(count > 1 && before > count / 2 && remaining <= count / 2)
                    {
                        flags |= HTIF;
                    }
                    if(remaining == 0)
                    {
                        flags |= TCIF;
                        if((cr & (1 << 8)) != 0)
                        {
                            remaining = count;
                            memOff = 0; perOff = 0;
                            if((cr & (1 << 18)) != 0) cr ^= 1 << 19;
                        }
                        else
                        {
                            enabled = false;
                        }
                    }
                }
                parent.UpdateInterrupts();
                return taken;
            }

            public void Request(uint fed, bool useFed)
            {
                if(!enabled || remaining == 0)
                {
                    return;
                }
                var psize = 1 << (int)((cr >> 11) & 3);
                var msize = 1 << (int)((cr >> 13) & 3);
                var pinc = (cr & (1 << 9)) != 0;
                var minc = (cr & (1 << 10)) != 0;
                var mem = (CurrentTarget ? m1ar : m0ar) + memOff;
                var per = par + perOff;
                var bus = parent.machine.SystemBus;
                uint v;
                if(Dir == 1) // memory -> peripheral
                {
                    v = ReadUnit(bus, mem, msize);
                    WriteUnit(bus, per, psize, v);
                }
                else // peripheral (or memory for M2M) -> memory
                {
                    v = useFed ? fed : ReadUnit(bus, per, psize);
                    WriteUnit(bus, mem, msize, v);
                }
                if(minc) memOff += (uint)msize;
                if(pinc) perOff += (uint)psize;
                remaining--;
                if(remaining == count / 2 && count > 1)
                {
                    flags |= HTIF;
                }
                if(remaining == 0)
                {
                    flags |= TCIF;
                    if((cr & (1 << 8)) != 0) // circular
                    {
                        remaining = count;
                        memOff = 0; perOff = 0;
                        if((cr & (1 << 18)) != 0) // double buffer: switch target
                        {
                            cr ^= 1 << 19;
                        }
                    }
                    else
                    {
                        enabled = false;
                    }
                }
                parent.UpdateInterrupts();
            }

            private static uint ReadUnit(IBusController bus, uint addr, int size)
            {
                switch(size)
                {
                case 1: return bus.ReadByte(addr);
                case 2: return bus.ReadWord(addr);
                default: return bus.ReadDoubleWord(addr);
                }
            }

            private static void WriteUnit(IBusController bus, uint addr, int size, uint v)
            {
                switch(size)
                {
                case 1: bus.WriteByte(addr, (byte)v); break;
                case 2: bus.WriteWord(addr, (ushort)v); break;
                default: bus.WriteDoubleWord(addr, v); break;
                }
            }

            public bool IrqPending =>
                ((flags & TCIF) != 0 && (cr & (1 << 4)) != 0) ||
                ((flags & HTIF) != 0 && (cr & (1 << 3)) != 0) ||
                ((flags & TEIF) != 0 && (cr & (1 << 2)) != 0);

            private int Dir => (int)((cr >> 6) & 3);
            private bool CurrentTarget => (cr & (1 << 19)) != 0;

            private const byte TCIF = 1 << 5, HTIF = 1 << 4, TEIF = 1 << 3;
            private readonly GD32_DMA parent;
            private readonly int id;
            private uint cr, ndtr, par, m0ar, m1ar, fcr, memOff, perOff;
            public uint remaining, count;
            public ulong enableCount;
            public byte flags;
            public bool enabled;
        }

        private readonly IMachine machine;
        private readonly Stream[] streams = new Stream[8];
    }
}
