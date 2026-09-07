#!/usr/bin/env python3
"""Rewrite HTTLS.c for MYOS_MBEDTLS_TIDY_TLS: keep SSL_* tidy_tls API, drop raw gnutls_*."""
from pathlib import Path
import re
import sys

path = Path(sys.argv[1])
text = path.read_text()

old_inc = """#ifdef USE_GNUTLS_INCL
#include <gnutls/x509.h>
#endif"""
new_inc = """/* myos: tidy_tls is mbedtls-backed; do not pull real GnuTLS headers. */
#if defined(USE_GNUTLS_INCL) && !defined(MYOS_MBEDTLS_TIDY_TLS)
#include <gnutls/x509.h>
#endif"""
if old_inc in text:
    text = text.replace(old_inc, new_inc, 1)

def guard_gnutls_blocks(src: str) -> str:
    lines = src.splitlines(keepends=True)
    out = []
    i = 0
    while i < len(lines):
        line = lines[i]
        is_if = (
            line.startswith("#ifdef USE_GNUTLS_INCL")
            or line.startswith("#if defined(USE_GNUTLS_INCL)")
            or ("defined(USE_GNUTLS_INCL)" in line and line.startswith("#if"))
        )
        if is_if and "MYOS_MBEDTLS_TIDY_TLS" not in line:
            depth = 0
            j = i
            block = []
            while j < len(lines):
                l = lines[j]
                if re.match(r"^#if", l):
                    depth += 1
                elif re.match(r"^#endif", l):
                    depth -= 1
                    block.append(l)
                    j += 1
                    if depth == 0:
                        break
                    continue
                block.append(l)
                j += 1
            block_text = "".join(block)
            if "gnutls_" in block_text:
                first = block[0]
                if "defined(USE_GNUTLS_INCL)" in first:
                    first = first.replace(
                        "defined(USE_GNUTLS_INCL)",
                        "defined(USE_GNUTLS_INCL) && !defined(MYOS_MBEDTLS_TIDY_TLS)",
                    )
                elif first.startswith("#ifdef USE_GNUTLS_INCL"):
                    first = "#if defined(USE_GNUTLS_INCL) && !defined(MYOS_MBEDTLS_TIDY_TLS)\n"
                block[0] = first
                out.extend(block)
                i = j
                continue
        out.append(line)
        i += 1
    return "".join(out)

text = guard_gnutls_blocks(text)

needle = "static BOOL cert_valid(SSL * handle, char *host)\n{"
if needle in text:
    start = text.find(needle)
    # only wrap once
    window = text[start : start + 500]
    if "MYOS_MBEDTLS_TIDY_TLS" not in window:
        i = text.find("{", start)
        depth = 0
        end = None
        for k in range(i, len(text)):
            if text[k] == "{":
                depth += 1
            elif text[k] == "}":
                depth -= 1
                if depth == 0:
                    end = k + 1
                    break
        if end is None:
            raise SystemExit("cert_valid end not found")
        body = text[start:end]
        open_brace = body.find("{")
        new_body = (
            body[: open_brace + 1]
            + "\n#ifdef MYOS_MBEDTLS_TIDY_TLS\n"
            + "    (void) handle;\n"
            + "    (void) host;\n"
            + "    return TRUE; /* verified in SSL_connect (mbedtls) */\n"
            + "#else\n"
            + body[open_brace + 1 : -1]
            + "\n#endif /* MYOS_MBEDTLS_TIDY_TLS */\n}"
        )
        text = text[:start] + new_body + text[end:]

path.write_text(text)
print(f"patched {path}")
