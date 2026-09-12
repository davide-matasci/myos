#!/usr/bin/env bash
# Fetch errno + libc + rustix from crates.io and apply myos patches into target/patched-crates/.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=ports/coreutils/versions.env
source "$ROOT/ports/coreutils/versions.env"

CRATES="$HERE/crates"
LIBC="$ROOT/ports/crates/libc"
DEST="$ROOT/target/patched-crates"
STAMP="$DEST/.coreutils-patches-version"
FETCH_DIR="$ROOT/target/crate-fetch-coreutils"
NIGHTLY="${MYOS_NIGHTLY:-nightly-2026-07-26}"

patch_version_hash() {
  {
    echo "errno=$ERRNO_VERSION libc=$LIBC_VERSION rustix=$RUSTIX_VERSION"
    echo "getrandom=$GETRANDOM_02_VERSION,$GETRANDOM_04_VERSION"
    echo "hostname=$HOSTNAME_VERSION console=$CONSOLE_VERSION filetime=$FILETIME_VERSION ctrlc=$CTRLC_VERSION blake3=$BLAKE3_VERSION"
    sha256sum "$HERE/versions.env"
    sha256sum "$CRATES/errno/"*
    sha256sum "$LIBC/"*
    sha256sum "$CRATES/rustix/"*
    sha256sum "$CRATES/getrandom/"*
    sha256sum "$CRATES/hostname/"*
    sha256sum "$CRATES/console/"*
    sha256sum "$CRATES/filetime/"*
    sha256sum "$CRATES/ctrlc/"*
    # Bust the stamp whenever this script changes (e.g. a patched-in-place
    # registry crate needs re-patching with new heal logic).
    sha256sum "$HERE/prepare.sh"
  } | sha256sum | awk '{print $1}'
}

find_registry_crate() {
  local name_ver="$1"
  local -a roots=(
    "${CARGO_HOME:-$HOME/.cargo}/registry/src"
    "/usr/local/cargo/registry/src"
  )
  local root dir
  for root in "${roots[@]}"; do
    for dir in "$root"/index.crates.io-*; do
      [[ -d "$dir/$name_ver" ]] || continue
      printf '%s\n' "$dir/$name_ver"
      return 0
    done
  done
  echo "error: could not find $name_ver in cargo registry (run cargo fetch)" >&2
  return 1
}

# In-place registry patching must be deterministic: restore the crate from
# the cargo download cache first so a crate dir corrupted by an earlier
# buggy patch pass (or double-patched by overlapping hunks) always heals.
reset_registry_crate() {
  local name_ver="$1" dir="$2" cache
  cache="$(dirname "$dir")/../../cache/$(basename "$(dirname "$dir")")/$name_ver.crate"
  if [[ -f "$cache" ]]; then
    rm -rf "$dir"
    tar -xzf "$cache" -C "$(dirname "$dir")"
    # Restore cargo's extraction marker — without it cargo silently
    # re-extracts the pristine crate on the next build, wiping the
    # in-place patches we just applied.
    printf '{"v":1}' >"$dir/.cargo-ok"
  fi
}

GETRANDOM_04_SRC="$(find_registry_crate "getrandom-$GETRANDOM_04_VERSION" 2>/dev/null || true)"
HOSTNAME_SRC="$(find_registry_crate "hostname-$HOSTNAME_VERSION" 2>/dev/null || true)"
CONSOLE_SRC="$(find_registry_crate "console-$CONSOLE_VERSION" 2>/dev/null || true)"
FILETIME_SRC="$(find_registry_crate "filetime-$FILETIME_VERSION" 2>/dev/null || true)"
CTRLC_SRC="$(find_registry_crate "ctrlc-$CTRLC_VERSION" 2>/dev/null || true)"

