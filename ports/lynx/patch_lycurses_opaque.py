#!/usr/bin/env python3
"""Use ncurses opaque getbegy/getbegx/getmax* function forms on myos."""
from pathlib import Path
import re
import sys

path = Path(sys.argv[1])
text = path.read_text()
if "MYOS_NCURSES_OPAQUE_MACROS" in text:
    print("already patched", path)
    raise SystemExit(0)

repls = [
    (r"#define getbegy\(win\)[^\n]*\n", "/* MYOS: use ncurses getbegy() */\n"),
    (r"#define getbegx\(win\)[^\n]*\n", "/* MYOS: use ncurses getbegx() */\n"),
    (r"#define getmaxy\(win\)[^\n]*\n", "/* MYOS: use ncurses getmaxy() */\n"),
    (r"#define getmaxx\(win\)[^\n]*\n", "/* MYOS: use ncurses getmaxx() */\n"),
    (r"#define getcury\(win\)[^\n]*\n", "/* MYOS: use ncurses getcury() */\n"),
    (r"#define getcurx\(win\)[^\n]*\n", "/* MYOS: use ncurses getcurx() */\n"),
]
for pat, rep in repls:
    text, n = re.subn(pat, rep, text)
    print(pat, n)

text = "/* MYOS_NCURSES_OPAQUE_MACROS */\n" + text
path.write_text(text)
print("patched", path)
