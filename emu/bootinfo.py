"""DBG:BOOT? for the emulator tests: the fields by name, decoded by
firmware/tools/dm40dbg.py `parse_boot` (the reply is "B2," + hex words;
docs/manual/scpi-reference.md "DBG:BOOT?")."""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'firmware', 'tools'))
from dm40dbg import parse_boot  # noqa: E402


def fields(text):
    """{name: int | enum name | text}; {} for NONE or another format."""
    f, _ = parse_boot(text)
    return f or {}


def settings(text):
    """The settings part as a list: SETTINGS,<from>,<slot | ->,<seq>,
    SETFILE,<state>,SAVE,<status>,<saves>,<newest seq> ([] without it)."""
    f = fields(text)
    if 'save_top' not in f:
        return []
    return ['SETTINGS', f['settings_from'], str(f['settings_slot']) if f['settings_from'] == 'FILE' else '-',
            str(f['settings_seq']), 'SETFILE', f['setfile'], 'SAVE', f['save'], str(f['saves']), str(f['save_top'])]
