//
// DM40 analog front end between the banana jacks (V-ohm, COM, A) and the HY3131.
//
// The user describes what is CONNECTED to the jacks (sources, loads, a resistor,
// a capacitor, a diode, a thermocouple, nothing), the firmware drives the relays
// (PA9/PA10 latching pair: V/A path or the ohm/mV network, PE0/PE1 shunt select,
// PC7 frequency mode) and configures the HY3131's own switch matrix; this class
// answers the chip model's questions about the resulting node voltages, so a
// fixed connection behaves like on the real meter when the function changes
// (ohm on a powered circuit, current mode with nothing in A, a source in the
// wrong jack ...). No Renode dependency; the HY3131 peripheral feeds the GPIO
// levels in. Board facts: stock/re/analysis/hy3131_v2_6.h (relays, shunts,
// divider scale), the datasheet typical circuit (10M divider, 900k/100k diode
// divider), firmware/src/dmm.rs relay_pa / relay_pe / pc7. The V-jack network
// constants are the stock's nominal scale factors; the DM40 netlist itself is
// not known (emu/hy3131/README.md).
//
using System;
using System.Collections.Generic;
using System.Globalization;
using System.Text;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    /// A voltage source: DC + AC (RMS) at Hz with a shape, through Rs ohm.
    public class VoltageSource
    {
        public double Dc, Ac, Hz = 50, Duty = 0.5, Rs = 0;
        public string Shape = "sine";
    }

    /// A current source: DC + AC (RMS), compliance voltage (what it can drive), Ph: phase of
    /// the AC part relative to the V source's AC part, degrees (positive = current lags).
    public class CurrentSource
    {
        public double Dc, Ac, Hz = 50, Duty = 0.5, Compliance = 10, Ph = 0;
        public string Shape = "sine";
    }

    public class Dm40Frontend
    {
        // ---- board constants ----
        public const double RinV = 10e6;              // V jack input resistance (divider / mV path)
        public double LeadOhm = 0.0;                  // lead / contact resistance in series with the ohm path (set from the cal offset)
        public const double RFuse = 0.05;             // A jack fuse + wiring
        public static readonly double[] ShuntOhm = { 0.01, 0.5, 47.5 }; // PE: 0 = A, 1 = mA, 2 = none (uA chain)
        public const double TcRs = 10.0;              // thermocouple lead resistance

        // ---- MCU pin state ----
        /// Latching relay: 0 = V/A path (divider, shunts), 1 = ohm / mV / cap / diode / Hz / temperature network.
        public int RelayPa = 0;
        /// Relay changes ohm network -> V/A path so far (the chip model's `ohmexit:`).
        public int PaExits;
        /// Shunt select: 0 = A (10 mohm), 1 = mA (0.5 ohm), 2 = none (47.5 ohm uA chain).
        public int ShuntSel = 2;
        public bool Pc7;
        private bool pa9, pa10, pe0, pe1;

        public void SetPin(string pin, bool level)
        {
            switch(pin)
            {
            case "PA9": if(level && !pa9) RelayPa = 1; pa9 = level; break;   // pulse -> ohm/mV network
            case "PA10": if(level && !pa10) { if(RelayPa == 1) PaExits++; RelayPa = 0; } pa10 = level; break; // pulse -> V/A path
            case "PE0": pe0 = level; UpdateShunt(); break;
            case "PE1": pe1 = level; UpdateShunt(); break;
            case "PC7": Pc7 = level; break;
            }
        }

        private void UpdateShunt() { ShuntSel = pe0 ? 0 : pe1 ? 1 : 2; }
        public double ShuntR => ShuntOhm[ShuntSel];

        // ---- what is connected ----
        public VoltageSource VSrc;        // between V and COM
        public CurrentSource ISrcV;       // current source between V and COM (wrong jack)
        public double R = double.PositiveInfinity; // resistor between V and COM (0 = short)
        public double C = 0;              // capacitor between V and COM (F)
        /// A current-limited path between V and COM (A, 0 = none; "isink:2.1e-7"): it holds the node at
        /// 0 V while that takes at most ISink, else it takes ISink. Device 1a11eda (the calibrator's
        /// disabled output, INFERRED: such a path, e.g. a leakage / bias current of its output stage):
        /// RES 6 Mohm read 0.104 Mohm (its ~0.215 uA test current nearly all taken), 600 kohm OL
        /// (a tenth of its ~2.17 uA taken).
        public double ISink = 0;
        public double DiodeVf = double.NaN; // diode between V and COM, anode at V unless DiodeRev
        public bool DiodeRev;
        public bool Tc;                   // thermocouple (type K) between V and COM
        public double TcTemp = 25;        // its hot junction, C
        public double TcMvOverride = double.NaN;
        public bool TcOpen;               // thermocouple selected but not connected (open)
        public double CjTemp = 25;        // reference junction (where the thermocouple meets the jacks), C
        public double DieTemp = 25;       // HY3131 die temperature (what the internal sensor reports), C
        public bool DieFollowsCj = true;  // die temperature not set explicitly: equal to CjTemp
        public CurrentSource ISrc;        // between A and COM
        public VoltageSource VSrcA;       // voltage source between A and COM (wrong jack)
        public double RLoadA = double.PositiveInfinity; // resistor between A and COM
        public bool LeadVA;               // a test lead from V/ohm to A (the opendm40 fuse check)
        public bool FuseOpen;             // the A-jack fuse is blown (a meter property: kept by "open")
        /// The floating mV input after a source on V-COM goes off (opt-in, "mvhold:V"; 0 = none): the
        /// mV rows read MvHoldV DC on the jack, and PB1 picks up MvHoldPb1Mv mV DC ("mvholdpb1:mV"), until
        /// the next source on V-COM. "mvholdrows:" which mV rows see the level: 1 DC mV, 2 AC mV, 3 both
        /// (the default). Device 185c066 (AC+DC mV, manual 600 mV): after the 0 / 9.5 V square or 5 V DC
        /// was switched off (the calibrator's output relay opens) the readout stayed OL and
        /// AJACK,AD1PK_PB1 came 0.45-0.9 s later in 8 of 13 runs, held while the input was open. The
        /// charge, which rows see it and the PB1 pickup are INFERRED (phenomenological). Kept by "open".
        public double MvHoldV, MvHoldPb1Mv;
        public int MvHoldRows = 3;
        /// The hold is in force (a source went off with MvHoldV set).
        public bool MvHeld;
        /// MvHoldV for the AC mV (`ac`) or DC mV row, NaN when not held there.
        public double MvHoldFor(bool ac) => MvHeld && (MvHoldRows & (ac ? 2 : 1)) != 0 ? MvHoldV : double.NaN;

        public double Die => DieFollowsCj ? CjTemp : DieTemp;

        public void Clear()
        {
            VSrc = null; ISrcV = null; R = double.PositiveInfinity; C = 0; DiodeVf = double.NaN; DiodeRev = false;
            Tc = false; TcOpen = false; TcMvOverride = double.NaN;
            ISrc = null; VSrcA = null; RLoadA = double.PositiveInfinity;
            LeadVA = false;
            ISink = 0;
            vcap = 0;
        }

        // ---- time-dependent state ----
        private double vcap;      // capacitor voltage on the ohm path (charged by the ohm source)
        private double lastNow;

        /// Advance the capacitor charge on the ohm path. vs / rref: the active ohm source (0 = none).
        public void Update(double now, double vs, double rref)
        {
            var dt = now - lastNow;
            lastNow = now;
            if(dt <= 0) return;
            if(C > 0 && rref > 0 && RelayPa == 1)
            {
                // Thevenin of the source (vs, rref) with the parallel external network
                double gth, ith;
                LinearNorton(out gth, out ith);
                var g = gth + 1 / rref;
                var vinf = (ith + vs / rref) / g;
                var tau = C / g;
                vcap += (vinf - vcap) * (1 - Math.Exp(-dt / tau));
            }
            else if(C > 0)
            {
                vcap *= Math.Exp(-dt / 0.05);
            }
        }

        // ---- queries ----
        /// Voltage at the V jack seen by the HY3131 input (path 0 = divider / V path, 1 = ohm-mV network
        /// including the thermocouple). The jack is open when the relay routes it elsewhere.
        public void VJack(int path, out double dc, out double ac, out double hz, out string shape, out double duty)
        {
            dc = 0; ac = 0; hz = 50; shape = "sine"; duty = 0.5;
            if(RelayPa != path) return;
            double gth, ith;
            LinearNorton(out gth, out ith);
            var g = gth + 1 / RinV;
            dc = Sink(ith / g, g);
            // forward diode clamps the node
            if(!double.IsNaN(DiodeVf))
            {
                var lim = DiodeRev ? -DiodeVf : DiodeVf;
                if((!DiodeRev && dc > lim) || (DiodeRev && dc < lim)) dc = lim;
            }
            // AC: the sources' AC parts through the same divider (same frequency assumed)
            double iac = 0;
            if(VSrc != null && VSrc.Ac > 0) { iac += VSrc.Ac / Math.Max(VSrc.Rs, 1e-3); hz = VSrc.Hz; shape = VSrc.Shape; duty = VSrc.Duty; }
            if(ISrcV != null && ISrcV.Ac > 0) { iac += ISrcV.Ac; hz = ISrcV.Hz; shape = ISrcV.Shape; duty = ISrcV.Duty; }
            ac = iac / g;
            if(ISrcV != null)
            {
                // a current source into 10 M: limited by its compliance
                dc = Math.Max(-ISrcV.Compliance, Math.Min(ISrcV.Compliance, dc));
                ac = Math.Min(ac, ISrcV.Compliance);
            }
        }

        /// Norton equivalent of the linear elements between V and COM (conductance, short-circuit current).
        private void LinearNorton(out double g, out double i)
        {
            g = 0; i = 0;
            if(VSrc != null) { var rs = Math.Max(VSrc.Rs, 1e-3); g += 1 / rs; i += VSrc.Dc / rs; }
            if(!double.IsInfinity(R)) { var r = Math.Max(R + LeadOhm, 1e-3); g += 1 / r; }
            if(Tc && !TcOpen) { g += 1 / TcRs; i += TcEmf() / 1000.0 / TcRs; }
            if(ISrcV != null) i += ISrcV.Dc;
            // lead V -> A: V/ohm sees the A jack's fuse + the relay-selected shunt chain to COM
            if(LeadVA && !FuseOpen) g += 1 / (ShuntR + RFuse);
        }

        /// Current through the A jack shunt chain (A), DC and AC RMS.
        public void ACurrent(out double dc, out double ac, out double hz, out string shape, out double duty)
        {
            dc = 0; ac = 0; hz = 50; shape = "sine"; duty = 0.5;
            if(FuseOpen) return; // nothing reaches the shunts
            var rsh = ShuntR + RFuse;
            if(ISrc != null)
            {
                dc = ISrc.Dc; ac = ISrc.Ac; hz = ISrc.Hz; shape = ISrc.Shape; duty = ISrc.Duty;
                var imax = ISrc.Compliance / rsh;
                dc = Math.Max(-imax, Math.Min(imax, dc));
                ac = Math.Min(ac, imax);
            }
            if(VSrcA != null)
            {
                var r = Math.Max(VSrcA.Rs, 1e-3) + rsh;
                dc += VSrcA.Dc / r; ac += VSrcA.Ac / r; hz = VSrcA.Hz; shape = VSrcA.Shape; duty = VSrcA.Duty;
            }
        }

        /// DC operating point of the V jack with the chip's ohm source (vs volts through rref ohm):
        /// vx = node voltage, vref = drop across rref. Open jack (relay elsewhere, nothing connected): vx = vs.
        public void OhmNode(double vs, double rref, out double vx, out double vref)
        {
            if(RelayPa != 1) { vx = vs; vref = 0; return; }
            double gth, ith;
            LinearNorton(out gth, out ith);
            var g = gth + 1 / rref;
            var v = (ith + vs / rref) / g;
            if(!double.IsNaN(DiodeVf))
            {
                var lim = DiodeRev ? -DiodeVf : DiodeVf;
                if((!DiodeRev && v > lim) || (DiodeRev && v < lim)) v = lim;
            }
            v = Sink(v, g);
            if(C > 0 && double.IsInfinity(R) && VSrc == null && ISrcV == null && !Tc && double.IsNaN(DiodeVf))
            {
                v = vcap; // charging capacitor
            }
            vx = v;
            vref = vs - v;
        }

        /// The node with the current-limited path to COM (ISink): v0 is the node without it, g the
        /// conductance it sees. The path holds the node at 0 V while that takes at most ISink, else it
        /// takes ISink (toward 0 V, whatever the sign).
        private double Sink(double v0, double g)
        {
            if(!(ISink > 0) || g <= 0) return v0;
            var i0 = Math.Abs(v0) * g;
            return i0 <= ISink ? 0 : v0 - Math.Sign(v0) * ISink / g;
        }

        /// Node voltage with a current source i (A) driven into the V jack (diode test), limited to vmax.
        public double DiodeNode(double i, double vmax)
        {
            if(RelayPa != 1) return vmax;
            double gth, ith;
            LinearNorton(out gth, out ith);
            double v;
            if(gth <= 0) v = vmax; // open
            else v = (ith + i) / gth;
            if(!double.IsNaN(DiodeVf))
            {
                var lim = DiodeRev ? -DiodeVf : DiodeVf;
                if(!DiodeRev && v > lim) v = lim;
                if(DiodeRev && v < lim) v = lim;
                if(DiodeRev && gth <= 0) v = vmax; // reverse biased, nothing else: open
            }
            return Math.Max(-vmax, Math.Min(vmax, v));
        }

        /// Can the RC relaxation oscillator (resistor r from the chip) run on the connected network?
        /// Returns the capacitance it charges, 0 when nothing oscillates (a low resistance, a source).
        public double CapOscC(double r)
        {
            if(RelayPa != 1) return 0;
            if(VSrc != null || ISrcV != null || Tc || !double.IsNaN(DiodeVf)) return 0;
            if(!double.IsInfinity(R) && R < 3 * r) return 0;
            return C;
        }

        /// Capacitance seen by the constant-current ramp (0 = the ramp cannot run).
        public double CapRampC(double iRamp, double vthr)
        {
            if(RelayPa != 1) return 0;
            if(VSrc != null || ISrcV != null || Tc || !double.IsNaN(DiodeVf)) return 0;
            if(!double.IsInfinity(R) && iRamp * R < vthr) return 0; // the leak keeps the node below the threshold
            return C;
        }

        /// Thermocouple EMF (mV) relative to the reference junction, NaN when none is connected.
        public double TcEmf()
        {
            if(!Tc || TcOpen) return double.NaN;
            if(!double.IsNaN(TcMvOverride)) return TcMvOverride;
            return TypeK.Emf(TcTemp) - TypeK.Emf(CjTemp);
        }

        public bool TcConnectedOpen => Tc && TcOpen;

        // ---- spec parser ----
        /// Connection spec. Elements accumulate; `open` clears everything. Keys:
        ///   vsrc:DC[,ac=RMS,f=Hz,shape=sine|square|triangle,duty=D,rs=OHM]   voltage source V-COM
        ///   isrc:DC[,ac=,f=,shape=,duty=,compl=V,ph=DEG]                   current source A-COM (ph: lag behind the V source)
        ///   pow:V@Hz,i=I[,pf=PF|ph=DEG,dc=VDC,idc=IDC,f=,shape=]          V source + current source at the same frequency
        ///   r:OHM | c:F | diode:VF[,rev=1] | tc:C | cj:C | die:C | short | open
        ///   avsrc:V[,rs=] (voltage source in the A jack) | aload:OHM | isrcv:A (current source in V-COM)
        ///   valead (a lead from V/ohm to A: the fuse check) | fuseopen (the A fuse blown; "open" keeps it)
        ///   vsrc:none, isrc:none, r:none ... remove one element
        /// Aliases of the older per-function form: dcv:V, acv:RMS@Hz[,dc=,shape=,duty=], hz:F[,duty=,amp=],
        ///   dca:A, aca:RMS@Hz[,dc=], ohm:OHM, cap:F, temp:C[,cj=], tcmv:MV, tcopen, chip:C.
        /// Model options (returned to the caller): noise:, cal:, beat:, powmix:, ad1mix:, swtr:, swtau:, swhz:, powvg:, powz:, op1vos:, ad2dcg:, mvclamp:, mvpb1:, contbump: ...
        public List<KeyValuePair<string, double>> Apply(string spec)
        {
            var options = new List<KeyValuePair<string, double>>();
            foreach(var part in spec.Split(';'))
            {
                var s = part.Trim();
                if(s.Length == 0) continue;
                var kv = s.Split(new[] { ':' }, 2);
                var key = kv[0].Trim().ToLowerInvariant();
                var args = kv.Length > 1 ? kv[1].Split(',') : new string[0];
                var opts = new Dictionary<string, string>();
                string main = null;
                foreach(var a in args)
                {
                    var t = a.Trim();
                    if(t.Length == 0) continue;
                    var eq = t.IndexOf('=');
                    if(eq > 0) opts[t.Substring(0, eq).Trim().ToLowerInvariant()] = t.Substring(eq + 1).Trim();
                    else if(main == null) main = t;
                }
                var none = main != null && (main.ToLowerInvariant() == "none" || main.ToLowerInvariant() == "off");
                double v = 0, hz = double.NaN;
                if(main != null && !none)
                {
                    var at = main.IndexOf('@');
                    if(at >= 0) { hz = D(main.Substring(at + 1)); main = main.Substring(0, at); }
                    v = D(main);
                }
                // the older per-function keys describe the whole connection: they replace it
                if(Exclusive.Contains(key)) Clear();
                switch(key)
                {
                case "open":
                    if(VSrc != null && MvHoldV > 0) MvHeld = true;
                    Clear();
                    break;
                case "short":
                    R = 0; C = 0; DiodeVf = double.NaN; VSrc = null; ISrcV = null; Tc = false;
                    break;
                case "vsrc": case "dcv": case "acv": case "hz":
                    if(none) { MvHeld |= VSrc != null && MvHoldV > 0; VSrc = null; break; }
                    MvHeld = false;
                    VSrc = new VoltageSource();
                    if(key == "dcv") { VSrc.Dc = v; VSrc.Ac = Opt(opts, "ac", 0); }
                    else if(key == "acv") { VSrc.Ac = v; VSrc.Dc = Opt(opts, "dc", 0); }
                    else if(key == "hz") { VSrc.Ac = Opt(opts, "amp", 1.0); VSrc.Dc = Opt(opts, "dc", 0); hz = v; VSrc.Shape = "square"; }
                    else { VSrc.Dc = v; VSrc.Ac = Opt(opts, "ac", 0); }
                    if(!double.IsNaN(hz)) VSrc.Hz = hz;
                    if(opts.ContainsKey("f")) VSrc.Hz = D(opts["f"]);
                    if(opts.ContainsKey("shape")) VSrc.Shape = opts["shape"].ToLowerInvariant();
                    VSrc.Duty = Opt(opts, "duty", 0.5);
                    VSrc.Rs = Opt(opts, "rs", 0);
                    if(opts.ContainsKey("noise")) options.Add(new KeyValuePair<string, double>("vnoise", D(opts["noise"])));
                    break;
                case "isrc": case "dca": case "aca":
                    if(none) { ISrc = null; break; }
                    ISrc = new CurrentSource();
                    if(key == "aca") { ISrc.Ac = v; ISrc.Dc = Opt(opts, "dc", 0); }
                    else { ISrc.Dc = v; ISrc.Ac = Opt(opts, "ac", 0); }
                    if(!double.IsNaN(hz)) ISrc.Hz = hz;
                    if(opts.ContainsKey("f")) ISrc.Hz = D(opts["f"]);
                    if(opts.ContainsKey("shape")) ISrc.Shape = opts["shape"].ToLowerInvariant();
                    ISrc.Duty = Opt(opts, "duty", 0.5);
                    ISrc.Compliance = Opt(opts, "compl", 10);
                    ISrc.Ph = Opt(opts, "ph", 0);
                    break;
                case "pow":
                {
                    // power meter shorthand: V (AC rms + dc) on V-COM, I (AC rms + idc) into A at the
                    // same frequency, the current lagging by ph (or acos(pf): pf < 0 -> P < 0)
                    if(none) { VSrc = null; ISrc = null; break; }
                    VSrc = new VoltageSource { Ac = v, Dc = Opt(opts, "dc", 0) };
                    ISrc = new CurrentSource { Ac = Opt(opts, "i", 0), Dc = Opt(opts, "idc", 0) };
                    if(opts.ContainsKey("f")) hz = D(opts["f"]);
                    if(!double.IsNaN(hz)) { VSrc.Hz = hz; ISrc.Hz = hz; }
                    if(opts.ContainsKey("shape")) VSrc.Shape = ISrc.Shape = opts["shape"].ToLowerInvariant();
                    ISrc.Ph = opts.ContainsKey("ph") ? D(opts["ph"])
                            : Math.Acos(Math.Max(-1, Math.Min(1, Opt(opts, "pf", 1)))) * 180 / Math.PI;
                    break;
                }
                case "isrcv":
                    if(none) { ISrcV = null; break; }
                    ISrcV = new CurrentSource { Dc = v, Ac = Opt(opts, "ac", 0), Compliance = Opt(opts, "compl", 10) };
                    if(!double.IsNaN(hz)) ISrcV.Hz = hz;
                    break;
                case "avsrc":
                    if(none) { VSrcA = null; break; }
                    VSrcA = new VoltageSource { Dc = v, Ac = Opt(opts, "ac", 0), Rs = Opt(opts, "rs", 0) };
                    if(!double.IsNaN(hz)) VSrcA.Hz = hz;
                    break;
                case "valead":
                    LeadVA = !none;
                    break;
                case "fuseopen":
                    FuseOpen = !none;
                    break;
                case "aload":
                    RLoadA = none ? double.PositiveInfinity : v;
                    break;
                case "isink":
                    ISink = none ? 0 : v;
                    break;
                case "r": case "ohm":
                    R = none ? double.PositiveInfinity : v;
                    break;
                case "c": case "cap":
                    C = none ? 0 : v;
                    break;
                case "diode":
                    if(none) { DiodeVf = double.NaN; break; }
                    DiodeVf = v; DiodeRev = Opt(opts, "rev", 0) != 0;
                    break;
                case "tc": case "temp":
                    if(none) { Tc = false; break; }
                    Tc = true; TcOpen = false; TcTemp = v; TcMvOverride = double.NaN;
                    if(opts.ContainsKey("cj")) CjTemp = D(opts["cj"]);
                    if(opts.ContainsKey("die")) { DieTemp = D(opts["die"]); DieFollowsCj = false; }
                    break;
                case "tcmv":
                    Tc = true; TcOpen = false; TcMvOverride = v;
                    break;
                case "tcopen":
                    Tc = true; TcOpen = true;
                    break;
                case "cj":
                    CjTemp = v;
                    break;
                case "die": case "chip":
                    if(none) { DieFollowsCj = true; break; }
                    DieTemp = v; DieFollowsCj = false;
                    break;
                case "mvhold": MvHoldV = v; if(v <= 0) MvHeld = false; break;
                case "mvholdpb1": MvHoldPb1Mv = v; break;
                case "mvholdrows": MvHoldRows = (int)v; break;
                case "noise": case "cal": case "beat": case "powmix": case "ad1mix": case "ohmexit": case "ohmexitlo": case "ohmexitrst": case "ohmtau": case "capda": case "capdatau": case "capasym": case "capcmpw": case "capvavg": case "swtr": case "swtau": case "swhz": case "ctsettle": case "ctretain": case "ct0dead": case "powvg": case "powz": case "op1vos": case "ad2dcg":
                case "n7": case "n7g": case "n7a": case "ad1acg": case "ad3sh": case "hpftau": case "rmsfill": case "acarrive": case "acaoff": case "rmsbig": case "rmsbigp":
                case "agndz": case "lpfcreep": case "creeptau": case "creepord": case "lpfn":
                case "pb5drift": case "pb5tau": case "acastep": case "acastepms": case "acasteptau": case "ad1vos": case "ad1ug": case "ad1chopjit": case "r22part": case "r22off": case "r22n": case "r22win": case "chophalf": case "chopskew": case "vjblind": case "senseneg": case "sensetau":
                case "mvclamp": case "mvpb1": case "contbump": case "contbumptau":
                    options.Add(new KeyValuePair<string, double>(key, v));
                    break;
                default:
                    throw new ArgumentException("unknown connection key '" + key + "'");
                }
            }
            return options;
        }

        private static readonly HashSet<string> Exclusive = new HashSet<string> { "dcv", "acv", "hz", "dca", "aca", "ohm", "cap", "temp", "tcmv", "tcopen", "short" };

        private static double D(string s) => double.Parse(s.Trim(), NumberStyles.Float, CultureInfo.InvariantCulture);
        private static double Opt(Dictionary<string, string> o, string k, double dflt) => o.ContainsKey(k) ? D(o[k]) : dflt;

        /// The connection as a spec string (for the GUI / state line).
        public string Connection()
        {
            var sb = new StringBuilder();
            var ci = CultureInfo.InvariantCulture;
            if(VSrc != null) sb.AppendFormat(ci, "vsrc:{0:G6},ac={1:G6},f={2:G6},shape={3},duty={4:G4},rs={5:G6};", VSrc.Dc, VSrc.Ac, VSrc.Hz, VSrc.Shape, VSrc.Duty, VSrc.Rs);
            if(ISrcV != null) sb.AppendFormat(ci, "isrcv:{0:G6},ac={1:G6};", ISrcV.Dc, ISrcV.Ac);
            if(!double.IsInfinity(R)) sb.AppendFormat(ci, "r:{0:G6};", R);
            if(ISink > 0) sb.AppendFormat(ci, "isink:{0:G6};", ISink);
            if(C > 0) sb.AppendFormat(ci, "c:{0:G6};", C);
            if(!double.IsNaN(DiodeVf)) sb.AppendFormat(ci, "diode:{0:G6}{1};", DiodeVf, DiodeRev ? ",rev=1" : "");
            if(Tc) sb.AppendFormat(ci, TcOpen ? "tcopen;" : "tc:{0:G6};", TcTemp);
            sb.AppendFormat(ci, "cj:{0:G6};die:{1:G6};", CjTemp, Die);
            if(ISrc != null) sb.AppendFormat(ci, "isrc:{0:G6},ac={1:G6},f={2:G6}{3};", ISrc.Dc, ISrc.Ac, ISrc.Hz, ISrc.Ph != 0 ? string.Format(ci, ",ph={0:G6}", ISrc.Ph) : "");
            if(VSrcA != null) sb.AppendFormat(ci, "avsrc:{0:G6};", VSrcA.Dc);
            if(!double.IsInfinity(RLoadA)) sb.AppendFormat(ci, "aload:{0:G6};", RLoadA);
            if(LeadVA) sb.Append("valead;");
            if(FuseOpen) sb.Append("fuseopen;");
            return sb.ToString();
        }

        public string PinState()
        {
            return string.Format("pa={0} shunt={1} pc7={2}", RelayPa, new[] { "A", "mA", "uA" }[ShuntSel], Pc7 ? 1 : 0);
        }
    }
}