if [[ -f "$STAMP" ]] && [[ "$(cat "$STAMP")" == "$(patch_version_hash)" ]] \
  && [[ -f "$DEST/errno-$ERRNO_VERSION/Cargo.toml" ]] \
  && [[ -f "$DEST/libc-$LIBC_VERSION/Cargo.toml" ]] \
  && [[ -f "$DEST/rustix-$RUSTIX_VERSION/Cargo.toml" ]] \
  && [[ -f "$DEST/getrandom-$GETRANDOM_02_VERSION/Cargo.toml" ]] \
  && [[ -n "$GETRANDOM_04_SRC" ]] \
  && [[ -f "$GETRANDOM_04_SRC/src/backends/myos.rs" ]] \
  && [[ -n "$HOSTNAME_SRC" ]] \
  && [[ -f "$HOSTNAME_SRC/src/myos.rs" ]] \
  && [[ -n "$CONSOLE_SRC" ]] \
  && [[ -f "$CONSOLE_SRC/src/myos_term.rs" ]] \
  && grep -A40 'pub fn family' "$CONSOLE_SRC/src/term.rs" | grep -q 'target_os = "myos"' \
  && [[ -n "$FILETIME_SRC" ]] \
  && [[ -f "$FILETIME_SRC/src/myos.rs" ]] \
  && grep -q 'target_os = "myos"' "$FILETIME_SRC/src/lib.rs" \
  && [[ -n "$CTRLC_SRC" ]] \
  && [[ -f "$CTRLC_SRC/src/platform/myos.rs" ]]; then
  echo "coreutils patched crates up to date at $DEST"
  exit 0
fi

echo "Fetching errno $ERRNO_VERSION, libc $LIBC_VERSION, rustix $RUSTIX_VERSION, getrandom, hostname, console, filetime, ctrlc..."
mkdir -p "$FETCH_DIR"
cat >"$FETCH_DIR/Cargo.toml" <<EOF
[package]
name = "coreutils-crate-fetch"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]

[lib]
path = "lib.rs"

[dependencies]
errno = "= $ERRNO_VERSION"
libc = "= $LIBC_VERSION"
rustix = "= $RUSTIX_VERSION"
getrandom02 = { package = "getrandom", version = "= $GETRANDOM_02_VERSION" }
getrandom04 = { package = "getrandom", version = "= $GETRANDOM_04_VERSION" }
hostname = "= $HOSTNAME_VERSION"
console = "= $CONSOLE_VERSION"
filetime = "= $FILETIME_VERSION"
ctrlc = { version = "= $CTRLC_VERSION", features = ["termination"] }
blake3 = "= $BLAKE3_VERSION"
EOF
echo '// fetch-only dummy crate' >"$FETCH_DIR/lib.rs"

( cd "$FETCH_DIR" && cargo "+$NIGHTLY" fetch -q )

ERRNO_SRC="$(find_registry_crate "errno-$ERRNO_VERSION")"
LIBC_SRC="$(find_registry_crate "libc-$LIBC_VERSION")"
RUSTIX_SRC="$(find_registry_crate "rustix-$RUSTIX_VERSION")"

rm -rf "$DEST"
mkdir -p "$DEST"

