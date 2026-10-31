//
// Bluetooth SPP/BLE module on UART4 as the DM40 firmware expects it
// (stock/re/analysis/bt_meas_v2_6.h, firmware/src/bt.rs): answers AT lines
// ("AT+SBN<name>" -> OK, "AT+CBN" -> "CBN+<name>", "AT+SLED0"/"AT+RESET" ->
// OK, "AT+RESET" -> a restart, anything else "AT..." -> OK). PC14 ("connected")
// is driven high while a host is attached to the TCP port, LinkDelayMs after
// it connects (its bytes pass at once), and for BootPinMs after each module
// start, which also prints the start-up banner (BootModel). With a port, a TCP server passes the
// firmware's UART bytes to the host and the host's bytes to the firmware, so
// the stock measurement protocol (AF 05 03 ...) and opendm40's SCPI can be used
// from the host; AT lines are still answered locally, while a host is linked
// only the ones that start a line of the firmware's output and that it sends
// linked (OnChar: an "AT" inside a reply is data).
//
// Module power (PC0, bt_meas_v2_6.h section 1: the stock drives it low at the
// BT UART init on the meter page and high when the page is left; high = off is
// the header's guess, confirmed by opendm40 native builds that left PC0 high and
// were unreachable over BT): the module is on only while PC0 is an output
// driven low (the stock boot drives it high first thing, so the reset state
// counts as off). While off it answers no AT line and keeps PC14 low; a host
// connection waits for power (a page attempt) and is refused when none comes
// within PageTimeoutMs of wall time (the host's "Host is down"); a link is
// dropped when the module loses power. PowerModel false = always on.
//
using System;
using System.Collections.Generic;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.UART;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public class BtModule : IDoubleWordPeripheral, IKnownSize
    {
        public BtModule(IMachine machine, IUART uart, IGPIOReceiver connPort = null, int connPin = 14)
        {
            this.machine = machine;
            this.uart = uart;
            this.connPort = connPort;
            this.connPin = connPin;
            uart.CharReceived += OnChar;
            Name = "DM40A";
            Reset();
            machine.ScheduleAction(TimeInterval.FromMilliseconds(WatchMs), Watch);
        }

        public void Reset()
        {
            line.Clear();
            inAt = false;
        }

        public long Size => 0x100;
        public uint ReadDoubleWord(long offset) => 0;
        public void WriteDoubleWord(long offset, uint value) { }

        public string Name { get; set; }
        /// Model the PC0 power pin (default on).
        public bool PowerModel { get; set; } = true;
        /// Host connections refused or dropped because the module was off.
        public ulong Refused { get; private set; }

        /// Model the module start (power-up, AT+RESET): PC14 high for
        /// BootPinMs and the start-up banner at BootTextMs (device 7b1b6d1
        /// trace, device-check doc "f59bac2"): "\0", then at ~150 ms
        /// "BLE03:VER2.1-20240615\0", "CBN:<name>", "CBMAC:<12 hex>",
        /// "CSN:<classic name>", "CSMAC:<12 hex>", "CBSW:1", "CSSW:1" (CR LF
        /// each), PC14 low again at ~650 ms. The MAC digits are placeholders.
        public bool BootModel { get; set; } = true;
        public int BootTextMs { get; set; } = 150;
        public int BootPinMs { get; set; } = 650;
        /// A host link is accepted this long after a start, not sooner
        /// (device: a client that kept retrying linked 0.32 s after power-up,
        /// after the banner, inside the PC14 pulse).
        public int BootLinkMs { get; set; } = 320;
        /// Module starts seen (diagnostics).
        public ulong Boots { get; private set; }

        /// Virtual ms from a host connection to PC14 high. The host's bytes
        /// pass from the connection on: device f59bac2 (device-check doc
        /// "f59bac2" note 4) dropped a line sent up to 0.3 s after the
        /// RFCOMM connect returned while firmware gated lines on PC14, and
        /// answered one sent at 0.5 s. 400 ms models that; 0 = PC14 at once.
        public int LinkDelayMs { get; set; } = 400;

        /// Wall-clock wait of a host connection for module power.
        public int PageTimeoutMs { get; set; } = 30000;

        /// Module on = PC0 an output driven low (GPIOC MODER pin 0 = 01, ODR bit 0 = 0).
        public bool Powered
        {
            get
            {
                if(!PowerModel) return true;
                var moder = machine.SystemBus.ReadDoubleWord(GpiocBase);
                var odr = machine.SystemBus.ReadDoubleWord(GpiocBase + 0x14);
                return (moder & 3) == 1 && (odr & 1) == 0;
            }
        }
        public ulong AtCommands { get; private set; }
        public ulong Lines { get; private set; }

        /// Host side: start a TCP server; bytes are passed both ways.
        public void Listen(int port)
        {
            listener = new TcpListener(IPAddress.Loopback, port);
            listener.Start();
            this.Log(LogLevel.Info, "BT passthrough listening on 127.0.0.1:{0}", port);
            var t = new Thread(AcceptLoop) { IsBackground = true, Name = "bt-accept" };
            t.Start();
        }

        /// Script side: raise/drop the module's "connected" pin (PC14) without a TCP host.
        public void SetConnected(bool on)
        {
            connPort?.OnGPIO(connPin, on);
        }

        /// Script side: inject bytes as if the host sent them (hex string, e.g. "AF0503000000..").
        public void SendHex(string hex)
        {
            var bytes = new List<byte>();
            for(var i = 0; i + 1 < hex.Length; i += 2)
            {
                bytes.Add(Convert.ToByte(hex.Substring(i, 2), 16));
            }
            SendToUart(bytes.ToArray());
        }

        public void SendText(string text)
        {
            SendToUart(Encoding.ASCII.GetBytes(text));
        }

        /// Module -> firmware byte time in virtual microseconds (115200 8N1 =
        /// 86.8 us; 0 = all at once, the model before #123). Each byte is a
        /// timed action, so the emulation quantum may stretch it (inferred).
        /// Only this direction is paced: the firmware's bytes leave at once.
        public int RxByteUs { get; set; } = 87;

        /// Injection goes through a timed action so that a monitor command (also
        /// one issued while the emulation is paused) never blocks on the UART's
        /// receive path; the bytes arrive once the emulation runs, one per
        /// RxByteUs (a 1.5 kB burst takes 130 ms, as on the wire).
        private void SendToUart(byte[] bytes)
        {
            lock(rxQueue)
            {
                foreach(var b in bytes)
                {
                    rxQueue.Enqueue(b);
                }
                if(rxPumping)
                {
                    return;
                }
                rxPumping = true;
            }
            machine.ScheduleAction(TimeInterval.FromMicroseconds(1), RxPump);
        }

        private void RxPump(TimeInterval _)
        {
            while(true)
            {
                byte b;
                lock(rxQueue)
                {
                    if(rxQueue.Count == 0)
                    {
                        rxPumping = false;
                        return;
                    }
                    b = rxQueue.Dequeue();
                }
                uart.WriteChar(b);
                if(RxByteUs > 0)
                {
                    machine.ScheduleAction(TimeInterval.FromMicroseconds((ulong)RxByteUs), RxPump);
                    return;
                }
            }
        }

        // ---- firmware -> module ----
        private void OnChar(byte b)
        {
            TxLog(b);
            if(!Powered)
            {
                line.Clear();
                DropIfOff();
                return;
            }
            // Pass through to the host first.
            var c = client;
            if(c != null)
            {
                try { c.GetStream().WriteByte(b); } catch(Exception) { client = null; }
            }
            // AT parser: lines starting with "AT" terminated by \r\n (or 20 ms idle for
            // the stock's "AT+SBNDM40A" without a terminator). While a host is
            // linked the firmware's bytes are data for it (SCPI replies, LOG:DATA?
            // blocks, stock frames), which may hold "AT" anywhere: an AT line then
            // has to start a line of the firmware's output (after CR / LF or
            // AtGapUs of quiet) and be one of the commands the firmware sends
            // while linked (cmd 07 AT+SBN, cmd 08 AT+CBN; `Complete`). Before
            // this an "AT" inside a reply made the model inject "OK" (logfmttest
            // --bt-port: -113 from the firmware's parser). What the real module
            // does with AT text while linked is not known (INFERRED: it answers
            // the stock's cmd 07 / 08 AT lines, which the stock sends linked).
            var now = (ulong)(machine.ElapsedVirtualTime.TimeElapsed.TotalSeconds * 1e6);
            var lineStart = prevTx == (byte)'\n' || prevTx == (byte)'\r' || now - lastTxUs >= AtGapUs;
            prevTx = b;
            lastTxUs = now;
            if(line.Count == 0 && (b != (byte)'A' || (client != null && !lineStart))) return;
            if(line.Count == 1 && b != (byte)'T') { line.Clear(); return; }
            line.Add(b);
            if(b == (byte)'\n' || b == (byte)'\r')
            {
                Complete();
            }
            else
            {
                idleGen++;
                var g = idleGen;
                machine.ScheduleAction(TimeInterval.FromMilliseconds(20), _ => { if(g == idleGen && line.Count > 2) Complete(); });
            }
        }

        private void Complete()
        {
            var s = Encoding.ASCII.GetString(line.ToArray()).Trim();
            line.Clear();
            if(s.Length < 2) return;
            if(client != null && !(s.StartsWith("AT+SBN") || s.StartsWith("AT+CBN") || s.StartsWith("AT+SLED") || s.StartsWith("AT+RESET")))
            {
                // Linked: data for the host that only looks like AT (see OnChar).
                return;
            }
            AtCommands++;
            this.Log(LogLevel.Debug, "AT command: {0}", s);
            string reply;
            if(s.StartsWith("AT+SBN"))
            {
                Name = s.Substring(6);
                reply = "OK";
            }
            else if(s.StartsWith("AT+CBN"))
            {
                reply = "CBN+" + Name;
            }
            else if(s.StartsWith("AT+RESET") && BootModel)
            {
                // No "OK" on the device: the module restarts (banner, PC14).
                machine.ScheduleAction(TimeInterval.FromMilliseconds(17), _ => Boot());
                return;
            }
            else
            {
                reply = "OK";
            }
            var r = Encoding.ASCII.GetBytes(reply + "\r\n");
            // Reply after a short module delay so the firmware sees a separate answer.
            machine.ScheduleAction(TimeInterval.FromMilliseconds(5), _ => SendToUart(r));
        }

        /// Power watch (PC0 is read, not signalled): a start on every power-up.
        private void Watch(TimeInterval _)
        {
            var on = Powered;
            if(on && !wasPowered)
            {
                Boot();
            }
            else if(!on && wasPowered)
            {
                bootGen++;
                linkReady = false;
                if(client == null) connPort?.OnGPIO(connPin, false);
            }
            wasPowered = on;
            machine.ScheduleAction(TimeInterval.FromMilliseconds(WatchMs), Watch);
        }

        private void Boot()
        {
            if(!BootModel) return;
            Boots++;
            var gen = ++bootGen;
            linkReady = false;
            machine.ScheduleAction(TimeInterval.FromMilliseconds((ulong)BootLinkMs), _ => { if(gen == bootGen) linkReady = true; });
            connPort?.OnGPIO(connPin, true);
            SendToUart(new byte[] { 0 });
            var text = "\0BLE03:VER2.1-20240615\0\r\nCBN:" + Name + "\r\nCBMAC:XXXXXXXXXXXX\r\nCSN:C-1-ATK-DM40-CALI\r\n"
                + "CSMAC:XXXXXXXXXXXX\r\nCBSW:1\r\nCSSW:1\r\n";
            var bytes = Encoding.ASCII.GetBytes(text);
            machine.ScheduleAction(TimeInterval.FromMilliseconds((ulong)BootTextMs), _ => { if(gen == bootGen && Powered) SendToUart(bytes); });
            machine.ScheduleAction(TimeInterval.FromMilliseconds((ulong)BootPinMs), _ => { if(gen == bootGen && client == null) connPort?.OnGPIO(connPin, false); });
        }

        /// The module lost power: the link drops (PC14 low).
        private void DropIfOff()
        {
            var c = client;
            if(c == null) return;
            Refused++;
            client = null;
            try { c.Close(); } catch(Exception) { }
            connPort?.OnGPIO(connPin, false);
        }

        private void TxLog(byte b)
        {
            if(b == (byte)'\n' || txLine.Count > 200)
            {
                this.Log(LogLevel.Debug, "fw -> host: {0}", Encoding.ASCII.GetString(txLine.ToArray()).TrimEnd('\r'));
                txLine.Clear();
            }
            else
            {
                txLine.Add(b);
            }
        }

        // ---- host socket ----
        private void AcceptLoop()
        {
            while(true)
            {
                TcpClient c;
                try { c = listener.AcceptTcpClient(); } catch(Exception) { return; }
                var waited = 0;
                while(!(Powered && (linkReady || !BootModel)) && waited < PageTimeoutMs)
                {
                    Thread.Sleep(10);
                    waited += 10;
                }
                if(!Powered)
                {
                    Refused++;
                    this.Log(LogLevel.Info, "BT host refused: module off (PC0 not driven low)");
                    try { c.Close(); } catch(Exception) { }
                    continue;
                }
                this.Log(LogLevel.Info, "BT host connected");
                client = c;
                // The host's bytes pass at once; PC14 rises LinkDelayMs
                // (virtual) later, as on the device.
                var gen = ++linkGen;
                if(LinkDelayMs <= 0)
                {
                    connPort?.OnGPIO(connPin, true);
                }
                else
                {
                    machine.ScheduleAction(TimeInterval.FromMilliseconds((ulong)LinkDelayMs), _ => { if(gen == linkGen && client == c) connPort?.OnGPIO(connPin, true); });
                }
                var buf = new byte[256];
                try
                {
                    var st = c.GetStream();
                    int n;
                    while((n = st.Read(buf, 0, buf.Length)) > 0)
                    {
                        if(!Powered)
                        {
                            Refused++;
                            this.Log(LogLevel.Info, "BT link dropped: module powered off");
                            break;
                        }
                        var copy = new byte[n];
                        Array.Copy(buf, copy, n);
                        SendToUart(copy); // delivered on the emulation thread
                    }
                }
                catch(Exception) { }
                client = null;
                linkGen++;
                connPort?.OnGPIO(connPin, false);
                this.Log(LogLevel.Info, "BT host disconnected");
            }
        }

        private const ulong GpiocBase = 0x40020800;
        private readonly IMachine machine;
        private readonly IUART uart;
        private readonly IGPIOReceiver connPort;
        private readonly int connPin;
        private readonly List<byte> line = new List<byte>();
        private readonly Queue<byte> rxQueue = new Queue<byte>();
        private bool rxPumping;
        private readonly List<byte> txLine = new List<byte>();
        private bool inAt;
        private byte prevTx = (byte)'\n';
        private ulong lastTxUs;
        /// Firmware TX quiet that starts a new line for the AT parser while linked.
        private const ulong AtGapUs = 2000;
        private int idleGen;
        private volatile int linkGen;
        private int bootGen;
        private volatile bool linkReady;
        private bool wasPowered;
        private const int WatchMs = 5;
        private TcpListener listener;
        private volatile TcpClient client;
    }
}
