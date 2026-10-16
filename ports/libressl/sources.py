#!/usr/bin/env python3
"""The source files of a LibreSSL library, from its automake file.

    sources.py DIR/Makefile.am VARIABLE [CONDITION...]

prints the files VARIABLE collects (`libcrypto_la_SOURCES`, ...), one per
line, with automake's conditionals (`if NAME`, `if !NAME`, `else`, `endif`)
resolved for the CONDITIONs given as true and the `include` directives of
files next to it followed. It stands in for configure, which a cross build
for newlib cannot run (ports/libressl/build.sh).
"""
import pathlib
import sys


def lines(path):
    for line in path.read_text().splitlines():
        s = line.strip()
        if s.startswith("include "):
            inc = s.split(None, 1)[1]
            # $(top_srcdir)/Makefile.am.common holds flags, no sources.
            if not inc.startswith("$("):
                yield from lines(path.parent / inc)
        else:
            yield s


def main():
    makefile = pathlib.Path(sys.argv[1])
    var = sys.argv[2]
    true = set(sys.argv[3:])
    # (this block is active, its condition held): `else` flips the latter
    # within the enclosing block.
    stack = [(True, True)]
    files = []
    for s in lines(makefile):
        if s.startswith("if "):
            cond = s[3:].strip()
            held = (cond.lstrip("!") in true) != cond.startswith("!")
            stack.append((stack[-1][0] and held, held))
        elif s == "else":
            _, held = stack.pop()
            stack.append((stack[-1][0] and not held, not held))
        elif s == "endif":
            stack.pop()
        elif stack[-1][0] and s.startswith(var):
            rest = s[len(var):].lstrip()
            if rest.startswith("+="):
                files += rest[2:].split()
            elif rest.startswith("="):
                files = rest[1:].split()
    if len(stack) != 1:
        sys.exit(f"{makefile}: unbalanced conditionals")
    print("\n".join(files))


if __name__ == "__main__":
    main()
