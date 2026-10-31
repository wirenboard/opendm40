#!/usr/bin/env python3
"""opendm40 misconnect sentinel (firmware/src/sentinel.rs) in the emulator: opendm40
native + the HY3131 model, a timed connection script per scenario, SCPI over
USB/IP (CONF:<f>, then DBG:PING? uptime, SYST:ALER?, DBG:SENT?, READ? in a
loop); each phase of the script must show only its expected alerts.

  emu/hy3131/senttest.py                         # all scenarios, opendm40 built from HEAD
  emu/hy3131/senttest.py --app firmware/opendm40.bin --port 3377 dcv-adc acv
  emu/hy3131/senttest.py --list

Results: emu/out/sent-<name>.json (+ .png of the end; Renode log
emu/out/sent-<name>-<port>-<pid>.log). One emulator at a time per port (a
busy port is refused); run several invocations on different ports in
parallel.
"""
import argparse
import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
EMU = os.path.dirname(HERE)
ROOT = os.path.dirname(EMU)
OUT = os.path.join(EMU, 'out')
ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument('names', nargs='*')
ap.add_argument('--app', help='opendm40 native image (default: run.py builds HEAD)')
ap.add_argument('--port', type=int, default=3377, help='USB/IP port (default 3377)')
ap.add_argument('--list', action='store_true')
A = ap.parse_args()
os.environ['USBIP_PORT'] = str(A.port)
sys.path.insert(0, EMU)
import usbip_client as uc  # noqa: E402
import emusync  # noqa: E402

# name: (SCPI function, run_for, [(t, spec, allowed alerts in this phase)]); a phase is checked
# from its start + 2.5 s to the next phase's start - 1.0 s (the first phase from fw 5 s; the
# last to the run's end); A-JACK clears 1 s after the last hit, V-JACK? needs 1 s + the settle.
# The phase starts are the input markers (emu/emusync.py, README "Input markers"): the fw
# uptime at which the model applied the change. `acts` run at their distance from the start
# of the phase they fall in. The windows were written for inputs at fw ~t - 0.5 s: the
# script puts the changes VLAG later in virtual time (and runs VLAG longer), which keeps
# them there with the boot of emusync.BOOT_S (3.56 s, fresh disk); if the boot changes,
# only the phase lengths move (the markers keep the windows on the inputs).
VLAG = emusync.BOOT_S - 0.5
# The 66 kHz model knobs (the hf scenarios below; INFERRED ratios).
HF = 'hfbw:6000;hfdc:0.3;hfpb1:10;hfpb2:1'
# A bouncing contact (cont-bounce): (s after the phase's input, V-COM ohm; 1e12 = open), set
# through SetOption (no input marker); the phase's own input is the open resistor.
BOUNCE_ON = [(0.05, 814.6), (0.12, 1e12), (0.2, 277), (0.3, 28.85), (0.38, 1e12), (0.45, 28.85), (0.6, 1e12),
             (0.7, 2.1), (0.8, 0.67)]