apply_patches() {
  local crate_src="$1"
  local crate_name="$2"
  local patch_dir="$3"
  local out="$DEST/$crate_name"

  echo "==> patching $crate_name"
  cp -a "$crate_src" "$out"
  if [[ -f "$patch_dir/myos.rs" ]] \
    && [[ "$crate_name" != "getrandom-$GETRANDOM_04_VERSION" ]]; then
    cp "$patch_dir/myos.rs" "$out/src/myos.rs"
  fi
  if [[ -f "$patch_dir/myos-0.4.rs" ]] \
    && [[ "$crate_name" == "getrandom-$GETRANDOM_04_VERSION" ]]; then
    cp "$patch_dir/myos-0.4.rs" "$out/src/backends/myos.rs"
  fi
  if [[ -f "$patch_dir/rustix_compat.rs" ]]; then
    cp "$patch_dir/rustix_compat.rs" "$out/src/rustix_compat.rs"
  fi
  for patch in "$patch_dir"/*.patch; do
    [[ -f "$patch" ]] || continue
    case "$(basename "$patch")" in
      lib-rs.patch|sys-rs.patch|lib-rs-0.2.patch|backends-rs-0.4.patch) continue ;;
    esac
    patch -d "$out" -p1 --forward <"$patch"
  done
  if [[ -f "$patch_dir/lib-rs.patch" ]]; then
    patch -d "$out" -p1 --forward <"$patch_dir/lib-rs.patch"
  fi
  if [[ -f "$patch_dir/lib-rs-0.2.patch" ]] \
    && [[ "$crate_name" == "getrandom-$GETRANDOM_02_VERSION" ]]; then
    patch -d "$out" -p1 --forward <"$patch_dir/lib-rs-0.2.patch"
  fi
  if [[ -f "$patch_dir/backends-rs-0.4.patch" ]] \
    && [[ "$crate_name" == "getrandom-$GETRANDOM_04_VERSION" ]]; then
    patch -d "$out" -p1 --forward <"$patch_dir/backends-rs-0.4.patch"
  fi
  if [[ -f "$patch_dir/sys-rs.patch" ]]; then
    patch -d "$out" -p1 --forward <"$patch_dir/sys-rs.patch"
  fi
}

GETRANDOM_02_SRC="$(find_registry_crate "getrandom-$GETRANDOM_02_VERSION")"
GETRANDOM_04_SRC="$(find_registry_crate "getrandom-$GETRANDOM_04_VERSION")"

apply_patches "$ERRNO_SRC" "errno-$ERRNO_VERSION" "$CRATES/errno"
apply_patches "$LIBC_SRC" "libc-$LIBC_VERSION" "$LIBC"
apply_patches "$RUSTIX_SRC" "rustix-$RUSTIX_VERSION" "$CRATES/rustix"
apply_patches "$GETRANDOM_02_SRC" "getrandom-$GETRANDOM_02_VERSION" "$CRATES/getrandom"
# getrandom 0.4.x is a separate semver line; patch the registry copy in-place
# (Cargo [patch.crates-io] can only redirect one getrandom source).
echo "==> patching getrandom-$GETRANDOM_04_VERSION (registry in-place)"
reset_registry_crate "getrandom-$GETRANDOM_04_VERSION" "$GETRANDOM_04_SRC"
cp "$CRATES/getrandom/myos-0.4.rs" "$GETRANDOM_04_SRC/src/backends/myos.rs"
if ! grep -q 'target_os = "myos"' "$GETRANDOM_04_SRC/src/backends.rs"; then
  patch -d "$GETRANDOM_04_SRC" -p1 --forward <"$CRATES/getrandom/backends-rs-0.4.patch"
fi

# myos uses a fixed-arity fcntl/ioctl libc shim; adjust rustix call sites.
RUSTIX_OUT="$DEST/rustix-$RUSTIX_VERSION"
for f in \
  "$RUSTIX_OUT/src/backend/libc/io/syscalls.rs" \
  "$RUSTIX_OUT/src/backend/libc/fs/syscalls.rs" \
  "$RUSTIX_OUT/src/backend/libc/process/syscalls.rs"; do
  [[ -f "$f" ]] || continue
  # Portable in-place edit (BSD sed -i differs from GNU).
  python3 - "$f" <<'PYRUSTIX'
from pathlib import Path
import sys

p = Path(sys.argv[1])
text = p.read_text()
repls = [
    ("c::fcntl(borrowed_fd(fd), c::F_GETFD))",
     "c::fcntl(borrowed_fd(fd), c::F_GETFD, 0))"),
    ("c::F_SETFL, flags.bits())",
     "c::F_SETFL, flags.bits() as c::c_ulong)"),
    ("c::F_SETFD, flags.bits())",
     "c::F_SETFD, flags.bits() as c::c_ulong)"),
    ("c::F_GETLK, &mut curr_lock)",
     "c::F_GETLK, (&mut curr_lock as *mut c::flock as c::c_ulong))"),
    ("(&mut curr_lock as *mut c::flock).cast()",
     "(&mut curr_lock as *mut c::flock as c::c_ulong)"),
    ("c::fcntl(borrowed_fd(fd), cmd, &lock)",
     "c::fcntl(borrowed_fd(fd), cmd, (&lock as *const c::flock as c::c_ulong))"),
    ("(&lock as *const c::flock).cast()",
     "(&lock as *const c::flock as c::c_ulong)"),
    ("c::F_DUPFD_CLOEXEC, min)",
     "c::F_DUPFD_CLOEXEC, min as c::c_ulong)"),
]
for old, new in repls:
    text = text.replace(old, new)
p.write_text(text)
PYRUSTIX
done

patch_registry_hostile_crates() {
  local hostname_src console_src
  hostname_src="$(find_registry_crate "hostname-$HOSTNAME_VERSION")"
  console_src="$(find_registry_crate "console-$CONSOLE_VERSION")"
  reset_registry_crate "hostname-$HOSTNAME_VERSION" "$hostname_src"
  reset_registry_crate "console-$CONSOLE_VERSION" "$console_src"

  echo "==> patching hostname-$HOSTNAME_VERSION (registry in-place)"
  cp "$CRATES/hostname/myos.rs" "$hostname_src/src/myos.rs"
  if ! grep -q 'target_os = "myos"' "$hostname_src/src/lib.rs"; then
    python3 - "$hostname_src/src/lib.rs" <<'PYHOST'
from pathlib import Path
import sys

p = Path(sys.argv[1])
lines = p.read_text().splitlines(keepends=True)
insert = [
    '    } else if #[cfg(target_os = "myos")] {\n',
    '        mod myos;\n',
    '        use crate::myos as sys;\n',
]
for i, line in enumerate(lines):
    if 'use crate::nix as sys;' in line:
        lines[i + 1 : i + 1] = insert
        break
else:
    raise SystemExit("hostname: 'use crate::nix as sys;' line not found")
p.write_text("".join(lines))
PYHOST
  fi

  echo "==> patching console-$CONSOLE_VERSION (registry in-place)"
  cp "$CRATES/console/myos_term.rs" "$console_src/src/myos_term.rs"
  if ! grep -q 'mod myos_term' "$console_src/src/lib.rs"; then
    python3 - "$console_src/src/lib.rs" <<'PYCONSOLE'
from pathlib import Path
import sys

p = Path(sys.argv[1])
lines = p.read_text().splitlines(keepends=True)
insert = [
    '#[cfg(target_os = "myos")]\n',
    'mod myos_term;\n',
]
for i, line in enumerate(lines):
    if line.rstrip() == 'mod wasm_term;':
        lines[i + 1 : i + 1] = insert
        break
else:
    raise SystemExit("console: 'mod wasm_term;' line not found")
p.write_text("".join(lines))
PYCONSOLE
  fi
  if ! grep -q 'pub(crate) use crate::myos_term' "$console_src/src/term.rs"; then
    python3 - "$console_src/src/term.rs" <<'PYTERMUSE'
from pathlib import Path
import sys

p = Path(sys.argv[1])
lines = p.read_text().splitlines(keepends=True)
insert = [
    '#[cfg(target_os = "myos")]\n',
    'pub(crate) use crate::myos_term::*;\n',
]
for i, line in enumerate(lines):
    if line.rstrip() == 'pub(crate) use crate::unix_term::*;':
        lines[i + 1 : i + 1] = insert
        break
else:
    raise SystemExit("console: 'pub(crate) use crate::unix_term::*;' line not found")
p.write_text("".join(lines))
PYTERMUSE
  fi
  # myos is not unix/windows/wasm — family() needs an explicit arm or it returns ().
  # NOTE: do not key off bare "target_os = myos" in term.rs (myos_term use already
  # adds that) or TermFamily::Dummy (wasm already has Dummy). Require myos *inside* family().
  if ! grep -A40 'pub fn family' "$console_src/src/term.rs" | grep -q 'target_os = "myos"'; then
    python3 - "$console_src/src/term.rs" <<'PYTERM'
from pathlib import Path
import sys
p = Path(sys.argv[1])
text = p.read_text()
needle = """        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            TermFamily::UnixTerm
        }
        #[cfg(target_arch = "wasm32")]
"""
insert = """        #[cfg(all(unix, not(target_arch = "wasm32")))]
        {
            TermFamily::UnixTerm
        }
        #[cfg(target_os = "myos")]
        {
            TermFamily::Dummy
        }
        #[cfg(target_arch = "wasm32")]
"""
if needle not in text:
    raise SystemExit("console family(): expected unix/wasm arms not found")
if 'target_os = "myos"' in text[text.index("pub fn family") : text.index("pub fn family") + 500]:
    print("console family myos arm already present")
else:
    p.write_text(text.replace(needle, insert, 1))
    print("console family myos arm applied")
PYTERM
  fi

  echo "==> patching filetime-$FILETIME_VERSION (registry in-place)"
  filetime_src="$(find_registry_crate "filetime-$FILETIME_VERSION")"
  reset_registry_crate "filetime-$FILETIME_VERSION" "$filetime_src"
  cp "$CRATES/filetime/myos.rs" "$filetime_src/src/myos.rs"
  if ! grep -q 'target_os = "myos"' "$filetime_src/src/lib.rs"; then
    python3 - "$filetime_src/src/lib.rs" <<'PYFT'
from pathlib import Path
import sys
p = Path(sys.argv[1])
text = p.read_text()
needle = """    } else if #[cfg(all(target_family = "wasm", not(target_os = "emscripten")))] {
        #[path = "wasm.rs"]
        mod imp;
    } else {"""
insert = """    } else if #[cfg(all(target_family = "wasm", not(target_os = "emscripten")))] {
        #[path = "wasm.rs"]
        mod imp;
    } else if #[cfg(target_os = "myos")] {
        #[path = "myos.rs"]
        mod imp;
    } else {"""
if needle not in text:
    raise SystemExit("filetime cfg_if arm not found")
p.write_text(text.replace(needle, insert, 1))
print("filetime myos arm applied")
PYFT
  fi

  echo "==> patching ctrlc-$CTRLC_VERSION (registry in-place)"
  ctrlc_src="$(find_registry_crate "ctrlc-$CTRLC_VERSION")"
  reset_registry_crate "ctrlc-$CTRLC_VERSION" "$ctrlc_src"
  mkdir -p "$ctrlc_src/src/platform"
  cp "$CRATES/ctrlc/myos.rs" "$ctrlc_src/src/platform/myos.rs"
  if ! grep -q 'target_os = "myos"' "$ctrlc_src/src/platform/mod.rs"; then
    python3 - "$ctrlc_src/src/platform/mod.rs" <<'PYCC'
from pathlib import Path
import sys
p = Path(sys.argv[1])
text = p.read_text()
old = """#[cfg(unix)]
mod unix;

#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use self::unix::*;

#[cfg(windows)]
pub use self::windows::*;
"""
new = """#[cfg(unix)]
mod unix;

#[cfg(windows)]
mod windows;

#[cfg(target_os = "myos")]
mod myos;

#[cfg(unix)]
pub use self::unix::*;

#[cfg(windows)]
pub use self::windows::*;

#[cfg(target_os = "myos")]
pub use self::myos::*;
"""
if old not in text:
    raise SystemExit("ctrlc platform mod pattern not found")
p.write_text(text.replace(old, new, 1))
print("ctrlc myos platform arm applied")
PYCC
  fi

  echo "==> patching blake3-$BLAKE3_VERSION (registry in-place, disable NEON on myos)"
  blake3_src="$(find_registry_crate "blake3-$BLAKE3_VERSION")"
  if ! grep -q 'Ok("myos")' "$blake3_src/build.rs"; then
    python3 - "$blake3_src/build.rs" <<'PYB3'
from pathlib import Path
import sys
p = Path(sys.argv[1])
text = p.read_text()
old = """fn is_pure() -> bool {
    defined("CARGO_FEATURE_PURE")
}"""
new = """fn is_pure() -> bool {
    // myos freestanding clang has no arm_neon.h; force portable Rust path.
    defined("CARGO_FEATURE_PURE")
        || std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("myos")
}"""
if old not in text:
    raise SystemExit("blake3 is_pure() not found")
p.write_text(text.replace(old, new, 1))
print("blake3 myos pure path applied")
PYB3
  fi
}

patch_registry_hostile_crates

patch_version_hash >"$STAMP"
echo "Patched crates ready under $DEST"
