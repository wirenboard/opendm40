//
// GD32F4/STM32F4 GPIO port with correct input handling. Renode 1.17.0's
// STM32_GPIOPort.WriteState writes every pin's State on an ODR/BSRR write
// (no mode check), which clobbers inputs driven by external models during a
// bit-banged transfer (the DM40's HY3131 MISO). This model keeps outputs,
// external inputs and pull resistors apart:
//   level(pin) = Output: ODR bit; Input/AF: external drive if any, else the
//   pull-up (1) / pull-down or none (0); Analog: 0.
// Registers: MODER OTYPER OSPEEDR PUPDR IDR ODR BSRR LCKR AFRL AFRH BRR(0x28, GD32 BC).
// Connections[n] carries the pin level (for EXTI and for the bit-bang models);
// OnGPIO(n, v) drives pin n externally; Release(n) lets it float again.
// LCKR: the lock key sequence (LKK 1, 0, 1 with the same pin bits, then a
// read) locks the selected pins: writes to their MODER / OTYPER / OSPEEDR /
// PUPDR / AFR fields are ignored until Reset() (a system reset or power-on).
// ODR / BSRR stay writable, as on the chip. opendm40's PA2 entry into the
// FS-loader relies on it (firmware/src/boot.rs loader_pa2).
//
using System;
using System.Collections.Generic;
using System.Collections.ObjectModel;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;

namespace Antmicro.Renode.Peripherals.GPIOPort
{
    public class GD32_GPIO : IDoubleWordPeripheral, IKnownSize, IGPIOReceiver, INumberedGPIOOutput
    {
        public GD32_GPIO(IMachine machine, uint modeResetValue = 0, uint outputSpeedResetValue = 0,
                         uint pullUpPullDownResetValue = 0, int numberOfAFs = 16)
        {
            this.modeReset = modeResetValue;
            this.speedReset = outputSpeedResetValue;
            this.pupdReset = pullUpPullDownResetValue;
            var conns = new Dictionary<int, IGPIO>();
            for(var i = 0; i < 16; i++) conns[i] = new GPIO();
            Connections = new ReadOnlyDictionary<int, IGPIO>(conns);
            Reset();
        }

        public void Reset()
        {
            moder = modeReset; ospeedr = speedReset; pupdr = pupdReset;
            otyper = 0; odr = 0; afrl = 0; afrh = 0; lckr = 0; locked = 0; lockStep = 0;
            for(var i = 0; i < 16; i++) { ext[i] = false; extDriven[i] = false; }
            UpdateLevels();
        }

        public long Size => 0x400;
        public IReadOnlyDictionary<int, IGPIO> Connections { get; }

        public uint ReadDoubleWord(long offset)
        {
            switch(offset)
            {
            case 0x00: return moder;
            case 0x04: return otyper;
            case 0x08: return ospeedr;
            case 0x0C: return pupdr;
            case 0x10: return Idr();
            case 0x14: return odr;
            case 0x18: return 0;
            case 0x1C:
                // The read that completes the key sequence sets LKK.
                if(lockStep == 3) { locked = lckr & 0xFFFF; lckr = LKK | locked; lockStep = 0; }
                return lckr;
            case 0x20: return afrl;
            case 0x24: return afrh;
            case 0x28: return 0;
            default: return 0;
            }
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            switch(offset)
            {
            case 0x00: moder = Keep(moder, value, 2); UpdateLevels(); break;
            case 0x04: otyper = Keep(otyper, value & 0xFFFF, 1); break;
            case 0x08: ospeedr = Keep(ospeedr, value, 2); break;
            case 0x0C: pupdr = Keep(pupdr, value, 2); UpdateLevels(); break;
            case 0x14: odr = value & 0xFFFF; UpdateLevels(); break;
            case 0x18: // BSRR / BOP: reset bits (high half) then set bits (low half) win
                odr = (odr & ~(value >> 16)) | (value & 0xFFFF);
                odr &= 0xFFFF;
                UpdateLevels();
                break;
            case 0x1C: LockWrite(value); break;
            case 0x20: afrl = Keep(afrl, value, 4); break;
            case 0x24: afrh = KeepHigh(afrh, value); break;
            case 0x28: // BRR / BC
                odr &= ~(value & 0xFFFF);
                UpdateLevels();
                break;
            }
        }

        /// External drive of pin `number` (a device, a key, a probe).
        public void OnGPIO(int number, bool value)
        {
            if(number < 0 || number > 15) return;
            ext[number] = value;
            extDriven[number] = true;
            UpdateLevels();
        }

        /// Remove the external drive: the pin follows its pull resistor again.
        public void Release(int number)
        {
            if(number < 0 || number > 15) return;
            extDriven[number] = false;
            UpdateLevels();
        }

        public bool Locked(int pin) => (locked & (1u << pin)) != 0;

        // Locked pins keep their field (w bits per pin, pins 0..32/w-1).
        private uint Keep(uint old, uint value, int w)
        {
            uint m = 0, f = (1u << w) - 1;
            for(var i = 0; i < 32 / w; i++) if(Locked(i)) m |= f << (w * i);
            return (old & m) | (value & ~m);
        }

        // AFRH: pins 8..15, 4 bits each.
        private uint KeepHigh(uint old, uint value)
        {
            uint m = 0;
            for(var i = 0; i < 8; i++) if(Locked(i + 8)) m |= 0xFu << (4 * i);
            return (old & m) | (value & ~m);
        }

        // Key sequence: LKK=1 + pins, LKK=0 + same pins, LKK=1 + same pins,
        // then a read of LCKR. Once locked, LCKR does not change until reset.
        private void LockWrite(uint value)
        {
            if((lckr & LKK) != 0 && locked != 0) return;
            var pins = value & 0xFFFF;
            var k = (value & LKK) != 0;
            if(lockStep == 0 && k) lockStep = 1;
            else if(lockStep == 1 && !k && pins == (lckr & 0xFFFF)) lockStep = 2;
            else if(lockStep == 2 && k && pins == (lckr & 0xFFFF)) lockStep = 3;
            else lockStep = k ? 1 : 0;
            lckr = value & (LKK | 0xFFFF);
        }

        public int Mode(int pin) => (int)((moder >> (2 * pin)) & 3);
        public int Pull(int pin) => (int)((pupdr >> (2 * pin)) & 3);
        public bool Level(int pin) => level[pin];

        private uint Idr()
        {
            uint v = 0;
            for(var i = 0; i < 16; i++) if(level[i]) v |= 1u << i;
            return v;
        }

        private void UpdateLevels()
        {
            for(var i = 0; i < 16; i++)
            {
                bool l;
                switch(Mode(i))
                {
                case 1: l = (odr & (1u << i)) != 0; break; // output
                case 3: l = false; break; // analog
                default: l = extDriven[i] ? ext[i] : Pull(i) == 1; break; // input / AF
                }
                if(l != level[i])
                {
                    level[i] = l;
                    Connections[i].Set(l);
                }
            }
        }

        private readonly uint modeReset, speedReset, pupdReset;
        private uint moder, otyper, ospeedr, pupdr, odr, afrl, afrh, lckr, locked;
        private int lockStep;
        private const uint LKK = 1u << 16;
        private readonly bool[] ext = new bool[16];
        private readonly bool[] extDriven = new bool[16];
        private readonly bool[] level = new bool[16];
    }
}