BOUNCE_OFF = [(0.05, 1e12), (0.15, 552), (0.25, 1e12), (0.32, 2.1), (0.4, 1e12)]
SC = {
    'dcv-adc':  ('VOLT:DC', 26, [(0, 'open;vsrc:1.234', {'NONE'}), (10, 'isrc:0.001', {'AJACK'}), (17, 'isrc:none', {'NONE'})]),
    'dcv-neg':  ('VOLT:DC', 18, [(0, 'open;vsrc:1.234', {'NONE'}), (10, 'isrc:-0.001', {'AJACK'})]),
    'dcv-aac':  ('VOLT:DC', 20, [(0, 'open;vsrc:1.234', {'NONE'}), (10, 'isrc:0,ac=0.005,f=50', {'AJACK'})]),
    'dcv-avs':  ('VOLT:DC', 18, [(0, 'open;vsrc:1.234', {'NONE'}), (10, 'avsrc:5', {'AJACK'})]),
    'dcv-small': ('VOLT:DC', 18, [(0, 'open;vsrc:1.234;isrc:0.0002', {'NONE'})]),
    # f80524f device check: A-JACK with 10 V on the 60 V range, A jack open. The model has no
    # such coupling; this checks that the autorange 6 <-> 60 V leaves the r0 sentinel working.
    'dcv-step': ('VOLT:DC', 36, [(0, 'open;vsrc:1', {'NONE'}), (8, 'vsrc:10', {'NONE'}), (14, 'vsrc:1', {'NONE'}),
                                 (20, 'vsrc:10', {'NONE'}), (26, 'vsrc:1;isrc:0.001', {'AJACK'})]),
    'dcmv':     ('VOLT:DC', 20, [(0, 'open;vsrc:0.1', {'NONE'}), (10, 'isrc:0.002', {'AJACK'})]),
    # (ohm: 4.7 Mohm, the 6 Mohm row, whose jack check line tells a current in A from a source;
    # on the rows below it a current in A shows LIVE?, the gap: ohm-v5-600)
    'ohm':      ('RES', 26, [(0, 'open;r:4.7e6', {'NONE'}), (12, 'isrc:0.002', {'AJACK'})]),
    'temp':     ('TEMP', 26, [(0, 'open;tc:100;cj:25', {'NONE'}), (12, 'isrc:0.005', {'AJACK'})]),
    'acv':      ('VOLT:AC', 26, [(0, 'open;vsrc:0,ac=1.0,f=50', {'NONE'}), (10, 'isrc:0,ac=0.01,f=50', {'AJACK'}), (18, 'isrc:none', {'NONE'})]),
    'acv-dc':   ('VOLT:AC', 20, [(0, 'open;vsrc:0,ac=1.0,f=50', {'NONE'}), (10, 'isrc:0.001', {'AJACK'})]),
    'acmv':     ('VOLT:AC', 20, [(0, 'open;vsrc:0,ac=0.05,f=50', {'NONE'}), (10, 'isrc:0,ac=0.005,f=60', {'AJACK'})]),
    'dca-v':    ('CURR:DC', 28, [(0, 'open;vsrc:5', {'NONE'}), (10, 'vsrc:24', {'VJACK'}), (19, 'vsrc:none', {'NONE'})]),
    'dca-vac':  ('CURR:DC', 20, [(0, 'open', {'NONE'}), (10, 'vsrc:0,ac=230,f=50', {'VJACK'})]),
    'dca-vi':   ('CURR:DC', 16, [(0, 'open;vsrc:24;isrc:0.0123', {'NONE'})]),
    # Device 22c85ba / c2ddd2a (A jack open): 5 / 10 V DC or the 0..9.6 V 40 Hz square on V-COM
    # tripped the comparator on PB1 in ohm / temperature (A-JACK), 1 V did not. The model's
    # Pb1VjackMv puts the V jack on PB1 in those rows. Temperature: the thermocouple input reads
    # far beyond any thermocouple (V_GATE), no A-JACK. Ohm: the device reads OL there, not the
    # negative code of LIVE? (519fb8a 7b; the model's default `ohmrevol`), so the jack check
    # (sentinel.rs JACK CHECK: AD2 on OP1O while PB1 hits) tells it: LIVE?, never A-JACK. A current
    # in A with V-COM open (the last phase) is A-JACK as before. ohm-v5-600: manual 600 ohm.
    'ohm-v5':   ('RES', 42, [(0, 'open;r:4700', {'NONE'}), (8, 'vsrc:5', {'LIVE'}), (16, 'vsrc:10', {'LIVE'}),
                             (24, 'vsrc:4.776,ac=4.776,f=40,shape=square', {'LIVE'}), (32, 'open;isrc:0.002;aiohm:31e6', {'AJACK'})],
                 [], {'monitor': ['sysbus.hy3131 Pb1VjackMv 10'], 'never': [('AJACK', 1, 4)]}),
    # (ohm-v5-600's last phase: on 600 ohm the open input's own source, 2.17 V, is in the band of a
    # source on V-COM, and the reading is OL either way: a DC current in A with V-COM open reads
    # 0x70 as a source does and shows LIVE? there, the gap of doc 4.13)
    'ohm-v5-600': ('RES 600', 42, [(0, 'open;r:470', {'NONE'}), (8, 'vsrc:5', {'LIVE'}), (16, 'vsrc:10', {'LIVE'}),
                                   (24, 'vsrc:4.776,ac=4.776,f=40,shape=square', {'LIVE'}), (32, 'open;isrc:0.002', {'LIVE'})],
                   [], {'monitor': ['sysbus.hy3131 Pb1VjackMv 10'], 'never': [('AJACK', 1, 4)]}),
    # Continuity, the same inputs (device 40728b8 item 4 / d2aff22 S6: A-JACK at 5 V from the AD2
    # peak on PB1; LIVE? at 1 V): the jack check's window says driven -> LIVE?.
    'cont-v5':  ('CONT', 42, [(0, 'open;r:10', {'NONE'}), (8, 'open;vsrc:5', {'LIVE'}), (16, 'vsrc:10', {'LIVE'}),
                              (24, 'vsrc:4.776,ac=4.776,f=40,shape=square', {'LIVE'}), (32, 'open;isrc:0.002', {'AJACK'})],
                 [], {'monitor': ['sysbus.hy3131 Pb1VjackMv 10'], 'never': [('AJACK', 1, 4)]}),
    # AC mV with the 0 / 9.5 V 40 Hz square on V-COM (device 2f56ff9: AJACK,AD1PK_PB1; the model's
    # mvclamp / mvpb1): the row's own result is far beyond the range (V_GATE, MV_DRV_K): no
    # A-JACK; then the source off and an AC current in A with V-COM open: A-JACK. Manual 60 mV /
    # 600 mV (the range stays in the clip); acmv-dc5: 5 V DC (the clamp's DC).
    'acmv-sq60': ('VOLT:AC 0.06', 30, [(0, 'open;mvclamp:1.212;mvpb1:10;vsrc:0,ac=0.003,f=50', {'NONE'}),
                                       (10, 'vsrc:4.776,ac=4.776,f=40,shape=square', {'NONE'}),
                                       (20, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [], {'never': [('AJACK', 1, 2)]}),
    'acmv-sq600': ('VOLT:AC 0.6', 30, [(0, 'open;mvclamp:1.212;mvpb1:10;vsrc:0,ac=0.003,f=50', {'NONE'}),
                                       (10, 'vsrc:4.776,ac=4.776,f=40,shape=square', {'NONE'}),
                                       (20, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [], {'never': [('AJACK', 1, 2)]}),
    'acmv-dc5': ('VOLT:AC 0.6', 30, [(0, 'open;mvclamp:1.212;mvpb1:10;vsrc:0,ac=0.003,f=50', {'NONE'}),
                                     (10, 'vsrc:5', {'NONE'}),
                                     (20, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [], {'never': [('AJACK', 1, 2)]}),
    # A 66 kHz signal on V-COM (device 2026-10-26, A jack empty: AC mV 600 mV AJACK,AD1PK_PB1 at
    # 26.6 mV, DC mV 600 mV OL with CMP_PB1 A-JACK / A-PROT, AC V 6 V a DC? hint at 1.16 mV). The
    # model's HF knobs (INFERRED): the RMS blocks roll off above 6 kHz (`hfbw`), the DC mV node and
    # the AC V tap carry 0.3 x the rms as DC (`hfdc`: DC mV OL), PB1 10 mV / V and PB2 1 mV / V of
    # the jack (`hfpb1` / `hfpb2`). AC mV: the counter's 66 kHz says driven (sentinel V_HF_KHZ); DC mV:
    # its OL result (V_GATE), also for A-PROT; AC V: no DC?. The last phase: a current in A with
    # V-COM open (DC mV) or a small 50 Hz on it (AC mV) still asserts A-JACK.
    'acmv-hf600': ('VOLT:AC 0.6', 30, [(0, 'open;%s;vsrc:0,ac=0.003,f=50' % HF, {'NONE'}),
                                       (10, 'vsrc:0,ac=3,f=66000', {'NONE'}),
                                       (20, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [],
                   {'never': [('AJACK', 1, 2), ('APROT', 0, 3)]}),
    'acmv-hf60': ('VOLT:AC 0.06', 30, [(0, 'open;%s;vsrc:0,ac=0.003,f=50' % HF, {'NONE'}),
                                       (10, 'vsrc:0,ac=3,f=66000', {'NONE'}),
                                       (20, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [],
                  {'never': [('AJACK', 1, 2), ('APROT', 0, 3)]}),
    'dcmv-hf600': ('VOLT:DC 0.6', 30, [(0, 'open;%s;vsrc:0.1' % HF, {'NONE'}),
                                       (10, 'vsrc:0,ac=3,f=66000', {'NONE'}),
                                       (20, 'vsrc:none;isrc:0.002', {'AJACK'})], [],
                   {'never': [('AJACK', 1, 2), ('APROT', 0, 3)]}),
    # The same at the SLOW rate (0.4 s conversions; the V gate's lag is 250 ms) and through AC V
    # AUTO (mV-first, as the owner switched).
    'dcmv-hf600-slow': ('VOLT:DC 0.6', 30, [(0, 'open;%s;vsrc:0.1' % HF, {'NONE'}),
                                            (10, 'vsrc:0,ac=3,f=66000', {'NONE'}),
                                            (20, 'vsrc:none;isrc:0.002', {'AJACK'})], ['SENS:RATE SLOW'],
                        {'never': [('AJACK', 1, 2), ('APROT', 0, 3)]}),
    'acv-auto-hf': ('VOLT:AC', 30, [(0, 'open;%s;vsrc:0,ac=0.003,f=50' % HF, {'NONE'}),
                                    (10, 'vsrc:0,ac=3,f=66000', {'NONE'}),
                                    (20, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [],
                    {'never': [('AJACK', 1, 2), ('APROT', 0, 3), ('DC', 1, 2)]}),
    # The device check of 2026-10-26 (0.2.4): the MR9270's 66 kHz output, a 0 / 9.5 V square, on
    # V-COM. AC mV 600 mV: AJACK,AD1PK_PB1 (~30 mV RMS); the counter did not say driven (the
    # model's `hfnocnt`); the input clips the square (`mvclamp`, its PB1 pickup `mvpb1`): the LPF's
    # ~0.6 V says driven (MV_DC_DRV_MV). DC V AUTO: AJACK,CMP_PB1 once in the log.
    'acmv-sq66k-600': ('VOLT:AC 0.6', 30, [(0, 'open;%s;hfnocnt:1;mvclamp:1.212;mvpb1:10;vsrc:0,ac=0.003,f=50' % HF, {'NONE'}),
                                           (10, 'vsrc:4.776,ac=4.776,f=66000,shape=square', {'NONE'}),
                                           (20, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [],
                       {'never': [('AJACK', 1, 2), ('APROT', 0, 3)]}),
    'acmv-sq66k-60': ('VOLT:AC 0.06', 30, [(0, 'open;%s;hfnocnt:1;mvclamp:1.212;mvpb1:10;vsrc:0,ac=0.003,f=50' % HF, {'NONE'}),
                                           (10, 'vsrc:4.776,ac=4.776,f=66000,shape=square', {'NONE'}),
                                           (20, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [],
                      {'never': [('AJACK', 1, 2), ('APROT', 0, 3)]}),
    # (`hfboth`: the comparator on PB1 reads 0x70 and 0 in turn, as the device in DC V AUTO / DC mV)
    'dcv-auto-sq66k': ('VOLT:DC', 30, [(0, 'open;%s;hfnocnt:1;hfboth:0.3;mvclamp:1.212;mvpb1:10;vsrc:1' % HF, {'NONE'}),
                                       (10, 'vsrc:4.776,ac=4.776,f=66000,shape=square', {'NONE'}),
                                       (20, 'vsrc:none;isrc:0.002', {'AJACK'})], [],
                       {'never': [('AJACK', 1, 2), ('APROT', 0, 3)]}),
    'dcmv-sq66k': ('VOLT:DC 0.6', 30, [(0, 'open;%s;hfnocnt:1;hfboth:0.3;mvclamp:1.212;mvpb1:10;vsrc:0.1' % HF, {'NONE'}),
                                       (10, 'vsrc:4.776,ac=4.776,f=66000,shape=square', {'NONE'}),
                                       (20, 'vsrc:none;isrc:0.002', {'AJACK'})], [],
                   {'never': [('AJACK', 1, 2), ('APROT', 0, 3)]}),
    'acv-hf':     ('VOLT:AC 6', 24, [(0, 'open;%s;vsrc:0,ac=1,f=50' % HF, {'NONE'}),
                                     (10, 'vsrc:0,ac=3,f=66000', {'NONE'})], [], {'never': [('DC', 1, 2)]}),
    # AC+DC V on manual 600 mV (doc 4.13 gap): its AC phase is the AC mV row with the same AD1
    # peak sentinel; V_GATE takes the AC phase's result there too (both phases share the run).
    'acdcmv-sq600': ('VOLT:ACDC 0.6', 34, [(0, 'open;mvclamp:1.212;mvpb1:10;vsrc:0,ac=0.003,f=50', {'NONE'}),
                                           (10, 'vsrc:4.776,ac=4.776,f=40,shape=square', {'NONE'}),
                                           (22, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [],
                     {'never': [('AJACK', 1, 2)]}),
    'acdcmv-dc5': ('VOLT:ACDC 0.6', 34, [(0, 'open;mvclamp:1.212;mvpb1:10;vsrc:0,ac=0.003,f=50', {'NONE'}),
                                         (10, 'vsrc:5', {'NONE'}),
                                         (22, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [],
                   {'never': [('AJACK', 1, 2)]}),
    # The same, then the source switched off with the input left open (device 185c066: the readout
    # OL, AJACK,AD1PK_PB1 0.45-0.9 s after the off in 8 of 13 runs, held while open). The model's
    # mvhold: the floating input keeps 0.75 V (OL on 600 mV, under MV_DRV_K x the range) and PB1
    # picks up 40 mV (INFERRED); -dcrow: only the DC mV row sees the level (the AC row reads 0).
    # Never A-JACK until a current in A (the last phase, the hold ended by a source on V-COM).
    'acdcmv-sq600-off': ('VOLT:ACDC 0.6', 42, [(0, 'open;mvclamp:1.212;mvpb1:10;mvhold:0.75;mvholdpb1:40;vsrc:0,ac=0.003,f=50', {'NONE'}),
                                               (10, 'vsrc:4.776,ac=4.776,f=40,shape=square', {'NONE'}),
                                               (20, 'vsrc:none', {'NONE'}),
                                               (30, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [],
                         {'never': [('AJACK', 1, 3)]}),
    'acdcmv-dc5-off': ('VOLT:ACDC 0.6', 42, [(0, 'open;mvclamp:1.212;mvpb1:10;mvhold:0.75;mvholdpb1:40;vsrc:0,ac=0.003,f=50', {'NONE'}),
                                             (10, 'vsrc:5', {'NONE'}),
                                             (20, 'vsrc:none', {'NONE'}),
                                             (30, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [],
                       {'never': [('AJACK', 1, 3)]}),
    'acdcmv-off-dcrow': ('VOLT:ACDC 0.6', 42, [(0, 'open;mvclamp:1.212;mvpb1:10;mvhold:0.75;mvholdpb1:40;mvholdrows:1;vsrc:0,ac=0.003,f=50', {'NONE'}),
                                               (10, 'vsrc:5', {'NONE'}),
                                               (20, 'vsrc:none', {'NONE'}),
                                               (30, 'vsrc:0,ac=0.003,f=50;isrc:0,ac=0.005,f=60', {'AJACK'})], [],
                         {'never': [('AJACK', 1, 3)]}),
    'temp-v5':  ('TEMP', 42, [(0, 'open;tc:100;cj:25', {'NONE'}), (8, 'vsrc:5', {'NONE'}), (16, 'vsrc:10', {'NONE'}),
                              (24, 'vsrc:4.776,ac=4.776,f=40,shape=square', {'NONE'}), (32, 'open;isrc:0.005', {'AJACK'})],
                 [], {'monitor': ['sysbus.hy3131 Pb1VjackMv 10']}),
    'ohm-live': ('RES', 22, [(0, 'open;r:4700', {'NONE'}), (10, 'vsrc:-1.0,rs=100', {'LIVE'})]),
    'cont-live': ('CONT', 18, [(0, 'open;r:10', {'NONE'}), (8, 'open;vsrc:1.234', {'LIVE'})]),
    'diode-live': ('DIOD', 18, [(0, 'open;diode:0.65', {'NONE'}), (8, 'open;vsrc:-1.0,rs=100', {'LIVE'})]),
    'masked':   ('VOLT:DC', 18, [(0, 'open;vsrc:1.234;isrc:0.001', {'NONE'})]),
    # Sentinels added after f80524f (doc 4.7): AC+DC (both phases), AC A (AD1 on the V tap),
    # frequency (AD1 peak on PB1), continuity / diode / cap r0..4 (AD2 + OP2 on PB1); cap r5..7
    # (the ramp) has none: the gap is asserted.
    'acdc-v':   ('VOLT:ACDC', 30, [(0, 'open;vsrc:1.234', {'NONE'}), (10, 'isrc:0.001', {'AJACK'}), (21, 'isrc:none', {'NONE'})]),
    'acdc-vac': ('VOLT:ACDC', 22, [(0, 'open;vsrc:0,ac=1.0,f=50', {'NONE'}), (10, 'isrc:0,ac=0.005,f=50', {'AJACK'})]),
    # AC+DC V above 6 V: both phases without a sentinel (the DC V 60 V+ gap), no flapping
    'acdc-v-hi-gap': ('VOLT:ACDC', 26, [(0, 'open;vsrc:30', {'NONE'}), (10, 'isrc:0.001', {'NONE'})]),
    'acdc-a':   ('CURR:ACDC', 30, [(0, 'open', {'NONE'}), (10, 'vsrc:24', {'VJACK'}), (21, 'vsrc:none', {'NONE'})]),
    # AC A counts the AD1 swing only (sentinel.rs PKV_SWING_THR, from ~2 V rms): a DC level on
    # SENSE cannot be told from the A rows' own offset (device 40728b8 2a); DC on V is DC A's
    'aca-v':    ('CURR:AC', 28, [(0, 'open;vsrc:0,ac=1,f=50', {'NONE'}), (10, 'vsrc:0,ac=5,f=50', {'VJACK'}), (19, 'vsrc:none', {'NONE'})]),
    'aca-vdc':  ('CURR:AC', 18, [(0, 'open;vsrc:24', {'NONE'})]),
    'aca-vac':  ('CURR:AC', 20, [(0, 'open', {'NONE'}), (10, 'vsrc:0,ac=230,f=50', {'VJACK'})]),
    'aca-vi':   ('CURR:AC', 18, [(0, 'open;vsrc:24;isrc:0,ac=0.01,f=50', {'NONE'})]),
    # Frequency: A-JACK on a one-sided PB1 - PB3 level above PK_DC_THR (~180 mV; negative:
    # PK_DC_NEG_THR, ~280 mV) only; the row's +2.5 mV with V-COM open (PR #55, SAD1I 00; the
    # model's `hzpb1`) under it: 1 mA in A (~50 mV with it) shows nothing there. 10 V DC on
    # V-COM puts -140 mV there (PR #55): nothing either.
    'freq-gap': ('FREQ', 24, [(0, 'open;hzpb1:2.5;vsrc:0,ac=1,f=1000', {'NONE'}), (10, 'isrc:0.001', {'NONE'})]),
    'freq-v10': ('FREQ', 20, [(0, 'open;hzpb1:-140;vsrc:10', {'NONE'})]),
    # A current in A with the V jack open (device 2026-10-26, the owner's PSU 20 mA CC into A /
    # COM: AC mV, ohm and Hz showed no A-JACK; DC V / AC V / continuity / diode / capacitance
    # did). Ohm: PB1 above the window reads 0x70 (the model's `pb1cmp70`, default), as a source on
    # V-COM does; RES AUTO sits on the 60 Mohm row, whose jack check reads OP1O at its own source
    # (1.6 V in the model) and the reading is in range (31 Mohm on the device: `aiohm`): A-JACK,
    # never LIVE?. AC mV: the open input carries 0.146 x PB1 as DC
    # and 1.5 V RMS of 50 kHz (`ailift` / `ainz`, INFERRED: the device read OL and Hz ~50 kHz),
    # which says driven (the counter, the RMS); PB1 pinned above the node's level overrides it.
    # Hz: PB1 - PB3 pinned. Each: the current off again, and (Hz) a signal on V-COM first.
    'ohm-acur': ('RES', 30, [(0, 'open;aiohm:31e6', {'NONE'}), (10, 'isrc:0.02', {'AJACK'}), (20, 'isrc:none', {'NONE'})], [],
                 {'never': [('LIVE', 1, 3)], 'queries': ['RES:RANG?']}),
    'acmv-acur': ('VOLT:AC 0.6', 30, [(0, 'open;ailift:0.146;ainz:1500', {'NONE'}), (10, 'isrc:0.02', {'AJACK'}),
                                      (20, 'isrc:none', {'NONE'})]),
    'freq-acur': ('FREQ', 30, [(0, 'open;hzpb1:2.5;vsrc:0,ac=1,f=1000', {'NONE'}), (10, 'vsrc:none;isrc:0.02', {'AJACK'}),
                               (20, 'isrc:none', {'NONE'})]),
    # (continuity / diode: A-JACK only while V-COM reads OL, as with the leads in A / COM; with a
    # part still on V-COM -closed- a current in A is no A-JACK there: cont-closed-a)
    'cont-a':   ('CONT', 24, [(0, 'open;r:10', {'NONE'}), (10, 'open;isrc:0.002', {'AJACK'}), (17, 'isrc:none', {'NONE'})]),
    'cont-closed-a': ('CONT', 18, [(0, 'open;r:10', {'NONE'}), (10, 'isrc:0.002', {'NONE'})]),
    # Owner 2026-10-26 (video): the probes touched together in continuity (leads in V / COM, A jack
    # empty) raised A-JACK for ~1.5 s while the readout went OL -> 814 -> 277 -> 28.85 -> 0.67 ohm.
    # The model's contbump (INFERRED shape: a PB1 burst on every V-COM resistor change in the
    # continuity row) with a bouncing contact (`bounce`: the resistor stepped without markers, s
    # after the phase's input); the jack check reads ~0 V there ("free"). Two taps, then a current
    # in A with V-COM open: A-JACK. diode-bounce: the same in diode.
    'cont-bounce': ('CONT', 40, [(0, 'open;r:1e12;contbump:150', {'NONE'}), (8, 'r:1e12', {'NONE'}), (14, 'r:1e12', {'NONE'}),
                                 (20, 'r:1e12', {'NONE'}), (26, 'r:1e12', {'NONE'}), (32, 'isrc:0.002', {'AJACK'})],
                    [], {'never': [('AJACK', 1, 5)], 'bounce': {1: BOUNCE_ON, 2: BOUNCE_OFF, 3: BOUNCE_ON, 4: BOUNCE_OFF}}),
    'diode-bounce': ('DIOD', 40, [(0, 'open;r:1e12;contbump:150', {'NONE'}), (8, 'r:1e12', {'NONE'}), (14, 'r:1e12', {'NONE'}),
                                  (20, 'r:1e12', {'NONE'}), (26, 'r:1e12', {'NONE'}), (32, 'isrc:0,ac=0.005,f=50', {'AJACK'})],
                     [], {'never': [('AJACK', 1, 5)], 'bounce': {1: BOUNCE_ON, 2: BOUNCE_OFF, 3: BOUNCE_ON, 4: BOUNCE_OFF}}),
    'diode-a':  ('DIOD', 24, [(0, 'open;diode:0.65', {'NONE'}), (10, 'open;isrc:0,ac=0.005,f=50', {'AJACK'}), (17, 'isrc:none', {'NONE'})]),
    'cap-a':    ('CAP', 26, [(0, 'open;c:100e-9', {'NONE'}), (12, 'isrc:0.002', {'AJACK'})]),
    'cap-ramp-gap': ('CAP', 30, [(0, 'open;c:100e-6', {'NONE'}), (14, 'isrc:0.002', {'NONE'})]),
    # Hints (doc 4.7): DC? in AC V (AD1 alternates PB1 / the V tap), AC? in DC V 6 V (the
    # comparator alternates PB1 / SENSE), FUSE? after 60 s at zero in an A function.
    'acv-dchint': ('VOLT:AC', 32, [(0, 'open;vsrc:0,ac=1,f=50', {'NONE'}), (10, 'vsrc:5', {'NONE', 'DC'}),
                                   (16, 'vsrc:5', {'DC'}), (23, 'vsrc:0,ac=1,f=50', {'NONE'})]),
    'acv-dc-ac': ('VOLT:AC', 20, [(0, 'open;vsrc:5,ac=1,f=50', {'NONE'})]),
    'acv-dc-ajack': ('VOLT:AC', 26, [(0, 'open;vsrc:5', {'NONE', 'DC'}), (10, 'isrc:0.001', {'AJACK'}), (18, 'isrc:none', {'NONE', 'DC'})]),
    'dcv-achint': ('VOLT:DC', 32, [(0, 'open;vsrc:1', {'NONE'}), (10, 'vsrc:0,ac=2,f=50', {'NONE', 'AC'}),
                                   (15, 'vsrc:0,ac=2,f=50', {'AC'}), (23, 'vsrc:1', {'NONE'})]),
    'dcv-ac-ajack': ('VOLT:DC', 24, [(0, 'open;vsrc:0,ac=2,f=50', {'NONE', 'AC'}), (10, 'isrc:0.001', {'AJACK'})]),
    'fuse-hint': ('CURR:DC', 76, [(0, 'open;fuseopen', {'NONE'}), (62, 'open;fuseopen', {'NONE', 'FUSE'}),
                                  (66, 'open;fuseopen', {'FUSE'})]),
    # Fuse check (SYST:FUSE:TEST, doc 4.7): a lead V -> A, the fuse intact / blown, a short V-COM.
    'fuse-ok':   ('CURR:DC', 20, [(0, 'open;valead', {'NONE'})], ['SYST:FUSE:TEST'],
                  {'queries': ['SYST:FUSE?', 'FUNC?'], 'fuse': 'OK', 'ohm': (44, 54)}),
    # the same through the UI: MF -> Current Settings -> Fuse check -> Start
    'fuse-ui':   ('CURR:DC', 22, [(0, 'open;valead', {'NONE'})], ['DBG:KEY 3', 'DBG:TAP 237,159', 'DBG:TAP 344,210'],
                  {'queries': ['SYST:FUSE?', 'FUNC?'], 'fuse': 'OK', 'ohm': (44, 54)}),
    'fuse-open': ('CURR:DC', 20, [(0, 'open;valead;fuseopen', {'NONE'})], ['SYST:FUSE:TEST'],
                  {'queries': ['SYST:FUSE?', 'FUNC?'], 'fuse': 'OPEN'}),
    # owner on ea73ea5: "the fuse check sometimes reads 18 ohm instead of 48" (the third raw
    # conversion after the fast ohm entry, the node still charging): with the model's ohm node
    # lag (800 ms, chosen so the old third conversion is ~13 % low) the verdict must come from a
    # vetted final reading (dmm.rs FUSE_READINGS / OHM_VET_*)
    'fuse-ok-lag': ('CURR:DC', 20, [(0, 'open;valead;ohmtau:800', {'NONE'})], ['SYST:FUSE:TEST'],
                    {'queries': ['SYST:FUSE?', 'FUNC?'], 'fuse': 'OK', 'ohm': (44, 54)}),
    'fuse-odd':  ('CURR:DC', 20, [(0, 'open;r:0.5', {'NONE'})], ['SYST:FUSE:TEST'],
                  {'queries': ['SYST:FUSE?', 'FUNC?'], 'fuse': 'FAIL', 'ohm': (0, 2)}),
    # OVERLOAD (doc 4.7): a MANUAL uA / mA range over range -> the 10 A shunt at once, latched;
    # a phase may carry a READ? window (lo, hi) in A. AUTO (owner 2026-10-22, as the power
    # meter's 3.14): the shunt's top range over is a quiet hop to the A shunt (6 A), no alert in
    # any sample, the walk down as usual (50 mA: back on the mA shunt).
    'ovl-dcma': ('CURR:DC', 40, [(0, 'open;isrc:0.01', {'NONE'}, (0.0099, 0.0101)),
                                 (10, 'isrc:2', {'NONE'}, (1.98, 2.02)),
                                 (20, 'isrc:0.05', {'NONE'}),
                                 (32, 'isrc:0.05', {'NONE'}, (0.0499, 0.0501))]),
    # Manual 60 mA: 2 A -> OVERLOAD on 10 A; then 40 mA (fits 60 mA: below 0.85 x 60 mA) for 5 s
    # -> back on manual 60 mA with its resolution (owner 2026-10-22: the release at half the
    # scale, 30 mA, kept the red OVLD for good).
    'ovl-dcma-man': ('CURR:DC', 44, [(0, 'open;isrc:0.01', {'NONE'}, (0.0099, 0.0101)),
                                     (10, 'isrc:2', {'OVERLOAD'}, (1.98, 2.02)),
                                     (20, 'isrc:0.04', {'OVERLOAD', 'NONE'}),
                                     (32, 'isrc:0.04', {'NONE'}, (0.03995, 0.04005))],
                     ['CURR:DC:RANG 0.06'], {'queries': ['CURR:DC:RANG?']}),
    # Manual 600 uA (Current Settings page): 10 mA -> 10 A (manual); 0 A for 5 s -> back to
    # 600 uA manual (100 uA reads with uA resolution again); 1 mA within 30 s of that release
    # trips again and the latch is sticky (0 A no longer releases it).
    'ovl-dcua-man': ('CURR:DC', 56, [(0, 'open;isrc:0.0001', {'NONE'}, (99e-6, 101e-6)),
                                     (12, 'isrc:0.01', {'OVERLOAD'}, (0.008, 0.012)),
                                     (22, 'isrc:none', {'OVERLOAD', 'NONE'}),
                                     (30, 'isrc:0.0001', {'NONE'}, (99.5e-6, 100.5e-6)),
                                     (38, 'isrc:0.001', {'OVERLOAD'}),
                                     (44, 'isrc:none', {'OVERLOAD'})],
                     ['DBG:KEY 3', 'DBG:TAP 64,70', 'DBG:KEY 3']),
    'ovl-acma': ('CURR:AC', 26, [(0, 'open;isrc:0,ac=0.01,f=50', {'NONE'}, (0.0098, 0.0102)),
                                 (10, 'isrc:0,ac=2,f=50', {'NONE'}, (1.95, 2.05))]),
    'ovl-acdc': ('CURR:ACDC', 30, [(0, 'open;isrc:0.01', {'NONE'}), (12, 'isrc:2', {'NONE'}, (1.97, 2.03))]),
    # Manual AC 600 mA: 2 A -> OVERLOAD (the latch is for manual ranges only now).
    'ovl-acma-man': ('CURR:AC', 26, [(0, 'open;isrc:0,ac=0.1,f=50', {'NONE'}, (0.098, 0.102)),
                                     (10, 'isrc:0,ac=2,f=50', {'OVERLOAD'}, (1.95, 2.05))], ['CURR:AC:RANG 0.6']),
    # Power meter, I fixed on 600 mA (CONF:POW AUTO,0.6): 2 A -> ik 5.
    'ovl-pow':  ('POW AUTO,0.6', 30, [(0, 'open;pow:230,i=0.1,f=50', {'NONE'}), (12, 'pow:230,i=2,f=50', {'OVERLOAD'})], ['POW:COUP ACDC']),
    # Power meter, I AUTO (owner: 3 A through A showed OVLD): an over range on the mA / uA
    # shunt is the normal up-range to the next shunt, no alert in any sample (READ? = P).
    'ovl-pow-auto-ma': ('POW', 30, [(0, 'open;pow:230,i=0.01,f=50', {'NONE'}, (2.25, 2.35)),
                                    (12, 'pow:230,i=3,f=50', {'NONE'}, (680, 700))], ['POW:COUP ACDC']),
    'ovl-pow-auto-ua': ('POW', 34, [(0, 'open;pow:230,i=0.0002,f=50', {'NONE'}, (0.044, 0.048)),
                                    (16, 'pow:230,i=3,f=50', {'NONE'}, (680, 700))], ['POW:COUP ACDC']),
    'ovl-pow-auto-dc': ('POW', 30, [(0, 'open;vsrc:12;isrc:0.01', {'NONE'}, (0.118, 0.122)),
                                    (12, 'vsrc:12;isrc:3', {'NONE'}, (35.6, 36.4))], ['POW:COUP DC']),
    # A-jack protection (doc 4.8): a source across A-COM in DC V / ohm / temperature. 5 V (104 mA
    # in the 47.5 ohm chain) is a warning only; 12 V (250 mA) moves the A jack to the 10 mohm
    # shunt 0.5 s after the current started, 24 V (0.5 A) at the first probe window. The latch
    # (APROT) holds until the user acts (CONF:... at `acts`) and the current is gone.
    # 'aprot_ms': the trip latency window (DBG:SENT? field 11, ms from the first hit window).
    'aprot-dcv-5': ('VOLT:DC', 24, [(0, 'open', {'NONE'}), (8, 'avsrc:5', {'AJACK'})]),
    'aprot-dcv-12': ('VOLT:DC', 32, [(0, 'open', {'NONE'}), (8, 'avsrc:12', {'APROT'}), (16, 'open', {'APROT'}),
                                     (22, 'open', {'NONE'})], [], {'acts': {19.5: 'DBG:KEY 2'}, 'aprot_ms': (400, 560)}),
    'aprot-dcv-24': ('VOLT:DC', 22, [(0, 'open', {'NONE'}), (8, 'avsrc:24', {'APROT'})], [], {'aprot_ms': (100, 450)}),
    # DC mV (its V_GATE since the 66 kHz fix: a trip needs a DC mV result inside the range since
    # the hit run began): the same sources, the latency of the trip.
    'aprot-dcmv-12': ('VOLT:DC 0.6', 22, [(0, 'open', {'NONE'}), (8, 'avsrc:12', {'APROT'})], [], {'aprot_ms': (400, 800)}),
    'aprot-dcmv-24': ('VOLT:DC 0.6', 22, [(0, 'open', {'NONE'}), (8, 'avsrc:24', {'APROT'})], [], {'aprot_ms': (100, 800)}),
    # Ohm / temperature carry no probe (device 22c85ba: there it follows the V input, 5 V on V-COM
    # read 1.7 A and latched A-PROT): a source across A-COM is A-JACK only.
    # (4.7 Mohm: the 6 Mohm row, as `ohm`)
    'aprot-ohm-12': ('RES', 24, [(0, 'open;r:4.7e6', {'NONE'}), (10, 'avsrc:12', {'AJACK'})]),
    'aprot-temp-12': ('TEMP', 26, [(0, 'open;tc:100;cj:25', {'NONE'}), (12, 'avsrc:12', {'AJACK'})]),
    # The user acts (another function) while the source is still there: the latch stays; the current goes later and
    # the latch ends 1 s after that (the act counts).
    'aprot-ack-early': ('VOLT:DC', 34, [(0, 'open', {'NONE'}), (8, 'avsrc:12', {'APROT'}), (16, 'avsrc:12', {'APROT'}),
                                        (22, 'open', {'APROT', 'NONE'}), (26, 'open', {'NONE'})], [], {'acts': {14: 'DBG:KEY 2'}}),
    # Device d8e249d: A-PROT latched with the A jack open on the probe's first window (AD2 just
    # restarted). The model's AD2 reads 3000 codes (0.72 A) off for its first results after a
    # start: a small current (1 V across A-COM, 21 mA) brings the probe in; A-JACK only.
    'aprot-glitch': ('VOLT:DC', 22, [(0, 'open', {'NONE'}), (8, 'avsrc:1', {'AJACK'})], [],
                     {'monitor': ['sysbus.hy3131 Ad2StartGlitch 3000']}),
    # A current function ends the latch (its own shunt switching takes over).
    # (DC A AUTO: the source's amps on the mA shunt are a quiet hop to the A shunt, no OVERLOAD)
    # CONF:CURR:DC at 17 (was 16: 0.5 s before the APROT window's end, and the latch ends
    # ~0.3 s after it, so a poll in between failed the run now and then).
    'aprot-to-dca': ('VOLT:DC', 30, [(0, 'open', {'NONE'}), (8, 'avsrc:12', {'APROT'}), (18, 'avsrc:12', {'NONE'})],
                     [], {'acts': {17: 'CONF:CURR:DC'}}),
}


def run(name):
    func, run_for, phases = SC[name][:3]
    setup = SC[name][3] if len(SC[name]) > 3 else []
    xtra = SC[name][4] if len(SC[name]) > 4 else {}
    script = [(ph[0], ph[1]) for ph in phases]
    vt = [t + VLAG if t > 0 else t for t, _ in script]
    if uc.port_busy(A.port):
        print('RESULT %s FAIL port %d in use (another emulator?): pass a free --port' % (name, A.port), flush=True)
        return False
    tag = 'sent-' + name
    # The run's own files (event script, Renode log, flash image, .resc) per
    # process: parallel runs (distinct --port) share none; results under tag.
    rtag = '%s-%d-%d' % (tag, A.port, os.getpid())
    os.makedirs(OUT, exist_ok=True)
    sp = os.path.join(OUT, rtag + '.script')
    with open(sp, 'w') as f:
        for m in xtra.get('monitor', []):
            f.write('0 monitor %s\n' % m)
        for v, (t, spec) in zip(vt, script):
            f.write('%g dmm %s\n' % (v, spec))
        for i, steps in xtra.get('bounce', {}).items():
            for dt, ohm in steps:
                f.write('%g monitor sysbus.hy3131 SetOption "r" %g\n' % (vt[i] + dt, ohm))
    # USB/IP from 3 s: an attach before opendm40 enumerated can leave the server stuck
    cmd = [sys.executable, os.path.join(EMU, 'run.py'), 'fwrs', '--fast', '--usbip', '3',
           '--usbip-port', str(A.port), '--run-for', str(run_for + VLAG), '--script', sp, '--name', rtag,
           '--shot', os.path.join(OUT, tag + '.png')] + (['--app', os.path.abspath(A.app)] if A.app else [])
    p = subprocess.Popen(cmd, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    mk = emusync.Markers(emusync.log_path(OUT, rtag))
    # Phase times as the fw uptimes the scenario was written for (the inputs at fw t - 0.5):
    # `acts` (fw uptime values) keep their distance from their phase's input.
    tl = emusync.Timeline([(t - 0.5 if t > 0 else t, spec) for t, spec in script], run_for + VLAG, mk, vt=vt)
    rows, early = [], []
    t_wall = time.time()
    tm = None
    while p.poll() is None and tm is None and time.time() - t_wall < 300:
        try:
            d = uc.Dev('1-0')
            tm = uc.Usbtmc(d)
        except Exception as e:
            print('attach:', type(e).__name__, e, flush=True)
            time.sleep(1)
    if tm is not None:
        try:
            if name == 'masked':
                tm.write('SYST:ALER:ENAB 6')
            tm.write('CONF:' + func)
            if mk.wait_clock() is None:
                print(name, 'no clock marker (dwt: CYCCNT write) in the log', flush=True)
            # (the setup after the clock marker: under host load a fast act, the fuse check, could
            # end before the first sample and its BUSY was never seen)
            for c in setup:
                time.sleep(1.0)
                tm.write(c)
            if 'fuse' in xtra:
                # the check's BUSY right after the start (it can end within ~1 s, before the
                # first sample row)
                tm.write('SYST:FUSE?')
                early.append(tm.read(timeout=60).decode(errors='replace').strip())
            # fw uptime: the distance from the start of the act's phase (predicted by the
            # clock marker = the value of the phase's input marker)
            acts = {tl.at(ta, predict=True) if mk.zero is not None else ta: c for ta, c in xtra.get('acts', {}).items()}
            # DBG:SENTinel? needs --features dbg-sent (refused: an empty reply,
            # its checks skipped).
            tm.write('DBG:SENT?')
            has_sent = bool(tm.read(timeout=60).decode(errors='replace').strip())
            if not has_sent:
                tm.write('*CLS')
                print('%s: DBG:SENT? not in this build (--features dbg-sent): its checks skipped' % name, flush=True)
            while p.poll() is None:
                row = {}
                logq = ['DBG:SENT:LOG?'] if 'never' in xtra else []
                for q in ['DBG:PING?', 'SYST:ALER?'] + ['DBG:SENT?'] * has_sent + ['STAT:QUES:COND?', 'READ?'] + xtra.get('queries', []) + logq:
                    tm.write(q)
                    row[q] = tm.read(timeout=60).decode(errors='replace').strip()
                row['t'] = int(row['DBG:PING?'].split(',')[0]) / 1000.0
                rows.append(row)
                for ta in sorted(acts):
                    if row['t'] >= ta:
                        tm.write(acts.pop(ta))
                print('%s %6.2f %-6s %-40s %s %s' % (name, row['t'], row['SYST:ALER?'], row.get('DBG:SENT?', '-'), row['READ?'],
                                                   ' '.join(row[q] for q in xtra.get('queries', []))), flush=True)
        except Exception as e:
            print(name, 'link ended:', e, flush=True)
    out, _ = p.communicate()
    for x in (sp, os.path.join(OUT, 'flash-%s.bin' % rtag), os.path.join(OUT, rtag + '.resc')):
        if os.path.exists(x):
            os.remove(x)
    bad = emusync.check_sync(mk, tl, name)
    ok = not bad
    expect = []
    for i, ph in enumerate(phases):
        b0, b1 = tl.start(i), tl.stop(i)
        if b0 is None or b1 is None:
            continue  # its marker is missing: check_sync failed the run
        expect.append((b0 + 2.5 if i > 0 else 5.0, b1 - 1.0 if i + 1 < len(phases) else b1, ph[2], ph[3] if len(ph) > 3 else None))
    for t0, t1, allowed, win in expect:
        seen = [r for r in rows if t0 <= r['t'] <= t1]
        if not seen:
            ok = False
            bad.append('no samples in %g..%g' % (t0, t1))
        for r in seen:
            if r['SYST:ALER?'] not in allowed:
                ok = False
                bad.append('%.2f %s not in %s' % (r['t'], r['SYST:ALER?'], sorted(allowed)))
            # STAT:QUES:COND? bit 9 = a red alert, bit 1 = OVERLOAD
            want = (512 if r['SYST:ALER?'] in ('AJACK', 'OVERLOAD', 'APROT') else 0) | (2 if r['SYST:ALER?'] == 'OVERLOAD' else 0)
            if len(allowed) == 1 and r.get('STAT:QUES:COND?') != str(want):
                ok = False
                bad.append('%.2f QUES %s for %s' % (r['t'], r.get('STAT:QUES:COND?'), r['SYST:ALER?']))
            if win:
                try:
                    v = float(r['READ?'])
                except ValueError:
                    v = float('nan')
                if not (win[0] <= abs(v) <= win[1]):
                    ok = False
                    bad.append('%.2f READ? %s outside %g..%g' % (r['t'], r['READ?'], win[0], win[1]))
            # DC V 60 V+ carries no comparator sentinel (f80524f device check)
            try:
                v = float(r['READ?'])
            except ValueError:
                v = 0.0
            if name == 'dcv-step' and 7.0 < v < 1e6 and 'DBG:SENT?' in r and not r['DBG:SENT?'].startswith('OFF'):
                ok = False
                bad.append('%.2f %g V: sentinel %s on the 60 V range' % (r['t'], v, r['DBG:SENT?'].split(',')[0]))
    if 'never' in xtra:
        # alerts that must never come on from phase i0's input to phase i1's (every assertion,
        # from the firmware's log: also the transients between two samples and in the first
        # 2.5 s of a phase that the windows above skip)
        ents = set()
        for r in rows:
            for e in r.get('DBG:SENT:LOG?', '').split(';')[1:]:
                f = e.split(',')
                if len(f) == 3:
                    ents.add((f[0], f[1], int(f[2]) / 1000.0))
        for al, i0, i1 in xtra['never']:
            b0 = tl.start(i0)
            b1 = tl.start(i1) if i1 < len(phases) else 1e9
            if b0 is None or b1 is None:
                continue
            hit = sorted(e for e in ents if e[0] == al and b0 <= e[2] < b1)
            if hit:
                ok = False
                bad.append('%s logged at %s' % (al, ' '.join('%s@%.2f' % (e[1], e[2]) for e in hit)))
    if 'aprot_ms' in xtra and not any('DBG:SENT?' in r for r in rows):
        print('  skip the A-PROT latency (DBG:SENT? not in this build)', flush=True)
    elif 'aprot_ms' in xtra:
        # the trip latency the firmware recorded (DBG:SENT? field 11)
        ms = [int(r['DBG:SENT?'].split(',')[10]) for r in rows if r['SYST:ALER?'] == 'APROT' and len(r['DBG:SENT?'].split(',')) > 10]
        lo, hi = xtra['aprot_ms']
        if not ms or not (lo <= ms[-1] <= hi):
            ok = False
            bad.append('A-PROT latency %s ms, want %d..%d' % (ms[-1:] or None, lo, hi))
        else:
            print('  A-PROT latency %d ms' % ms[-1], flush=True)
    if 'fuse' in xtra:
        # the verdict came, the function came back (CURR), the ohm value in its window
        fz = early + [r['SYST:FUSE?'] for r in rows if 'SYST:FUSE?' in r]
        done = [f for f in fz if f not in ('BUSY', 'IDLE')]
        if not done or not done[-1].startswith(xtra['fuse']):
            ok = False
            bad.append('SYST:FUSE? %s, want %s' % (fz[-6:], xtra['fuse']))
        elif 'ohm' in xtra:
            v = float(done[-1].split(',')[1])
            if not (xtra['ohm'][0] <= v <= xtra['ohm'][1]):
                ok = False
                bad.append('fuse ohm %g outside %s' % (v, xtra['ohm']))
        if 'BUSY' not in fz:
            ok = False
            bad.append('never BUSY')
        if not rows or rows[-1].get('FUNC?') != '"CURR"':
            ok = False
            bad.append('function after the check: %s' % (rows[-1].get('FUNC?') if rows else None))
    res = {'name': name, 'ok': ok, 'bad': bad, 'rows': rows, 'rc': p.returncode, 'log': out[-600:] if p.returncode else '',
           'sync': emusync.summary(mk, tl)}
    with open(os.path.join(OUT, tag + '.json'), 'w') as f:
        json.dump(res, f, indent=1)
    print('RESULT %s %s %s' % (name, 'OK' if ok else 'FAIL', '; '.join(bad[:5])), flush=True)
    return ok


if __name__ == '__main__':
    if A.list:
        for n, v in SC.items():
            print('%-11s %-8s %s' % (n, v[0], ' | '.join('%g %s -> %s' % (x[0], x[1], '/'.join(sorted(x[2]))) for x in v[2])))
        sys.exit(0)
    allok = True
    for n in A.names or list(SC):
        allok &= run(n)
    sys.exit(0 if allok else 1)
