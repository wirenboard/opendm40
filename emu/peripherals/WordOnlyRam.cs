//
// RAM whose writes ignore the transfer size, as the GD32F427's USBHS FIFO RAM
// window at 0x40060000 on the DM40: a byte store writes the whole 32-bit word
// with the byte in all four lanes ("DLG1" stored byte by byte reads back
// 31 31 31 31). A halfword store is modelled as the halfword in both halves
// (the AHB data bus replication; inferred, the device was seen with byte
// stores). Reads of any size and 32-bit writes are plain memory.
// firmware/src/wordram.rs is the firmware side.
//
using System;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals;
using Antmicro.Renode.Peripherals.Bus;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public class WordOnlyRam : IDoubleWordPeripheral, IWordPeripheral, IBytePeripheral, IKnownSize
    {
        public WordOnlyRam(long size = 0x1000)
        {
            mem = new uint[size / 4];
        }

        public void Reset()
        {
            // Plain RAM keeps its bytes over a machine reset (as Renode's
            // MappedMemory); the firmware zeroes the area at boot.
        }

        public long Size => mem.Length * 4;

        public uint ReadDoubleWord(long offset)
        {
            return mem[offset >> 2];
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            mem[offset >> 2] = value;
        }

        public ushort ReadWord(long offset)
        {
            return (ushort)(mem[offset >> 2] >> (int)(8 * (offset & 2)));
        }

        public void WriteWord(long offset, ushort value)
        {
            Replicated(offset, value | ((uint)value << 16), 2);
        }

        public byte ReadByte(long offset)
        {
            return (byte)(mem[offset >> 2] >> (int)(8 * (offset & 3)));
        }

        public void WriteByte(long offset, byte value)
        {
            uint v = value;
            Replicated(offset, v | v << 8 | v << 16 | v << 24, 1);
        }

        // A sub-word store: the whole word takes the replicated bus value.
        // Counted and logged once, so a run shows the firmware did one.
        private void Replicated(long offset, uint value, int size)
        {
            mem[offset >> 2] = value;
            subWord++;
            if(subWord == 1)
            {
                this.Log(LogLevel.Warning, "{0}-byte store at +0x{1:X}: the whole word written (0x{2:X8}); word stores only", size, offset, value);
            }
        }

        public ulong SubWordStores => subWord;

        private readonly uint[] mem;
        private ulong subWord;
    }
}
