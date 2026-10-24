#!/usr/bin/env bash
# pin.sh YYYY-MM-DD: rewrite versions.env for that nightly from its channel
# manifest (static.rust-lang.org/dist/DATE/channel-rust-nightly.toml): the
# sha256s of the musl rustc, cargo and rust-std for each host arch.
# toolchain/std/bump-nightly.sh --write runs it with the date it pins.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
date="${1:?usage: pin.sh YYYY-MM-DD}"
manifest="$(mktemp)"
trap 'rm -f "$manifest"' EXIT
curl -fsSL -o "$manifest" "https://static.rust-lang.org/dist/$date/channel-rust-nightly.toml"
python3 - "$manifest" "$date" "$HERE/versions.env" <<'PY'
import re, sys, tomllib
manifest, date, out = sys.argv[1:]
pkg = tomllib.load(open(manifest, "rb"))["pkg"]
text = open(out).read()
lines = [l for l in text.splitlines() if l.startswith("#")]
lines.append(f"RUST_NIGHTLY={date}")
hosts = {"x86_64": "x86_64-unknown-linux-musl", "aarch64": "aarch64-unknown-linux-musl",
         "riscv64": "riscv64gc-unknown-linux-musl"}
for arch, host in hosts.items():
    for comp, var in (("rustc", "RUSTC"), ("cargo", "CARGO"), ("rust-std", "STD")):
        t = pkg[comp]["target"][host]
        if not t.get("available"):
            sys.exit(f"{comp} for {host} is not in the {date} nightly")
        lines.append(f"RUST_{var}_{arch}={t['xz_hash']}")
open(out, "w").write("\n".join(lines) + "\n")
PY
echo "pinned nightly-$date in $HERE/versions.env"
