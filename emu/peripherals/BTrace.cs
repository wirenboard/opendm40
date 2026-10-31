//
// Code executed while the AD2 stream ISR owns the HY3131 bus
// (hy3131::HANDED != 0), for firmware/tools/region-profile/rprof.py: which
// region B code must stay zero-wait. tlib's block-begin hook
// (TranslationCPU.SetHookAtBlockBegin) is called at the start of every
// executed translation block; while the byte at `handed` is non-zero the
// block's start PC and its instruction count are recorded, each block once,
// as "pc count" lines appended to `path`. A watchpoint on `handed` keeps the
// flag, so the hook itself reads no memory. Off until Arm (the hook is a
// call out of the translated code at every block: the emulator runs slower).
//   sysbus.btrace Arm 0x1000xxxx "/path/run.btrace"
// emu/run.py arms it when DM40_BTRACE names a directory.
//
using System;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.CPU;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public class BTrace : IDoubleWordPeripheral, IKnownSize
    {
        public BTrace(IMachine machine)
        {
            this.machine = machine;
        }

        public void Reset()
        {
        }

        public long Size => 0x100;

        public uint ReadDoubleWord(long offset)
        {
            return 0;
        }

        public void WriteDoubleWord(long offset, uint value)
        {
        }

        public void Arm(ulong handed, string path)
        {
            this.path = path;
            TranslationCPU cpu = null;
            foreach(var c in machine.SystemBus.GetCPUs())
            {
                if(c is TranslationCPU t)
                {
                    cpu = t;
                    break;
                }
            }
            if(cpu == null)
            {
                this.Log(LogLevel.Error, "no translation CPU");
                return;
            }
            Action<ulong, uint> hook = (pc, n) =>
            {
                if(on && seen.Add(pc))
                {
                    System.IO.File.AppendAllText(this.path, string.Format("{0:x} {1}\n", pc, n));
                }
            };
            on = machine.SystemBus.ReadByte(handed) != 0;
            if(on)
            {
                cpu.SetHookAtBlockBegin(hook);
            }
            // HANDED is an AtomicBool (strb), but a word / doubleword store
            // over it (the .bss clear) must count too: one watchpoint per
            // width, on the aligned address, the flag's byte lane taken. The
            // block hook is set only while the flag is: no call per block (no
            // slowdown) outside the stream.
            foreach(var w in new[] { SysbusAccessWidth.Byte, SysbusAccessWidth.Word, SysbusAccessWidth.DoubleWord })
            {
                var size = w == SysbusAccessWidth.Byte ? 1UL : w == SysbusAccessWidth.Word ? 2UL : 4UL;
                var at = handed & ~(size - 1);
                var shift = (int)(8 * (handed - at));
                machine.SystemBus.AddWatchpointHook(at, w, Access.Write, (c, addr, width, value) =>
                {
                    var now = ((value >> shift) & 0xff) != 0;
                    if(now && !on)
                    {
                        cpu.SetHookAtBlockBegin(hook);
                    }
                    else if(!now && on)
                    {
                        cpu.ClearHookAtBlockBegin();
                    }
                    on = now;
                    if(now)
                    {
                        sets++;
                    }
                });
            }
            this.Log(LogLevel.Info, "armed: blocks executed while [0x{0:X8}] != 0 -> {1}", handed, path);
        }

        public ulong Sets => sets;

        private readonly IMachine machine;
        private string path;
        private volatile bool on;
        private ulong sets;
        private readonly System.Collections.Generic.HashSet<ulong> seen = new System.Collections.Generic.HashSet<ulong>();
    }
}
