#!/usr/bin/env bash
# Fetch errno + libc + rustix from crates.io and apply myos patches into target/patched-crates/.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=patches/coreutils/versions.env
source "$ROOT/patches/coreutils/versions.env"

PATCHES="$ROOT/patches/coreutils"
DEST="$ROOT/target/patched-crates"
STAMP="$DEST/.coreutils-patches-version"
FETCH_DIR="$ROOT/target/crate-fetch-coreutils"
NIGHTLY="${MYOS_NIGHTLY:-nightly-2026-07-26}"

patch_version_hash() {
  {
    echo "errno=$ERRNO_VERSION libc=$LIBC_VERSION rustix=$RUSTIX_VERSION"
    echo "getrandom=$GETRANDOM_02_VERSION,$GETRANDOM_04_VERSION"
    sha256sum "$PATCHES/versions.env"
    sha256sum "$PATCHES/errno/"*
    sha256sum "$PATCHES/libc/"*
    sha256sum "$PATCHES/rustix/"*
    sha256sum "$PATCHES/getrandom/"*
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

GETRANDOM_04_SRC="$(find_registry_crate "getrandom-$GETRANDOM_04_VERSION" 2>/dev/null || true)"

if [[ -f "$STAMP" ]] && [[ "$(cat "$STAMP")" == "$(patch_version_hash)" ]] \
  && [[ -f "$DEST/errno-$ERRNO_VERSION/Cargo.toml" ]] \
  && [[ -f "$DEST/libc-$LIBC_VERSION/Cargo.toml" ]] \
  && [[ -f "$DEST/rustix-$RUSTIX_VERSION/Cargo.toml" ]] \
  && [[ -f "$DEST/getrandom-$GETRANDOM_02_VERSION/Cargo.toml" ]] \
  && [[ -n "$GETRANDOM_04_SRC" ]] \
  && [[ -f "$GETRANDOM_04_SRC/src/backends/myos.rs" ]]; then
  echo "coreutils patched crates up to date at $DEST"
  exit 0
fi

echo "Fetching errno $ERRNO_VERSION, libc $LIBC_VERSION, rustix $RUSTIX_VERSION, getrandom..."
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
  local patch_subdir="$3"
  local out="$DEST/$crate_name"

  echo "==> patching $crate_name"
  cp -a "$crate_src" "$out"
  if [[ -f "$PATCHES/$patch_subdir/myos.rs" ]] \
    && [[ "$crate_name" != "getrandom-$GETRANDOM_04_VERSION" ]]; then
    cp "$PATCHES/$patch_subdir/myos.rs" "$out/src/myos.rs"
  fi
  if [[ -f "$PATCHES/$patch_subdir/myos-0.4.rs" ]] \
    && [[ "$crate_name" == "getrandom-$GETRANDOM_04_VERSION" ]]; then
    cp "$PATCHES/$patch_subdir/myos-0.4.rs" "$out/src/backends/myos.rs"
  fi
  if [[ -f "$PATCHES/$patch_subdir/rustix_compat.rs" ]]; then
    cp "$PATCHES/$patch_subdir/rustix_compat.rs" "$out/src/rustix_compat.rs"
  fi
  for patch in "$PATCHES/$patch_subdir"/*.patch; do
    [[ -f "$patch" ]] || continue
    case "$(basename "$patch")" in
      lib-rs.patch|sys-rs.patch|lib-rs-0.2.patch|backends-rs-0.4.patch) continue ;;
    esac
    patch -d "$out" -p1 --forward <"$patch"
  done
  if [[ -f "$PATCHES/$patch_subdir/lib-rs.patch" ]]; then
    patch -d "$out" -p1 --forward <"$PATCHES/$patch_subdir/lib-rs.patch"
  fi
  if [[ -f "$PATCHES/$patch_subdir/lib-rs-0.2.patch" ]] \
    && [[ "$crate_name" == "getrandom-$GETRANDOM_02_VERSION" ]]; then
    patch -d "$out" -p1 --forward <"$PATCHES/$patch_subdir/lib-rs-0.2.patch"
  fi
  if [[ -f "$PATCHES/$patch_subdir/backends-rs-0.4.patch" ]] \
    && [[ "$crate_name" == "getrandom-$GETRANDOM_04_VERSION" ]]; then
    patch -d "$out" -p1 --forward <"$PATCHES/$patch_subdir/backends-rs-0.4.patch"
  fi
  if [[ -f "$PATCHES/$patch_subdir/sys-rs.patch" ]]; then
    patch -d "$out" -p1 --forward <"$PATCHES/$patch_subdir/sys-rs.patch"
  fi
}

GETRANDOM_02_SRC="$(find_registry_crate "getrandom-$GETRANDOM_02_VERSION")"
GETRANDOM_04_SRC="$(find_registry_crate "getrandom-$GETRANDOM_04_VERSION")"

apply_patches "$ERRNO_SRC" "errno-$ERRNO_VERSION" "errno"
apply_patches "$LIBC_SRC" "libc-$LIBC_VERSION" "libc"
apply_patches "$RUSTIX_SRC" "rustix-$RUSTIX_VERSION" "rustix"
apply_patches "$GETRANDOM_02_SRC" "getrandom-$GETRANDOM_02_VERSION" "getrandom"
# getrandom 0.4.x is a separate semver line; patch the registry copy in-place
# (Cargo [patch.crates-io] can only redirect one getrandom source).
echo "==> patching getrandom-$GETRANDOM_04_VERSION (registry in-place)"
cp "$PATCHES/getrandom/myos-0.4.rs" "$GETRANDOM_04_SRC/src/backends/myos.rs"
if ! grep -q 'target_os = "myos"' "$GETRANDOM_04_SRC/src/backends.rs"; then
  patch -d "$GETRANDOM_04_SRC" -p1 --forward <"$PATCHES/getrandom/backends-rs-0.4.patch"
fi

# myos uses a fixed-arity fcntl/ioctl libc shim; adjust rustix call sites.
RUSTIX_OUT="$DEST/rustix-$RUSTIX_VERSION"
for f in \
  "$RUSTIX_OUT/src/backend/libc/io/syscalls.rs" \
  "$RUSTIX_OUT/src/backend/libc/fs/syscalls.rs" \
  "$RUSTIX_OUT/src/backend/libc/process/syscalls.rs"; do
  [[ -f "$f" ]] || continue
  sed -i \
    -e 's/c::fcntl(borrowed_fd(fd), c::F_GETFD))/c::fcntl(borrowed_fd(fd), c::F_GETFD, 0))/g' \
    -e 's/c::F_SETFL, flags.bits())/c::F_SETFL, flags.bits() as c::c_ulong)/g' \
    -e 's/c::F_SETFD, flags.bits())/c::F_SETFD, flags.bits() as c::c_ulong)/g' \
    -e 's/c::F_GETLK, \&mut curr_lock)/c::F_GETLK, (\&mut curr_lock as *mut c::flock as c::c_ulong))/g' \
    -e 's/(\&mut curr_lock as \*mut c::flock).cast()/(\&mut curr_lock as *mut c::flock as c::c_ulong)/g' \
    -e 's/c::fcntl(borrowed_fd(fd), cmd, \&lock)/c::fcntl(borrowed_fd(fd), cmd, (\&lock as *const c::flock as c::c_ulong))/g' \
    -e 's/(\&lock as \*const c::flock).cast()/(\&lock as *const c::flock as c::c_ulong)/g' \
    -e 's/c::F_DUPFD_CLOEXEC, min)/c::F_DUPFD_CLOEXEC, min as c::c_ulong)/g' \
    "$f"
done

patch_registry_hostile_crates() {
  local hostname_src console_src
  hostname_src="$(find_registry_crate "hostname-$HOSTNAME_VERSION")"
  console_src="$(find_registry_crate "console-$CONSOLE_VERSION")"

  echo "==> patching hostname-$HOSTNAME_VERSION (registry in-place)"
  cp "$PATCHES/hostname/myos.rs" "$hostname_src/src/myos.rs"
  if ! grep -q 'target_os = "myos"' "$hostname_src/src/lib.rs"; then
    sed -i '/use crate::nix as sys;/a\    } else if #[cfg(target_os = "myos")] {\n        mod myos;\n        use crate::myos as sys;' \
      "$hostname_src/src/lib.rs"
  fi

  echo "==> patching console-$CONSOLE_VERSION (registry in-place)"
  cp "$PATCHES/console/myos_term.rs" "$console_src/src/myos_term.rs"
  if ! grep -q 'mod myos_term' "$console_src/src/lib.rs"; then
    sed -i '/^mod wasm_term;$/a\
#[cfg(target_os = "myos")]\
mod myos_term;' "$console_src/src/lib.rs"
  fi
  if ! grep -q 'pub(crate) use crate::myos_term' "$console_src/src/term.rs"; then
    sed -i '/pub(crate) use crate::unix_term::\*;/a\
#[cfg(target_os = "myos")]\
pub(crate) use crate::myos_term::*;' \
      "$console_src/src/term.rs"
  fi
  if ! grep -q 'target_os = "myos"' "$console_src/src/term.rs" || ! grep -A3 'pub fn family' "$console_src/src/term.rs" | grep -q myos; then
    sed -i '/#\[cfg(all(unix, not(target_arch = "wasm32")))\]/,/TermFamily::UnixTerm/{ /TermFamily::UnixTerm/a\
        #[cfg(target_os = "myos")]\
        {\
            return TermFamily::Dummy;\
        }
}' "$console_src/src/term.rs"
  fi
}

patch_version_hash >"$STAMP"
patch_registry_hostile_crates
echo "Patched crates ready under $DEST"
