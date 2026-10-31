# SCPI command reference

opendm40 accepts SCPI commands from a computer, so a script can read the
multimeter, set it up, read scope waveforms, drive the generator and fetch
files. This page lists every command. How to connect (drivers, cable,
Bluetooth pairing, PyVISA, sigrok) is in
[the user manual, Remote control and connecting from a computer](README.md#remote-control-and-connecting-from-a-computer).

## Links

| Link | What it is | Notes |
|---|---|---|
| USB | USBTMC (USB488) interface of the USB cable connection | All commands. |
| Bluetooth | The meter's classic Bluetooth serial port (SPP, **C-1-ATK-DM40-CALI**); the phone-app protocol shares it | Works while Bluetooth is on in the meter's settings. Some commands are refused; see [Bluetooth differences](#bluetooth-differences). |

Each link has its own session: its own error queue and its own waiting
`READ?`. The meter's settings are shared: a function set over Bluetooth is
the function USB reads, and the same as one set by touch.

## Message rules

- **Case and forms.** Commands are not case-sensitive. Each part of a header
  is accepted in its short form (the capital letters in this reference,
  `MEAS`) or its long form (`MEASURE`), nothing in between. Parts written all
  in capitals here (`DBG:PING`, `RUN`) have one form only.
- **Optional parts** are in brackets: `[SENSe:]VOLTage:RANGe` is
  `VOLT:RANG` or `SENS:VOLT:RANG`. A leading `:` is allowed.
- **Parameters** follow the header after a space; several parameters are
  separated by commas: `CONF:VOLT 60,MAX`.
- **Queries** end in `?`. `[?]` in this reference means the command sets a
  value and the same header with `?` reads it back.
- **Several commands in one message** are separated by `;`. Each command is a
  full header from the root (`SOUR:FREQ 1000;SOUR:VOLT 2`, not
  `SOUR:FREQ 1000;VOLT 2`). The commands run in order. The replies to the
  queries come back as one response, in order, separated by `;`.
- **Termination.** Over USB a message is one USBTMC transfer; a `\n` at its
  end is optional. Over Bluetooth a message is one line ending in `\n` or
  `\r` (CR LF is fine). Every response ends with `\n`, also after a binary
  block.
- **Sizes.** A message is at most 256 bytes. One command (header and its
  parameters) is at most 64 characters; a longer one is refused with
  `Syntax error`. A joined text response is at most
  512 bytes; a reply that does not fit is dropped and `Out of memory` is
  queued.
- **Binary blocks.** Some queries answer with an IEEE 488.2 definite-length
  block: `#`, one digit n, n digits of length, the data, then `\n`
  (`#3400` followed by 400 bytes). A block query must be the only query in
  its message; otherwise it is refused with `Query error`. Exception: the
  400-byte `WAVeform:DATA?` (SCReen, BYTE) joins other replies.
- **Numbers in replies** are in SCPI NR3 with 7 significant digits:
  `+1.234567E-03`, in SI units (V, A, ohm, F, Hz, s, W). Two special values:
  `+9.90000000E+37` is an overload (OL on the screen), `+9.91000000E+37`
  means "no value" (SCPI NaN: no reading yet, or not measured).
- **Numbers in parameters** are plain or in exponent form: `0.001`, `1E-3`,
  `-2.5`, optionally with a suffix (IEEE 488.2, not case-sensitive, a space
  before it is allowed): a multiplier `G` = 10^9, `MA` = 10^6, `K` = 10^3,
  `M` = 10^-3, `U` = 10^-6, `N` = 10^-9, `P` = 10^-12, then the
  parameter's unit if it has one (`V`, `A`, `OHM`, `HZ`, `S`, `PCT`):
  `1m` and `1M` are 0.001, `1MA` is 1 000 000, `500MV` is 0.5 V,
  `100US` is 100 µs. `MHZ` and `MOHM` are the exceptions: megahertz and
  megohm (`2.5MHZ`, `1MOHM`). With the unit A, `MA` is milliampere
  (`6MA` = 0.006 A); mega-ampere would be `MAA`. A unit that is not the
  parameter's (`SOUR:FREQ 1V`) is `Illegal parameter value`.
- **ON / OFF** parameters also take `1` / `0`. Replies to ON / OFF queries
  are `1` or `0`.
- **Unanswerable queries.** A message with a query always gets a response.
  When a query cannot be answered (unknown header, wrong state), its reply is
  left out; if nothing is left, the response is an empty line. Ask
  `SYSTem:ERRor?` for the reason.

### Errors

Each link keeps an error queue of 8 entries. `SYSTem:ERRor?` returns the
oldest error and removes it: `<code>,"<text>"`, or `0,"No error"` when the
queue is empty. `SYSTem:ERRor:COUNt?` answers the number of entries. When
the queue is full, the newest entry becomes `-350,"Queue overflow"` and
later errors are lost until it is read. `*CLS` clears it; `*RST` does not.

A set command with a bad value changes nothing and does not answer: it
queues `Missing parameter` (no value), `Illegal parameter value` (not a
number or keyword of the command) or `Data out of range` (a number the
setting cannot take, e.g. `SOUR:FREQ 0`). The codes and texts:

| Code | Text | Meaning |
|---|---|---|
| -100 | `Command error` | `CONFigure` / `MEASure` with an unknown function. |
| -102 | `Syntax error` | A character not allowed in a header, or a command over 64 characters. |
| -108 | `Parameter not allowed` | A parameter after a command that takes none (`*IDN? 1`, `READ? 2`). |
| -109 | `Missing parameter` | A set command without its value (`SOUR:FREQ`). |
| -113 | `Undefined header` | Unknown command, or a query form that does not exist. |
| -200 | (its own text) | Other execution errors: `Upload not started`, `Not an update file`, `CRC mismatch`, `Verify failed`, `Rate too high for Bluetooth`, `Output buffer overflow` (Bluetooth: a reply did not fit the send buffer), ... |
| -203 | `Command protected` | Refused over Bluetooth (see below). |
| -221 | `Settings conflict` | Not possible in the present state (wrong function, a logger running, the scope running, ...). Some add the reason after a colon or semicolon. |
| -222 | `Data out of range` | A number outside what the setting takes; a range value above the largest range. |
| -223 | `Too much data` | A block reply larger than the link can send at once (Bluetooth). |
| -224 | `Illegal parameter value` | A parameter that is unknown or not a number. |
| -225 | `Out of memory` | The joined response would exceed 512 bytes. |
| -230 | `Data stale` | No fresh value: the reply is `+9.91E37` / `+9.9E37` or an old value. |
| -240 | `Flash error` | The loader marker could not be written (`DBG:UPDATE`). |
| -250 | `Mass storage busy`, `Mass storage error` | The disk is in use (a recording commits, a screenshot is saved, the PC has it mounted), or a disk failure. |
| -254 | `Mass storage full` | No space on the disk. |
| -256 | `File name not found` | `MMEMory` file name not on the disk. |
| -350 | `Queue overflow` | The error queue was full (see above). |
| -363 | `Input buffer overrun` | Bluetooth: a line longer than 256 bytes (dropped). |
| -400 | `Query error` | A block query together with other queries in one message. |
| -410 | `Query INTERRUPTED` | USB: a new message came while a `READ?` / `MEASure?` was still waiting; that query gets no reply. |

### Status reporting

There is no status or event reporting: `*STB?` and `*ESR?` always answer `0`,
there is no service request (SRQ), and `*OPC?` answers `1` at once. USB488
remote / local requests (REN, GO TO LOCAL, LOCAL LOCKOUT) are accepted and
change nothing; the touch screen and keys always stay active.
`STATus:QUEStionable` reports the wrong-connection alerts.

### Bluetooth differences

- Every `DBG:` command and `DIAG?` / `DIAG:PWM?` are refused. A message that
  contains one is refused as a whole (nothing of it runs, no reply) and
  queues `Command protected`.
- Also refused with `Command protected`: `MMEMory:DELete`, `LOG:CSV ON`,
  `LOG:STATe ON` with `LOG:DEST DISK`, and the firmware upload.
- `LOG:STATe ON` is refused (`Rate too high for Bluetooth`) for AD2
  sessions (AD1 sessions, at most 640 samples/s, work). `LOG:DATA?` over
  Bluetooth of such a session (started over USB) answers an empty block with
  the same error. These limits are the same in every `LOG:FORMat`.
- A block reply must fit 6144 bytes. A longer `WAVeform:DATA?` answers `#10`
  and queues `Too much data`: use `WAVeform:POINts 2000` or less with WORD.
- Lines are handled one at a time: the next line is read after the reply to
  the previous one has been sent. Send the next query after reading the
  reply.
- The link carries 11.5 kB/s (115200 baud). A `READ?` reply takes about 1 ms
  on the link; a 6 kB block about 0.5 s.

## Example session

```
> *IDN?
< Alientek,DM40C,0123456789ABCDEF01234567,opendm40 0.2.0
```

The four fields: maker, model (`DM40A`, `DM40B` or `DM40C`, from the meter's
calibration record), serial number (the chip's unique ID as 24 hex digits,
the same as the USB serial number), firmware name and version.

A DC voltage reading, then the function and range:

```
> CONF:VOLT:DC
> READ?
< +1.234567E+00
> CONF?
< "VOLT +6.000000E+00,+1.000000E-05"
> MEAS:RES?
< +4.701230E+03
```

`READ?` right after `CONF` waits for the first settled reading (up to about
3 s); give the host a timeout of 5 s or more.

A full scope record as 16-bit samples:

```
> STOP
> TRIG:STAT?            (repeat until STOP)
< STOP
> WAV:POIN MAX
> WAV:FORM WORD
> WAV:PRE?
< 1,2,<points>,1,<xinc>,<xorig>,0,<yinc>,<yorig>,0
> WAV:DATA?
< #<n><length><length bytes>
```

Sample i (a little-endian 16-bit word) is at time `i * xinc + xorig` seconds
from the trigger point and has the value `code * yinc + yorig` volts at the
probe tip. In Python with PyVISA:

```python
import pyvisa
rm = pyvisa.ResourceManager('@py')
names = [r for r in rm.list_resources() if '::4617::53312::' in r or '::0x1209::0xD040::' in r]
m = rm.open_resource(names[0], read_termination='\n', write_termination='\n',
                     timeout=5000)
print(m.query('*IDN?'))
m.write('CONF:VOLT:DC')
print(float(m.query('READ?')))
m.write('STOP;:WAV:POIN MAX;:WAV:FORM WORD')
while m.query('TRIG:STAT?') != 'STOP':
    pass
fmt, typ, n, _, xinc, xorig, xref, yinc, yorig, yref = map(float, m.query('WAV:PRE?').split(','))
codes = m.query_binary_values('WAV:DATA?', datatype='H', container=list)
volts = [(c - yref) * yinc + yorig for c in codes]
```

## IEEE 488.2 common commands

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `*IDN?` | | `Alientek,DM40C,<serial>,opendm40 0.2.0` (see the example above); with `SYSTem:IDN:EMULation 34401A`: `HEWLETT-PACKARD,34401A,0,opendm40 0.2.0` | both |
| `*RST` | | Accepted; changes nothing (settings, the error queue and the event bits stay). | both |
| `*CLS` | | Clears the error queue and the questionable event bits. | both |
| `*OPC?` | | `1` | both |
| `*ESR?` | | `0` | both |
| `*STB?` | | `0` | both |

## SYSTem and STATus

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `SYSTem:ERRor?` | | The oldest queued error, `<code>,"<text>"`, or `0,"No error"` (see [Errors](#errors)) | both |
| `SYSTem:ERRor:COUNt?` | | Errors in the queue, `0`..`8` | both |
| `SYSTem:MODel?` | | `<model>,<model byte>,<counts>`: the model (`DM40A`, `DM40B`, `DM40C`) and the raw model byte of the calibration record, then the count limit, always `60000` | both |
| `SYSTem:IDN:EMULation[?]` | `OFF` (default) or `34401A` | `OFF` or `34401A` | both |
| `SYSTem:LOCal` | | Accepted, does nothing. | both |
| `SYSTem:REMote` | | Accepted, does nothing. | both |
| `SYSTem:TEMPerature?` | | Temperature of the meter's processor, in the `UNIT:TEMPerature` unit: `+3.120000E+01` | both |
| `SYSTem:RAIL[?]` | `AUTO` (default) or `BOTH` | `AUTO` or `BOTH` | both |
| `SYSTem:RAIL:STATe?` | | `<dmm on 1/0>,<scope on 1/0>,<n>,<n>,<n>` | both |
| `SYSTem:ALERt?` | | `NONE`, `OVERLOAD`, `AJACK`, `APROT`, `VJACK`, `LIVE`, `DC`, `AC` or `FUSE` | both |
| `SYSTem:ALERt:ENABle[?]` | `0`..`15` (8 is ignored), `ON` (= 7, default) or `OFF` (= 0) | the mask, `7` | both |
| `SYSTem:ALERt:ALARm[?]` | `LOUD` (default), `VOLume` or `OFF` | `LOUD`, `VOLUME` or `OFF` | both |
| `SYSTem:BEEPer:VOLume[?]` | `0`..`100` (%) | the key/alert volume, `0`..`100`; `0` also silences the continuity / diode beeper | both |
| `SYSTem:FUSE:TEST` | | Starts the fuse check; returns at once. | both |
| `SYSTem:FUSE?` | | `IDLE`, `BUSY`, `OK,<ohm>`, `OPEN`, `FAIL,<ohm>` or `FAIL,NONE` | both |
| `STATus:QUEStionable:CONDition?` | | Bits now: bit 9 (512) a red alert (AJACK, APROT, OVERLOAD), bit 1 (2) OVERLOAD | both |
| `STATus:QUEStionable[:EVENt]?` | | The same bits collected since the last read or `*CLS`; reading clears them | both |
| `UNIT:TEMPerature[?]` | `C` or `F` | `C` or `F` | both |
| `MEMory:STATe:RECall:AUTO[?]` | `ON` or `OFF` (default) | `1` or `0` | both |

- `SYSTem:IDN:EMULation 34401A` makes `*IDN?` report an HP 34401A, for
  programs that know that meter but not the DM40 (sigrok and SmuView without
  the DM40 profile). It is stored. With it on, `READ?` and `MEASure?` never
  wait longer than 0.85 s and answer `+9.91E37` until the first reading after
  a function change.
- `SYSTem:RAIL BOTH` keeps the multimeter and the scope powered at the same
  time; `AUTO` powers only what the shown page needs. It is stored.
- `SYSTem:ALERt?` names the wrong-connection alert in force: `OVERLOAD` the
  uA / mA shunt was overloaded and the meter moved to the 10 A shunt;
  `AJACK` current in the A jack while not measuring current; `APROT` the A
  jack protection; `VJACK` a voltage on V/ohm while measuring current;
  `LIVE` a powered circuit in ohm / continuity / diode; `DC` / `AC` a hint
  that the input has DC on AC V / AC on DC V; `FUSE` a current function
  reading zero for 60 s.
- `SYSTem:ALERt:ENABle` bits: 1 AJACK, 2 VJACK and the DC / AC / FUSE hints,
  4 LIVE. 8 is accepted and ignored (it was the removed "Leads in A?" reminder).
  OVERLOAD cannot be turned off. Stored.
- `SYSTem:ALERt:ALARm`: the two-tone alarm for AJACK, APROT and a current
  above 10 A, at full volume (`LOUD`), at the key volume (`VOLume`) or off.
  Stored.
- `MEMory:STATe:RECall:AUTO` is Settings 3/3 **Mode Remember**: `ON`
  continues the multimeter after a power-off as it was. Stored.
- `SYSTem:FUSE:TEST`: plug one lead from V/ohm into A, nothing else. The
  meter measures 600 ohm for three readings, then returns to the function
  before. `OK` is about 48 ohm; `OPEN` is a blown fuse or lead.

## Measurement: CONFigure, MEASure, READ, FETCh

`CONFigure` selects a function and its range; `READ?` returns the next
reading. `MEASure?` selects the function and returns a reading in one step.
`FETCh?` returns the reading on the display without waiting for a new one.

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `CONFigure:VOLTage[:DC]` | `[<range>\|AUTO\|MIN\|MAX\|DEF[,<resolution>\|MIN\|MAX\|DEF]]` | | both |
| `CONFigure:VOLTage:AC` | as `CONFigure:VOLTage` | | both |
| `CONFigure:VOLTage:ACDC` | as `CONFigure:VOLTage` | | both |
| `CONFigure:VOLTage:AUTO` | as `CONFigure:VOLTage` | | both |
| `CONFigure:CURRent[:DC]` | as `CONFigure:VOLTage` | | both |
| `CONFigure:CURRent:AC` | as `CONFigure:VOLTage` | | both |
| `CONFigure:CURRent:ACDC` | as `CONFigure:VOLTage` | | both |
| `CONFigure:CURRent:AUTO` | as `CONFigure:VOLTage` | | both |
| `CONFigure:RESistance` | as `CONFigure:VOLTage` | | both |
| `CONFigure:FRESistance` | Same as `CONFigure:RESistance` (2-wire). | | both |
| `CONFigure:CONTinuity` | none | | both |
| `CONFigure:DIODe` | none | | both |
| `CONFigure:CAPacitance` | `[AUTO\|DEF]` | | both |
| `CONFigure:FREQuency` | `[AUTO\|DEF]` | | both |
| `CONFigure:TEMPerature` | `[<sensor>]` | | both |
| `CONFigure:POWer` | `[<V range>\|AUTO[,<A range>\|AUTO]]` (see [Power meter](#power-meter)) | | both |
| `CONFigure?` | | `"<function> <range>,<resolution>"`: `"VOLT +6.000000E+00,+1.000000E-04"`; `"CONT"`, `"DIOD"`; power meter `"POW AUTO,AUTO"` | both |
| `MEASure:VOLTage[:DC]?` | | a reading: `+1.234567E+00` | both |
| `MEASure:VOLTage:AC?` | | a reading | both |
| `MEASure:VOLTage:ACDC?` | | a reading | both |
| `MEASure:VOLTage:AUTO?` | | a reading | both |
| `MEASure:CURRent[:DC]?` | | a reading | both |
| `MEASure:CURRent:AC?` | | a reading | both |
| `MEASure:CURRent:ACDC?` | | a reading | both |
| `MEASure:CURRent:AUTO?` | | a reading | both |
| `MEASure:RESistance?` | | a reading in ohm | both |
| `MEASure:FRESistance?` | | as `MEASure:RESistance?` | both |
| `MEASure:CONTinuity?` | | a reading in ohm | both |
| `MEASure:DIODe?` | | a reading in V | both |
| `MEASure:CAPacitance?` | | a reading in F | both |
| `MEASure:FREQuency?` | | a reading in Hz | both |
| `MEASure:TEMPerature?` | `[<sensor>]` | a reading in the `UNIT:TEMPerature` unit | both |
| `MEASure:POWer?` | | real power P in W | both |
| `READ?` | | the next reading of the present function | both |
| `FETCh?` | | the reading on the display; if the function has no reading yet, the first one | both |
| `INITiate` | | Accepted, does nothing (the meter measures all the time). | both |
| `ABORt` | | Accepted, does nothing. | both |

Functions:

| Function | Header | Ranges (`<range>` values) |
|---|---|---|
| DC voltage | `VOLTage[:DC]` | 0.06, 0.6, 6, 60, 600, 1000 V |
| AC voltage (RMS) | `VOLTage:AC` | as DC voltage |
| AC+DC voltage (RMS of both) | `VOLTage:ACDC` | as DC voltage |
| Voltage, AC or DC chosen by the meter | `VOLTage:AUTO` | as DC voltage |
| DC current | `CURRent[:DC]` | 0.0006, 0.006, 0.06, 0.6, 6, 10 A |
| AC current, AC+DC current, AUTO | `CURRent:AC`, `CURRent:ACDC`, `CURRent:AUTO` | as DC current |
| Resistance | `RESistance` (`FRESistance` = the same) | 60, 600, 6k, 60k, 600k, 6M, 60M ohm |
| Continuity | `CONTinuity` | none |
| Diode | `DIODe` | none |
| Capacitance | `CAPacitance` | autorange only (6 nF .. 60 mF) |
| Frequency | `FREQuency` | autorange only (60 Hz .. 60 MHz) |
| Temperature | `TEMPerature` | none |
| Power meter | `POWer` | see [Power meter](#power-meter) |

- **Range.** A number selects the smallest range that holds it
  (`CONF:VOLT 10` = 60 V); `MIN` the smallest range, `MAX` the largest;
  `AUTO`, `DEF` or no range = autorange. A value above the largest range is
  `Data out of range`.
- **Resolution.** A value at least the range's 60000-count step (or `MAX`)
  sets the fast rate (`RATE FAST`, 60000 counts); a finer value, `MIN` or
  `DEF` sets the medium rate with one more digit where the range has it
  (`RATE MED`; `RATE SLOW` stays SLOW). The rate is one setting for all
  functions.
- **Temperature sensors** (`<sensor>`): `TCK` (K thermocouple, also `TC`
  or `K`), `PT100`, `PT1000`, `NTC10K3950`, `NTC10K3435`, `NTC100K3950`,
  `NTCUSER` (see `TEMPerature:NTC`). Without one, the present sensor stays.
- `MEASure` takes no range: a range given is ignored, and a newly selected
  function starts in autorange. `MEASure:TEMPerature?` takes a sensor.
- **AUTO coupling** (`VOLTage:AUTO`, `CURRent:AUTO`): the reading is the DC
  or the AC part, whichever the meter shows. `[SENSe:]VOLTage:COUPling?`
  says which.
- **Overload** reads `+9.9E37`.

**When the reply comes.** `READ?` and `MEASure?` return a fresh reading:

- Function already running: the next reading, or the latest one if none
  comes within 0.85 s.
- Just switched by this host (`MEASure?` with a new function, `CONFigure`
  then `READ?`): the first reading after the input has settled: a fraction
  of a second for DC V and DC A, about 2 s for resistance (settling and the
  range search; the high ranges longer), up to 3.9 s for the AUTO coupling. `+9.91E37` and `Data stale`
  if no reading comes within 1.9 s after settling.
- Changed on the meter (touch, keys, the phone app, the other link) while a
  `READ?` waits, or between this host's `FUNCtion?` / `CONFigure?` and its
  `READ?` / `FETCh?`: the reply is `+9.91E37` with `Data stale` at once, so
  a reading is never filed under the wrong function. Until the new function
  has a reading, `READ?` answers `+9.91E37` within 0.85 s.
- `FETCh?` answers `+9.91E37` while the range has just changed and no reading
  of the new range exists yet.
- While a data logger session runs (`LOG:STATe ON`), `READ?` and
  `MEASure?` answer `+9.9E37` at once; `FETCh?` returns the displayed
  reading if there is one, else `+9.9E37`.

## SENSe: function, coupling, range, rate

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `[SENSe:]FUNCtion[?]` | `"<function>"`: a header from the table above, quoted or not: `"VOLT:AC"` | `"VOLT"`, `"VOLT:AC"`, `"VOLT:ACDC"`, `"VOLT:AUTO"`, `"CURR"`, `"CURR:AC"`, `"CURR:ACDC"`, `"CURR:AUTO"`, `"RES"`, `"CONT"`, `"DIOD"`, `"CAP"`, `"FREQ"`, `"TEMP"`, `"POW"` | both |
| `[SENSe:]VOLTage:COUPling[?]` | `DC`, `AC`, `ACDC` or `AUTO`: selects voltage with that coupling | `DC`, `AC`, `ACDC`, `AUTO,DC` or `AUTO,AC` | both |
| `[SENSe:]CURRent:COUPling[?]` | as `VOLTage:COUPling`, for current | as `VOLTage:COUPling?` | both |
| `[SENSe:]VOLTage:RANGe[?]` | `<range>`, `AUTO`, `MIN`, `MAX` or `DEF` | the present range: `+6.000000E+00` | both |
| `[SENSe:]VOLTage:RANGe:AUTO[?]` | `ON`, `OFF` or `ONCE` | `1` autorange, `0` fixed | both |
| `[SENSe:]VOLTage:RESolution[?]` | `<resolution>`, `MIN`, `MAX` or `DEF` | the resolution: `+1.000000E-05` | both |
| `[SENSe:]CURRent:RANGe[?]` | as `VOLTage:RANGe` | as `VOLTage:RANGe?` | both |
| `[SENSe:]CURRent:RANGe:AUTO[?]` | as `VOLTage:RANGe:AUTO` | as `VOLTage:RANGe:AUTO?` | both |
| `[SENSe:]CURRent:RESolution[?]` | as `VOLTage:RESolution` | as `VOLTage:RESolution?` | both |
| `[SENSe:]RESistance:RANGe[?]` | as `VOLTage:RANGe` | as `VOLTage:RANGe?` | both |
| `[SENSe:]RESistance:RANGe:AUTO[?]` | as `VOLTage:RANGe:AUTO` | as `VOLTage:RANGe:AUTO?` | both |
| `[SENSe:]RESistance:RESolution[?]` | as `VOLTage:RESolution` | as `VOLTage:RESolution?` | both |
| `[SENSe:]RESistance:NEGative[?]` | `ON` or `OFF` (default): resistance readings below 0 shown, not 0 | `1` or `0` | both |
| `[SENSe:]CAPacitance:RANGe[?]` | `AUTO` or `DEF` | the present range: `+6.000000E-06` | both |
| `[SENSe:]CAPacitance:RANGe:AUTO[?]` | `ON`, `OFF` or `ONCE` | `1` or `0` | both |
| `[SENSe:]FREQuency:RANGe[?]` | `AUTO` or `DEF` | the present range: `+6.000000E+03` | both |
| `[SENSe:]FREQuency:RANGe:AUTO[?]` | `ON`, `OFF` or `ONCE` | `1` or `0` | both |
| `[SENSe:]RATE[?]` | `SLOW`, `MEDium` or `FAST` | `SLOW`, `MED` or `FAST` | both |
| `[SENSe:]RATE:PERiod?` | | Time between readings in s, measured over the last few (not across a change or a dropped conversion): `+1.000000E-01` (0 before two readings) | both |

- **Range commands** act on the present function. `VOLTage` and `CURRent`
  without a coupling mean that quantity in whatever coupling is selected;
  add `:DC`, `:AC`, `:ACDC` or `:AUTO` to name one (`VOLT:AC:RANG?`). Every
  form of `CONFigure`'s function headers is accepted (`FRESistance:RANGe`,
  `CAPacitance:RESolution?`, ...).
- Setting the range of a function that is not selected is a
  `Settings conflict`. Querying it is no error: `:RANGe:AUTO?` answers `1`,
  `:RANGe?` and `:RESolution?` answer `+9.91E37`.
- `:RANGe:AUTO ON` returns to the autorange chosen on the meter (AUTO+ or
  AUTO); `OFF` and `ONCE` keep the present range. Setting a range turns REL
  off.
- `:RANGe?` also reports the range while autoranging.
- `RESistance:RANGe` above 60 MΩ selects the experimental 600 MΩ
  (`+6.000000E+08`); `MAX` is 60 MΩ. `:RANGe:AUTO ON` leaves it. It reads
  once per ~2 s, so `READ?` may answer with a reading up to 2 s old;
  `:RESolution` sets nothing there (its digits follow the value).
- In the power meter, `VOLTage:RANGe?`, `CURRent:RANGe?` and their `:AUTO?`
  report the power meter's V and I channels; set them with
  `POWer:VOLTage:RANGe` / `POWer:CURRent:RANGe`.
- Continuity, diode and temperature have no range (`Undefined header`).
- Range and resolution changes are refused (`Settings conflict`) while a data
  logger session runs or the Wave view is open; `CONFigure` then only
  selects the function.
- **Rate:** `FAST` 10 readings/s with 60000 counts (resistance: 5/s);
  `MEDium` 5 readings/s (resistance: 2.5/s) and `SLOW` 2.5 readings/s, with one more digit
  (in DC V 600 mV .. 600 V and resistance 600 ohm .. 60 kohm); `SLOW` also
  averages 16 conversions. Every rate rejects 50 and 60 Hz. In DC V, mV
  and A, `FAST` runs the converter unchopped and measures its offset
  itself 2 s after a function or range change and every 30 s: two
  conversions (0.2 s) give no new reading then (`DBG:FAST?`). It applies to
  DC voltage, DC current and resistance; other functions keep their own
  timing. Stored.
- `RESistance:NEGative` is Settings 3/3 **Negative ohm**. Stored.

## Temperature

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `[SENSe:]TEMPerature:TRANsducer[?]` | `<sensor>` (see [sensors](#measurement-configure-measure-read-fetch)) | `TCK`, `PT100`, `PT1000`, `NTC10K3950`, `NTC10K3435`, `NTC100K3950` or `NTCUSER` | both |
| `[SENSe:]TEMPerature:OFFSet[?]` | offset in C, -20 .. 20 in 0.1 C steps | `+0.000000E+00` | both |
| `[SENSe:]TEMPerature:NTC[?]` | `<R25 in ohm>,<Beta>` | `+1.000000E+04,+3.950000E+03` | both |
| `[SENSe:]TEMPerature:EXCitation?` | | `<range>,<R ohm>,<I A>,<P W>` of a resistive sensor | both |
| `MEASure:TEMPerature:CJ?` | | `<cold junction temperature>,<age s>` | both |

- `TEMPerature:OFFSet` belongs to the present sensor; for the thermocouple it
  is added to the cold junction. Offsets are always in C.
- `TEMPerature:NTC` sets the parameters of the `NTCUSER` sensor.
- `TEMPerature:EXCitation?` gives the present range and the sensor's
  resistance, current and dissipated power; with the thermocouple, and on
  the 60 MΩ range (no measured test current), it is a `Settings conflict`.
- `MEASure:TEMPerature:CJ?` is fresh only while the thermocouple is
  measured; otherwise it gives the last value with its age and queues
  `Data stale` (`+9.9E37` if it was never measured).

## Burden voltage

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `FETCh:BURDen?` | | Voltage across the shunt for the latest current reading, V: `+1.234000E-02` | both |
| `DISPlay:BURDen[?]` | `ON` or `OFF` (default) | `1` or `0` | both |

- `FETCh:BURDen?` uses the shunt in use (or the resistance set with
  `POWer:BURDen`); DC readings are signed, AC and AC+DC are RMS. Outside the
  current functions it answers `+9.9E37` with `Data stale`.
- `DISPlay:BURDen` shows the burden voltage on the DMM page (DC A and AC A).
  Stored.

## CALCulate: hold, statistics, limits, smoothing

These act on the present DMM function; values are in its SI unit (W in the
power meter).

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `CALCulate:AHOLd[?]` | `ON` or `OFF` | `1` or `0` | both |
| `CALCulate:AVERage[:AVERage]?` | | Mean: `+1.234567E+00` | both |
| `CALCulate:AVERage:MINimum?` | | Minimum | both |
| `CALCulate:AVERage:MAXimum?` | | Maximum | both |
| `CALCulate:AVERage:COUNt?` | | Number of samples: `120` | both |
| `CALCulate:AVERage:SDEViation?` | | Sample standard deviation (N − 1); `0` after one sample | both |
| `CALCulate:AVERage:CLEar` | | Restarts the statistics. | both |
| `CALCulate:AVERage:STATe[?]` | `ON` or `OFF` | `1` or `0` | both |
| `CALCulate:LIMit:LOWer[?]` | `<value>` | `+1.000000E+00`, or `+9.9E37` when not set | both |
| `CALCulate:LIMit:UPPer[?]` | `<value>` | as `LIMit:LOWer?` | both |
| `CALCulate:LIMit:STATe[?]` | `ON` or `OFF` | `1` or `0` | both |
| `CALCulate:LIMit:CLEar` | | Removes both limits. | both |
| `CALCulate:LIMit:FAIL?` | | `1` the value is outside the limits, else `0` | both |
| `CALCulate:SMOothing[:STATe][?]` | `ON` or `OFF` | `1` or `0` | both |
| `CALCulate:SMOothing:RESPonse[?]` | `SLOW`, `MEDium` or `FAST` | `SLOW` or `FAST` | both |

- `AHOLd`: AutoHold, as a long press on RUN.
- `AVERage...?` before the first sample answer `+9.9E37` with `Data stale`.
- `AVERage:STATe ON` starts a recording session: the statistics restart and
  the meter does not power off when idle.
- Setting a limit turns the limit check on.
- Smoothing changes the displayed value and `READ?` / `MEASure?` /
  `FETCh?`; the statistics and the CSV log use the unsmoothed readings.
  `RESPonse SLOW` is the strong smoothing, `MEDium` and `FAST` the light one;
  setting it turns smoothing on.
  Stored.

## Power meter

The power meter measures voltage (V/COM) and current (A jacks) at the same
time. Select it with `CONFigure:POWer` (or on the meter); `READ?`, `FETCh?`
and `MEASure:POWer?` then give the real power P in W.

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `FETCh:POWer:ALL?` | | `P,S,Q,PF,Vrms,Irms,F,E_Wh,seconds,Vdc,Idc,VAh,burden_V` | both |
| `MEASure:POWer:APParent?` | | S, VA | both |
| `MEASure:POWer:REACtive?` | | Q, var | both |
| `MEASure:POWer:PFACtor?` | | Power factor | both |
| `MEASure:POWer:ENERgy?` | | Energy, Wh | both |
| `MEASure:POWer:VOLTage?` | | Vrms | both |
| `MEASure:POWer:CURRent?` | | Irms | both |
| `[SENSe:]POWer:VOLTage:RANGe[?]` | `<V>` (6, 60, 600, 1000) or `AUTO` | `AUTO,+6.000000E+02` or `MAN,+6.000000E+01` | both |
| `[SENSe:]POWer:CURRent:RANGe[?]` | `<A>` (0.0006 .. 10) or `AUTO` | as `VOLTage:RANGe?` | both |
| `[SENSe:]POWer:COUPling[?]` | `DC`, `AC` or `ACDC` | `DC`, `AC` or `ACDC` | both |
| `[SENSe:]POWer:ENERgy:RESet` | | Restarts energy and time. | both |
| `[SENSe:]POWer:ENERgy:STATe[?]` | `ON` or `OFF` | `1` or `0` | both |
| `[SENSe:]POWer:BURDen[?]` | `<ohm>` (0 .. 1000) or `DEFault` | `+1.000000E-02` | both |
| `[SENSe:]POWer:BURDen:STATe[?]` | `ON` or `OFF` (default) | `1` or `0` | both |
| `[SENSe:]POWer:METHod[?]` | `SIMultaneous` or `ALTernate` | `SIM` or `ALT` | both |
| `[SENSe:]POWer:LIMit[?]` | `<A>[,<s>]`: 0.1 .. 10 A (default 6; smaller values are set to 0.1), 0 .. 3600 s (default 30) | `+6.000000E+00,30` | both |
| `[SENSe:]POWer:PHASe[?]` | `-127` .. `127` | `0` | both |
| `[SENSe:]POWer:PHASe:CALibrate[?]` | `[REACtive]` | `NONE`, `RUN`, `OK`, `FLAT`, `NOTREACTIVE` or `RANGE`, then the phase setting: `OK,-3` | both |
| `[SENSe:]POWer:CYCle?` | | `<discarded>,<P period ms>,<full cycle ms>`: `2,642,1284` | both |
| `[SENSe:]POWer:ZERO[?]` | none (start) or `CLEar` | `<result>,SET\|NONE,<V zero>,<6 current zeros>,<6 DC zeros>` | both |
| `[SENSe:]POWer:VOLTage:CALibrate[?]` | none (start) or `DEFault` | `<result>,<V range>,<gain>,SELF\|FACT` | both |
| `[SENSe:]POWer:VOLTage:GAIN?` | | the four V range gains | both |

- `CONFigure:POWer [<V>|AUTO[,<A>|AUTO]]` selects the power meter and its
  ranges: a number picks the smallest range that holds it. From another
  function the coupling starts as DC: send `POWer:COUPling` after
  `CONFigure:POWer`.
- `MEASure:POWer?` from another function waits for the range search, up to
  15 s.
- `FETCh:POWer:ALL?` and `MEASure:POWer:<x>?` answer only in the power
  meter (else no reply, `Settings conflict`). A part with no value is
  `+9.91E37` (PF, S, Q and F below their AC thresholds; PF also with no
  current or voltage, S at the noise level; F when no frequency is
  measured).
- `BURDen`: the resistance of the current path, used for the burden voltage
  and for "At load" (`BURDen:STATe ON`: V and P corrected for the drop in
  the meter). `DEFault` restores the shunt's value.
- `METHod ALTernate` measures V and I in turn (a fallback without P); the
  default `SIMultaneous` measures P directly.
- `LIMit`: the continuous current and the time allowed above it before the
  meter warns.
- `PHASe` and `PHASe:CALibrate`: a phase correction between the V and I
  channels; calibrate with a reactive load (a capacitor or inductor, PF near
  0) in AC or AC+DC coupling. `RESistive` is refused.
- `ZERO` (leads open, nothing in A, about 12 s) measures the zero offsets;
  `ZERO CLEar` removes them. Results: `RUN`, `OK`, `SIGNAL`, `SMALL`, `BIG`,
  `UNSTABLE`, `RANGE`, `ABORT`, `METHOD`, `NONE`.
- `VOLTage:CALibrate` (a steady AC or DC voltage of 10 .. 70 % of the range
  on V/COM) matches the V channel's gain on the present range to the meter's
  voltmeter; `DEFault` returns to the factory-derived gain.
- Ranges, coupling, burden, zeros and gains are stored.

## Scope

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `RUN` | | Starts acquiring. | both |
| `STOP` | | Stops after one more capture (as `DIGitize`, at most the screen's time + 1 s; `TRIGger:STATus?` is `STOP` once it is in); a second `STOP` meanwhile stops at once. | both |
| `SINGle` | | Sets the sweep to Single and starts. | both |
| `DIGitize` | | One acquisition with the present sweep, then STOP. | both |
| `TRIGger:STATus?` | | `STOP`, `WAIT`, `AUTO` (untriggered), `TD` (triggered) or `RUN` (roll) | both |
| `TIMebase[:MAIN]:SCALe[?]` | s/div, 100E-9 .. 50 | `+1.000000E-03` | both |
| `CHANnel1:SCALe[?]` | V/div at the probe tip, 0.01 .. 10 (times the probe factor) | `+1.000000E+00` | both |
| `CHANnel1:COUPling[?]` | `AC` or `DC` | `AC` or `DC` | both |
| `CHANnel1:OFFSet[?]` | V added to the signal (positive moves the trace up) | `+0.000000E+00` | both |
| `TRIGger[:EDGE]:LEVel[?]` | V at the probe tip | `+5.000000E-01` | both |
| `TRIGger[:EDGE]:SLOPe[?]` | `POSitive` or `NEGative` | `POS` or `NEG` | both |
| `TRIGger:SWEep[?]` | `AUTO`, `NORMal` or `SINGle` | `AUTO`, `NORM` or `SING` | both |
| `TRIGger[:EDGE]:SOURce[?]` | `CHANnel1` or `GENerator` (the generator's output, computed from its timers: edge only, no wire needed) | `CHAN1` or `GEN` | both |
| `ACQuire:SRATe?` | | Sample rate, S/s: `+5.000000E+07`; stopped, the stored record's, also after a zoom (1 / the record preamble's xinc; the screen preamble's xinc follows the shown timebase) | both |
| `SCOPe:MEASure:VPP?` | | Peak-to-peak, V | both |
| `SCOPe:MEASure:VAVG?` | | Mean, V | both |
| `SCOPe:MEASure:VRMS?` | | RMS, V | both |
| `SCOPe:MEASure:FREQuency?` | | Frequency, Hz | both |
| `WAVeform:SOURce[?]` | `CHANnel1` | `CHAN1` | both |
| `WAVeform:FORMat[?]` | `BYTE` (default) or `WORD` | `BYTE` or `WORD` | both |
| `WAVeform:POINts[?]` | `SCReen` (default), `MAXimum` or `<n>` | points of the next `WAVeform:DATA?` | both |
| `WAVeform:PREamble?` | | `format,type,points,count,xinc,xorig,xref,yinc,yorig,yref` | both |
| `WAVeform:DATA?` | | block: `#3400` and 400 bytes for SCReen + BYTE | both (see notes) |

- Scope commands turn the scope on if it is off; the meter's screen shows
  the same settings.
- Time and voltage settings snap to the nearest step at or above the value
  (1-2-5 steps). A scale of 0 or below is `Data out of range`; a value that
  is no number or keyword of the command is `Illegal parameter value`
  (nothing changes).
- Volts are at the probe tip: the probe factor set in the scope settings
  window applies. There is no probe command.
- Setting `CHANnel1:SCALe` or `CHANnel1:COUPling` closes the Wave view (the
  multimeter-input scope); timebase, trigger, the queries and the waveform
  commands work on it.
- `WAVeform:POINts`: `SCReen` the 400 screen columns (mean of each; in
  roll mode the middle of each column's min and max, 0 before data),
  `MAXimum` the whole acquisition record, `<n>` the record thinned to at
  most n points.
- `WAVeform:FORMat`: `BYTE` one byte per point (the 10-bit code divided by
  4), `WORD` the 10-bit code as a little-endian 16-bit word.
- `WAVeform:PREamble?`: format 0 BYTE / 1 WORD; type 0 screen columns, 2
  record; count 1. Point i is at time `(i - xref) * xinc + xorig` seconds
  from the trigger point (screen centre when untriggered) and has
  `(data - yref) * yinc + yorig` volts (BYTE: the middle of the four codes
  a byte stands for).
- `WAVeform:DATA?` for the record (any POINts other than SCReen) needs the
  scope stopped (`STOP`, `SINGle` or `DIGitize`, then `TRIGger:STATus?` =
  `STOP`); while it runs the reply is `#10` with `Settings conflict`. Over
  USB a reply of any length is sent (a full WORD record is up to 512 KiB);
  over Bluetooth at most 6 kB.
- Only the SCReen + BYTE reply may be combined with other queries in one
  message.

## Protocol decoder

UART and Modbus RTU on the scope page ([the decoder](README.md#protocol-decoder)).

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `DECode:PROTocol[?]` | `OFF` (default), `UART` or `MODBus` | `OFF`, `UART` or `MODB` | both |
| `DECode:BAUD[?]` | bit rate, 300 .. 115200, or `AUTO` (default) | `AUTO` or `9600` | both |
| `DECode:RATE?` | | the bit rate of the decoded record: `9600` (`0`: none) | both |
| `DECode:FORMat[?]` | `8N1` (default), `8E1`, `8O1` or `7E1` | `8N1` | both |
| `DECode:POLarity[?]` | `NORMal` (default) or `INVerted` (RS-232 levels) | `NORM` or `INV` | both |
| `DECode:THReshold[?]` | level, V at the probe tip, or `AUTO` (default) | `AUTO` or `+1.500000E+00` | both |
| `DECode:LEVel?` | | the level of the decoded record, V (`+9.91E37`: none) | both |
| `DECode:MODE[?]` | `BAND` (default: under the trace) or `LOG` (the frame log; leaving it: `BAND`) | `BAND` or `LOG` | both |
| `DECode:COUNt?` | | `frames,bytes,framing,parity,crc,lost` (Band: of what `DECode:FRAMes?` reports, `lost` 0; Log: since the start or `DECode:CLEar`) | both |
| `DECode:FRAMes?` | Band: `[<first>]`, from frame number `first` (default 0); Log: `[<n>]`, at most n frames (default all) | block of frame lines | both |
| `DECode:CLEar` | | Log: the list and the counters start again | both |

- `DECode:FRAMes?` answers a `#4nnnn` block of text lines, one per frame:
  `seq,time_s,proto,hex,status,info`, of the decoded record: running, the
  last record that triggered; stopped, the stored record (decoded on the
  query if the scope page has not shown it). It holds up to 80 bytes and
  16 Modbus frames; a record with more: those around the screen. `seq`
  counts from 0; a reply holds as many whole lines as fit (about 6 kB), so
  read on from the last `seq` + 1 until the block is empty (`#40000`).

  `time_s` is the frame start in seconds from the trigger point (as Save
  CSV), `proto` `UART` or `MB`, `hex` the bytes, `status` `OK`, `FE`
  (framing error), `PE` (parity), `CRC` or `PART` (the frame runs past the
  record or the screen). Modbus `info`: the fields, for example `REQ 01 03
  @0000 n=10`, `RSP 01 03 20B req=12` (the request's `seq`), `EXC 01 83
  E02`; `?` without fields. UART frames: bytes with less than 2 character
  times between them, at most 16.
- Log (`DECode:MODE LOG`, the scope page's input): `DECode:FRAMes?` gives
  the frames not read yet, oldest first, and moves past them (a FIFO; poll
  it to follow the bus). `time_s` counts from the log's start; `seq`
  numbers every frame; a gap in `seq` means frames the list dropped
  (oldest first when full) before they were read. `lost`: times the
  decoder fell behind the input (samples skipped). `LOG` in the Wave view
  or while a DMM logger session streams (`LOG:STATe ON`): `Settings
  conflict`; so is `LOG:STATe ON` while the frame log is open.
- With too few samples per bit (fewer than 4: a slow timebase, the Wave

  view above 4800 baud) nothing is decoded; `DECode:RATE?` gives `0` with
  `AUTO`.

## Generator (SOURce)

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `SOURce:FUNCtion[?]` | `SINusoid`, `SQUare`, `TRIangle` or `RAMP` | `SIN`, `SQU`, `TRI` or `RAMP` | both |
| `SOURce:FREQuency[?]` | Hz, 1 .. 10E6 (whole Hz) | the frequency produced: `+1.000000E+03` | both |
| `SOURce:VOLTage[?]` | amplitude Vpp, 0 .. 3 | `+1.000000E+00` | both |
| `SOURce:VOLTage:OFFSet[?]` | centre level, V, or `AUTO` | `+1.500000E+00` | both |
| `SOURce:FUNCtion:SQUare:DCYCle[?]` | duty cycle, % (0 .. 100) | the duty produced: `+5.000000E+01` | both |
| `OUTPut[?]` | `ON` or `OFF` | `1` or `0` | both |

- A frequency of 0 or below, a negative amplitude or a duty outside 0 .. 100
  is `Data out of range`; the setting stays as it was.
- Sine, triangle and ramp go up to 50 kHz, square up to 10 MHz.
  `SOURce:FREQuency?` and `DCYCle?` report what the output really produces
  (limited and rounded to the generator's steps).
- `SOURce:VOLTage:OFFSet` is the centre of the waveform. It is kept within
  Vpp/2 .. 3 - Vpp/2 (with an offset set, a Vpp too large for it is
  reduced to fit); `AUTO` is the default 0 .. Vpp output (the query then
  answers Vpp/2). With an offset and 0 Vpp the output is a DC level.
- The generator keeps running on every page.

## Inrush current

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `INRush[:STATe][?]` | `ON` or `OFF` | `1` or `0` | both |
| `INRush:ARM` | | Arms a new capture (opens the page if needed). | both |
| `INRush:THReshold[?]` | trigger step, A, 0.1 .. 50 (default 1) | `+1.000000E+00` | both |
| `INRush:CONDition?` | | `OFF`, `IDLE`, `ARMING`, `ARMED`, `TRIGGERED`, `SAVING` or `DONE` | both |
| `INRush:FETCh?` | | `<Irms first 100 ms>,<Ipeak>,<Iidle>,<lost samples>`: `+1.234000E+01,+2.500000E+01,+1.000000E-01,0` | both |
| `INRush:FILE?` | | `"LOGS/FASTnnn.DLG"` of the capture, or `""` | both |
| `INRush:PREamble?` | | `<rate Hz>,<samples>,<trigger index>,<a>,<b>`: current = a + b * code | both |
| `INRush:DATA?` | `[<start>[,<count>]]`, count at most 3000 (default) | block of little-endian 16-bit codes | both |

- `INRush ON` opens the Inrush Current page (current function, 10 A range)
  and arms it; `OFF` closes it and restores the previous settings.
- `INRush:FETCh?` without a capture answers `+9.9E37` in the first three
  fields with `Data stale`; a peak beyond the range is `+9.9E37`.
- The trace has 19200 samples at 9600 Hz, the trigger at sample 1920. Read
  it in pieces: `INR:DATA? 0,3000`, `INR:DATA? 3000,3000`, ...

## Data logging (LOG)

Two loggers: the **CSV recorder** writes the multimeter's readings to the
meter's disk (as the Record page), and the **fast stream** delivers every
conversion of the meter's converter, either to the host (`LOG:DATA?`) or to
the disk.

### CSV recorder

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `LOG:CSV[?]` | `ON` or `OFF` | `<state>,<file>,<rows>,<bytes>,<committed>,<dropped>` | both; `ON` USB only |
| `LOG:CSV:INTerval[?]` | `0` (every reading, default), `1`, `10` or `60` s, or `CHANge` (on change) | `0`, or `CHAN` | both |
| `LOG:CSV:CHANge[?]` | `0.1`, `1` (default) or `5` % of the range's full scale | `1` | both |
| `LOG:CSV:DURation[?]` | `0` (until stopped, default), `600`, `3600`, `28800` or `86400` s | `0` | both |
| `LOG:CSV:FREE?` | | Free bytes on the disk | both |

- `LOG:CSV?` state: 0 idle, 1 recording, 2 stopping, 3 done, 4 disk full, 5
  no disk, 6 error; file is -1 until the first save.
- `INTerval` and `DURation` apply to the next recording (`Settings
  conflict` while one runs). With an interval, each row is the mean of the
  readings in it. With `CHANge`, a row at its reading's time when the
  value moves by more than `LOG:CSV:CHANge` % of the range's full scale
  from the last row, when OL / HI / LO changes, or 60 s after the last row.
- `LOG:CSV ON` fails with `Settings conflict: Wave view`, `Settings
  conflict: inrush page`, or `Settings conflict` (no reading yet, a
  recording already on, the disk not available).
- The recordings are `LOGS/LOGnnn` files; fetch them with `MMEMory:DATA?`.

### Fast stream

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `LOG:STATe[?]` | `ON` or `OFF` | `1` or `0` | both (see notes) |
| `LOG:SOURce[?]` | `AD1` (default) or `AD2` | `AD1` or `AD2` | both |
| `LOG:OSR[?]` | `0` .. `7` (default 3) | `3` | both |
| `LOG:CHOP[?]` | `ON` (default) or `OFF` | `1` or `0` | both |
| `LOG:SRATe[?]` | AD2 rate, S/s (default 4800) | the requested rate: `4800` | both |
| `LOG:DEST[?]` | `HOST` (default) or `DISK` | `HOST` or `DISK` | both |
| `LOG:DURation[?]` | DISK session length, s (0 = until stopped or full; default 60) | `60` | both |
| `LOG:FORMat[?]` | `ROWS` (default), `PACKed` or `DLG` | `ROWS`, `PACK` or `DLG` | both |
| `LOG:DATA?` | | block of records, oldest first, in the `LOG:FORMat` layout | both |
| `LOG:PREamble?` | | `mode,range,osr,nominal_us,measured_hz,chop,source,req_hz,nominal_mhz,r26,coupling` | both |
| `LOG:SCALe?` | | `a,b`: value = a + b * raw, in V or A (AD2); `0,0` for AD1 | both |
| `LOG:RATE?` | | Measured mean sample rate, Hz | both |
| `LOG:COUNt?` | | `buffered,captured,overwritten,missed,lost` | both |
| `LOG:GAPs?` | | `bad,max_gap_us,g2,g3_4,g5_16,g17_128,g129` | both |
| `LOG:DISK?` | | `state,file,alloc,bytes,rows,prep_pct,fifo_hw,programs,tpp_us,tpp_max_us,gate,gate_isr_permille,gate_fill,writer_permille` | both |

- **Sources.** `AD1`: DC voltage, DC millivolt and DC current; the rate is
  set by `LOG:OSR` (0 = 320/s .. 3 = 40/s .. 7 = 2.5/s; `LOG:CHOP OFF`
  doubles it). `AD2`: the AC voltage, AC millivolt and AC current rows and
  DC voltage, at `LOG:SRATe`: the nearest of 1200, 2400, 4800, 9600, 19200,
  38400 and 76800 S/s. Set the function on the meter (or with `CONFigure`)
  first; AC+DC, AUTO coupling, the power meter, resistance, capacitance,
  frequency and temperature cannot be streamed (`Settings conflict`).
  Source, OSR, chop and rate take effect at the next `LOG:STATe ON`.
- While a session runs the multimeter does not update its readings.
- **To the host** (`LOG:DEST HOST`): read `LOG:DATA?` repeatedly. The
  buffer holds 2048 records; when it is full the newest are dropped
  (`overwritten`). An empty buffer answers `#40000` in every format.
- **`LOG:FORMat ROWS`** (default): each record is `u32 sequence, u32 time in
  us, i32 raw code`, little-endian, 12 bytes; at most 511 records per
  reply.
- **`LOG:FORMat PACKed`**: 3 bytes per sample. A 20-byte reply header
  (`LP`, version, source, rate in mHz, `a`, `b` of `LOG:SCALe?`, records
  left), then blocks: a 16-byte header (first sequence number, its time in
  us, sample count, sample width, flags, conversions lost before the block)
  and the samples. Sample `i` of a block was taken at `t0 + i / rate`. Every
  gap starts a new block, so the lost samples show in the block header,
  never inside a block. Up to ~2030 samples per reply.
- **`LOG:FORMat DLG`**: the same reply header (`LD`), one block header,
  then the samples coded as in the meter's fast log files (`FASTnnn.DLG`):
  exact times and values, about 1 to 3 bytes per sample (it depends on the
  signal's noise), up to 4 KiB of data per reply.
- The format can be changed at any time; each reply describes itself. The
  byte layout is in
  [`firmware/src/logger.rs`](../../firmware/src/logger.rs) ("Compact
  LOG:DATA?"), and `dm40scpi.py log --format` decodes both formats
  ([Connecting from a computer](README.md#fast-stream-to-a-csv-file)).
- **Raw codes.** AD1: counts; 1.389 V / 8388607 counts at the converter,
  times the range's decade; the calibration is not applied. AD2: use
  `LOG:SCALe?`.
- **To the disk** (`LOG:DEST DISK`, USB only): `LOG:STATe ON` reserves and
  erases space for `LOG:DURation` (progress in `LOG:DISK?`), then records
  to `LOGS/FASTnnn.DLG` until `LOG:STATe OFF`, the duration or a full disk.
  `LOG:DATA?` answers an empty block meanwhile. `LOG:DISK?` state: 0 idle,
  1 preparing, 2 ready, 3 running, 4 stopping, 5 done, 6 full, 7 no disk,
  8 error; file is -1 until the file exists.
- Over Bluetooth, AD1 sessions to the host work; AD2 and disk sessions are
  refused.

## Files on the disk (MMEMory)

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `MMEMory:CATalog?` | | `<used bytes>,<free bytes>{,"<name>,BIN,<size>"}` | both |
| `MMEMory:DATA?` | `"<name>"[,<offset>[,<length>]]`, length at most 6000 (default) | block of the file's bytes from offset | both |
| `MMEMory:DELete` | `"<name>"` | | USB only |

- `MMEMory:CATalog?` lists the screenshots, CSV recordings and scope records,
  each as `"<folder>/<name>,BIN,<size>"`: `"SHOTS/S000.BMP,BIN,<size>"`,
  `"LOGS/LOG000.DLG,BIN,<size>"`, `"SCOPE/SCOP000.DLG,BIN,<size>"`. The
  sizes are in bytes. Entries that do not fit the 512-byte reply are left
  out.
- `MMEMory:DATA?` reads up to 6000 bytes; read a large file in pieces with
  rising offsets until the block is shorter than asked. The name may have
  its folder (`LOGS/`, `SHOTS/`, `SCOPE/`) or not; a folder must be the
  file's own (`SHOTS/LOG005.DLG` answers `File name not found`, as does
  `MMEMory:DELete` with it). Asking for
  `LOGnnn.CSV`, `SCOPnnn.CSV` or `FASTnnn.CSV` returns the CSV file the
  meter makes from the `.DLG` recording (the same CSV the USB disk shows).
- `MMEMory:DELete` deletes one screenshot or recording; not the one being
  recorded.

## Display and screenshots

There is no regular SCPI command that takes a screenshot. A three-finger
swipe down on the screen saves one as `SHOTS/Snnn.BMP` (over USB the debug
command `DBG:SWIPE D,3` does the same); fetch it with `MMEMory:DATA?` or copy
it from the USB disk. `DISPlay:BURDen` is listed under
[Burden voltage](#burden-voltage).

## Debug commands

For diagnostics and development; they may change between versions. All
`DBG:` and `DIAG` commands work over USB only. Replies are comma-separated numbers
unless shown otherwise.

| Command | Parameters | Reply | Link |
|---|---|---|---|
| `DIAG?` | | `<min V>,<max V>,<mid crossings>,<window s>` of the generator's DAC output | USB |
| `DIAG:PWM?` | | `<PWM edges per s>,PB12=0\|1` | USB |
| `DBG:PING?` | `[<ms>]` | `<uptime ms>,<flags>,<keepalive left ms>`; `4294967295` = no limit (the default after boot) | USB |
| `DBG:BOOT?` | | `NONE`, or `B2,<57 words>,<renamed>`: boot and reset information ([below](#dbgboot)) | USB |
| `DBG:IDLE` | `<ms>` (`0` = the **Shutdown** setting) | Sets the automatic power-off time until the next restart, for tests. | USB |
| `DBG:IDLE?` | | `<ms since the last touch or key>,<ms since the last touch, key or host activity>,<power-off ms, 0 = never>` | USB |
| `DBG:VIEW?` | | `<page>,<settings page>,<wave view 1/0>,<trend chart ms/div>` | USB |
| `DBG:READout?` | | `<kind F\|G\|P\|N>,<unit>,<value>,<hold>`: the DMM value as drawn | USB |
| `DBG:TAP` | `<x>,<y>` | Taps the screen. | USB |
| `DBG:SWIPE` | `L`, `R`, or `D,<fingers>` | Swipes (D,3 saves a screenshot, D,2 powers off). | USB |
| `DBG:DRAG` | `<x0>,<y0>,<x1>,<y1>,<ms>[,<hold ms>[,<jitter px>]]` | One-finger drag; the jitter wobbles the path sideways. | USB |
| `DBG:KEY` | `<key 0..3>[,<hold ms>]` | Presses a side key: 0 MENU, 1 RUN, 2 AUTO, 3 MF. | USB |
| `DBG:LCD?` | `<x>,<y>,<w>,<h>,<n>` | block: n raw display memory words | USB |
| `DBG:TE?` | | Per phase (TE off; TE on, mode 0; TE on, mode 1; `;` between): `<samples>`, then `<level changes>,<samples high>` for PA3, PB13, PC1, PC15. Blocks ~0.3 s. ([LCD, TE](../../hw/README.md#lcd)) | USB |
| `DBG:DIAG?` | | block of diagnostic bytes. Only in builds with the `dbg-diag` feature. | USB |
| `DBG:CHG?` | | `<PB10>,<PB11>,<mode>,<pull>,<open drain>,<output>,<USB configured>,<battery ADC>`: the charger status pins' levels; GPIOB mode / pull / output-type / output bits of PB10..PB11 (bits 20..23, 20..23, 10..11, 10..11, shifted down); 1 while a USB host has the meter configured; the sum of 16 battery conversions (V = sum / 16 × 3 / 4096 × 2) | USB |
| `DBG:CHG:PIN` | `10\|11,IN\|UP\|DOWN\|LOW\|HIGH\|OD\|DEF` | Sets PB10 or PB11 for a charger experiment: input without pull, with pull-up or pull-down, driven low or high, open drain released, or back to the default. Until the next restart. | USB |
| `DBG:CHG:LOG?` | | `<changes>,<ms>:<pins>,…`: the last 8 changes of the charger pins since the start, oldest first; pins bit 0 PB10, bit 1 PB11, bit 2 USB configured | USB |
| `DBG:CHG:OUT` | `IN\|UP\|DOWN\|LOW\|HIGH\|OD\|DEF` | `DBG:CHG:PIN 10,…` | USB |
| `DBG:TOUCH[?]` | `0` clears | Touch log text. Only in builds with the `dbg-touch` feature. | USB |
| `DBG:TOUCH:TEST[?]` | `ON`, `OFF` | `1` while the [touch test page](README.md#touch-test) is shown. `ON` is refused while a recording session runs. | USB |
| `DBG:TOUCH:CAL[?]` | `RESET`, or `a,b,c,d,e,f` | The [touch calibration](README.md#touch-calibration) x' = a x + b y + c, y' = d x + e y + f; the query returns the six numbers (`1,0,0,0,1,0` without a calibration). `RESET` removes it. A set outside the page's limits (a, e within 0.85..1.15, b, d within ±0.15, at most 40 px of shift at the targets) is refused with -222. Kept in the settings, as the page's **Save**. | USB |
| `DBG:PROF[?]` | | Main loop timing table. Only in builds with the `dbg-prof` feature. | USB |
| `DBG:PROF:RESET` | | Clears the timing table. Only in builds with the `dbg-prof` feature. | USB |
| `DBG:FLASH?` | | `<flash id>,<blocks>,<failed>,` then screenshot and preparation counters | USB |
| `DBG:ISR?` | | Stream interrupt statistics (23 fields) | USB |
| `DBG:FAST?` | | `RATE FAST` in DC V / mV / A: `<offset measurements used>,<rejected (input moving)>,<offset of the current range, converter codes>,<conversion period, ms (every conversion, the dropped ones too)>` | USB |
| `DBG:GRAB[?]` | hold time in ms, 0 .. 5000 | `0` | USB |
| `DBG:PINCH[?]` | `0` one step per 2.15x of finger span, `1` per 1.25x, `2` live stretch, snapped on release | `1` | USB |
| `DBG:GEST?` | | `<n>;<target>,<axis>,<delta>,<ms>;...\|POS <hpos>,<trig row>,<pos>\|AUTO...`: the last scope drags, the positions now | USB |
| `DBG:ALARm?` | | `<sounding>,<silenced>,<events>,<setting>` | USB |
| `DBG:SENTinel?` | | Wrong-connection detector state. Only in builds with the `dbg-sent` feature. | USB |
| `DBG:SENTinel:LOG?` | | `<n>;<ALERT>,<KIND>,<ms>;...`: the last 8 alerts | USB |
| `DBG:SENTinel:REG?` | | Measurement chip registers and relay state. Only in builds with the `dbg-sent` feature. | USB |
| `DBG:SENTinel:VARiant[?]` | `0` .. `127` | `0` | USB |
| `DBG:SENTinel:PRECond[?]` | ms, 0 .. 2000 | `100` | USB |
| `DBG:SRC` | `<vref>,<path>[,<sense>][,FORCE]` or `OFF`: OP3's reference tap on a stock ohm / diode row, for [`srcmap.py`](../../firmware/tools/srcmap.py) (experimental; refused in the A functions and the power meter; `dbg-src` builds) | | USB |
| `DBG:SRC?` | | `0`, or `1,<cycles>,<vref>,<path>,<sense>,<V_in>,<V_far>,<I>,<V_vref>,<V_drop>,<V_com>,<VDDA-AGND>,<AGND-VSS>;<10 AD1 codes>;<registers hex>`. Only in builds with the `dbg-src` feature. | USB |
| `DBG:OHM6` | `<gain>,<offset uV>,<skip>,<keep>,<leak nS>` (an empty field keeps its value; a leak value stops the learning on open leads, a negative one restarts it): the experimental 600 MΩ range's tuning, RAM only ([ohm-600m.md](../../firmware/docs/ohm-600m.md); `dbg-ohm6` builds) | | USB |
| `DBG:OHM6?` | | `<gain>,<offset uV>,<skip>,<keep>,<leak nS>,<Vx>,<Vfb>` (the leak learned on open leads; V, the last phase medians; `dbg-ohm6` builds) | USB |
| `DBG:ACDC?` | | AC+DC phase diagnostic. Only in builds with the `dbg-acdc` feature. | USB |
| `DBG:ACDC:METH[?]` | `A`, `B` or `C` | `A`. Only in builds with the `acdc-meth` feature. | USB |
| `DBG:ACDC:STAT?` | | AC+DC method state (25 fields). Only in builds with the `acdc-meth` feature. | USB |
| `DBG:ACDC:ZERO[?]` | `<slot 0..9>,<value>\|AUTO` | `<valid>,<fixed>;<c0>,...,<c9>`. Only in builds with the `acdc-meth` feature. | USB |
| `DBG:ACDC:R0[?]` | `1` or `2` | `1`. Only in builds with the `acdc-meth` feature. | USB |
| `DBG:ACDC:RANG` | `<item 0..5>` or `AUTO` | Sets the range item. | USB |
| `DBG:ACDC:TRACE[?]` | `0`, `1` or `2`; query `[<from>]` | `<n>,<held>,<trig>,<on>;<hex>...`. Only in builds with the `acdc-trace` feature. | USB |
| `DBG:HWTRIG?` | | Comparator trigger state | USB |
| `DBG:HWTRIG:ENABLE` | `1` or `0` | | USB |
| `DBG:HWTRIG:CAL?` | | Calibration result or the error name | USB |
| `DBG:HWTRIG:STAT?` | | `<hits>,<misses>,<last>,<min>,<max>,<mean>` | USB |
| `DBG:HWTRIG:STAT:RESET` | | Clears the statistics. | USB |
| `DBG:GENTRIG:LAG[?]` | ns the generator's output reaches CH1 later than its timers say (GEN trigger and trace; not kept) | `+1.850000E+02` (37 ticks of 5 ns) | USB |
| `DBG:GENTRIG:STAT?` | | `<readings>,<failed>,<last window>,<failed window>`: the generator phase readings for the GEN trigger and trace (failed = no consistent reading; the windows in CPU cycles: the last reading's, the narrowest try of the last failure) | USB |
| `DBG:CH2:LAG[?]` | us the DMM CH2's conversion is older than its stamp (CH2 against CH1; not kept) | `+0.000000E+00` | USB |
| `DBG:HYSPI:SELF?` | `<n>[,<div>[,<cpha>]]` | Measurement chip bus self-test report. Only in builds with the `dbg-hyspi` feature. | USB |
| `DBG:HYSPI:WR?` | `<mode 0..3>[,<div>]` | Bus write test report. Only in builds with the `dbg-hyspi` feature. | USB |
| `DBG:AD2:TORN?` | `<r26>,<n>[,SYNChronous]` | Converter read test report (AC rows) | USB |
| `DBG:AD2:NOISE?` | `<r26>,<n>[,<op1chop>]` | Converter noise report (AC rows) | USB |
| `DBG:AD2:DCV?` | `<range>,<r26>,<n>[,<flags>]` | DC V converter test report | USB |
| `DBG:AD2:N7?` | `<item>,<state>,<n>` | DC row test report | USB |
| `DBG:AD2:DRAW` | `0`, `1` or `2` | Logger panel redraw level. | USB |
| `DBG:AD2:DIV` | `0`, `8`, `16` or `32`[,`<cpha>`] | Stream read divider (0 = automatic). | USB |
| `DBG:AD2:DMA[?]` | `0` or `1` | `1` | USB |
| `DBG:POWer:ROW?` | | Power meter chip row, hex | USB |
| `DBG:POWer:RAW?` | | Power meter raw sample ring | USB |
| `DBG:POWer:POKE` | `<register 0..21>,<value>` or `CLEar` | | USB |
| `DBG:POWer:STAT` | `XY`, `XX`, `YY` or `AUTO` | | USB |
| `DBG:POWer:DISC[?]` | `0` .. `8` | `2` | USB |
| `DBG:VDISK:EJECT[?]` | `<n>` | `<ejected>,<start-stops>,<last>` | USB |
| `DBG:VDISK:STAT?` | | `<files>,<next free cluster>,<files left out>` | USB |
| `DBG:VDISK:RO[?]` | `0` or `1` | `<read-only>,<pending>` | USB |
| `DBG:VDISK:RD?` | | USB disk read counters | USB |
| `DBG:VDISK:MIR?` | | USB disk mirror state | USB |
| `DBG:VDISK:WR?` | | Host write counters (13 fields) | USB |
| `DBG:UPLoad:BEGin` | `<size>,<crc32>` (decimal) | Starts a firmware file upload. | USB |
| `DBG:UPLOAD:DATA` | `<offset>,<raw bytes>` | One piece of the file (see notes). | USB |
| `DBG:UPLoad:END?` | | `<size>,<crc32>` when the file is complete and checked | USB |
| `DBG:UPLoad:ABORt` | | Cancels the upload. | USB |
| `DBG:UPLoad?` | | `<active 1/0>,<received>,<size>` | USB |
| `DBG:UPDATE` | | Makes the vendor loader install the uploaded file: the meter restarts. | USB |
| `DBG:LOADER` | | Restarts into the vendor loader's update screen; the installed firmware stays. | USB |
| `DBG:RECOVER` | | Same as `DBG:LOADER`. | USB |
| `DBG:RESET` | | Restarts the meter. | USB |
| `DBG:RELEASE` | | Stops feeding the watchdog: the meter resets in about 20 s and counts a crash. | USB |

- `DBG:UPLOAD:DATA` must be written exactly so, in capitals, followed by one
  space; the bytes after the comma are binary and run to the end of the USB
  message. The offset is the number of bytes sent so far.
  `firmware/tools/dm40dbg.py upload` and `update` do the whole sequence.
- `DBG:UPDATE`: the vendor loader installs the staged file; without one it
  stays on its update screen with its own USB disk until a firmware file is
  copied there.
- `DBG:LOADER`: a staged file is installed; switching the meter off and on
  leaves the loader and starts opendm40.

### DBG:BOOT?

`B2`, then 57 numbers in hex (no `0x`, no leading zeros), then the 8.3
name fatren gave the installed `dm40*.bin` (11 characters, blanks if none).
[`firmware/tools/dm40dbg.py boot`](../../firmware/tools/dm40dbg.py) prints it decoded.

| # | Field |
|---|---|
| 0 | mode: 0 normal, 1 recovery |
| 1 | reset reason: 0 none, 1 watchdog after recovery, 2 watchdog (stale), 3 watchdog (retry), 4 key hold aborted, 5 requested, 6 clock failed |
| 2 | clock: 0 HXTAL, 1 IRC16M, 2 failed |
| 3, 4 | reset flags (RCU_RSTSCK) at reset / after the shim |
| 5 | the previous boot's last word |
| 6 | the last loader trigger (`5452001x`: marker zeroed, failed, PA2 entry; `5452000x`: recovery file) |
| 7..9 | loader marker words |
| 10 | marker valid: 1 / 0 |
| 11 | crash word (`435200nn`: nn watchdog resets in a row) |
| 12 | healthy (60 s of uptime): 1 / 0 |
| 13 | last flash status of the marker zeroing (0 = clean) |
| 14..17 | FSMC BCR1, BTR1, BWTR1 and the panel ID as found at boot |
| 18 | fatren: 0 not run, 1 no medium, 2 bad BPB, 3 clean, 4 dry run, 5 renamed, 6 error |
| 19..25 | fatren: found, old backup, old deleted, freed clusters, others left, recovery file deleted, staged |
| 26 | calibration safety copy: 0 not run, 1 no medium, 2 bad BPB, 3 present, 4 written, 5 no space, 6 error |
| 27..29 | its written / conflict / differs masks (bit 0 CALIB, 1 BOOTDATA, 2 UID) |
| 30 | its time, ms |
| 31, 32 | stack used / stack size, bytes |
| 33, 34 | SPI flash clock divider (4 / 16) and its check: 0 not run, 1 OK, 2 forced, 3 no chip, 4 no signature, 5 / 6 differed (1st / 2nd read) |
| 35..37 | disk folders: ok, created, failed (bit masks) |
| 38 | calibration record CRC: 1 OK, 0 bad |
| 39 | settings from: 0 not run, 1 backup SRAM, 2 file, 3 defaults |
| 40, 41 | the file slot and sequence number used |
| 42 | the settings file at boot: 0 not run, 1 present, 2 created, 3 no file, 4 error |
| 43 | the last settings save: 0 none, 1 OK, 2 deferred, 3 bad record, 4 error, 5 no file |
| 44, 45 | saves this boot, the newest slot's sequence number |
| 46..53 | boot phases, ms: board + splash, fatren, safety copy, folders, settings, mirror, USB disk, refresh |
| 54..56 | main's clock at App::new's end, the first frame, the touch reset, ms |
