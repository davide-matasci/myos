#!/usr/bin/env bash
# Fetch ripgrep, apply minimal myos Cargo.toml patch section + pcre2-sys build hook.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=patches/ripgrep/versions.env
source "$ROOT/patches/ripgrep/versions.env"

"$ROOT/scripts/fetch-ripgrep.sh"
"$ROOT/scripts/prepare-coreutils-patches.sh"

RG="$ROOT/target/ripgrep-src"
mkdir -p "$RG/.cargo"
cp "$ROOT/vendor/ripgrep-port/cargo-config.toml" "$RG/.cargo/config.toml"
# Absolute newlib link paths (cargo cwd is ripgrep-src; relative ../../ can break).
sed -i "s|-L../../target/newlib-|-L$ROOT/target/newlib-|g" "$RG/.cargo/config.toml"


# Size profile for myos embedding (PT_LOAD must fit MAX_INIT_PAGES).
if ! grep -q '\[profile.release-myos\]' "$RG/Cargo.toml"; then
  cat >>"$RG/Cargo.toml" <<'PROF'

# --- myos: applied by scripts/prepare-ripgrep-myos.sh (not upstream) ---
[profile.release-myos]
inherits = "release"
opt-level = "z"
lto = "fat"
codegen-units = 1
strip = "symbols"
debug = false
panic = "abort"
incremental = false
# --- end myos profile ---
PROF
fi

# Ensure [patch.crates-io] for myos libc (pcre2-sys needs libc::c_int on myos).
if ! grep -q '\[patch.crates-io\]' "$RG/Cargo.toml"; then
  cat >>"$RG/Cargo.toml" <<'PATCH'

# --- myos: applied by scripts/prepare-ripgrep-myos.sh (not upstream) ---
[patch.crates-io]
errno = { path = "../../target/patched-crates/errno-0.3.14" }
libc = { path = "../../target/patched-crates/libc-0.2.189" }
rustix = { path = "../../target/patched-crates/rustix-1.1.4" }
getrandom = { path = "../../target/patched-crates/getrandom-0.2.17" }
# --- end myos ---
PATCH
fi

# Force lockfile onto patched libc 0.2.189 when pcre2 pulls a different 0.2.x.
(
  cd "$RG"
  unset RUSTC
  cargo "+${MYOS_NIGHTLY:-nightly-2026-07-26}" update -p libc >/dev/null 2>&1 || true
)

# Patch pcre2-sys build.rs once cargo has fetched it (prefer prebuilt PCRE2).
find_pcre2_sys() {
  local ver="$PCRE2_SYS_VERSION"
  local -a roots=(
    "${CARGO_HOME:-$HOME/.cargo}/registry/src"
    "/usr/local/cargo/registry/src"
  )
  local root dir
  for root in "${roots[@]}"; do
    for dir in "$root"/index.crates.io-*; do
      [[ -d "$dir/pcre2-sys-$ver" ]] || continue
      printf '%s\n' "$dir/pcre2-sys-$ver"
      return 0
    done
  done
  return 1
}

# Ensure the crate is downloaded.
(
  cd "$RG"
  unset RUSTC
  cargo "+${MYOS_NIGHTLY:-nightly-2026-07-26}" fetch --locked 2>/dev/null \
    || cargo "+${MYOS_NIGHTLY:-nightly-2026-07-26}" fetch
)

PCRE2_SYS="$(find_pcre2_sys)" || {
  # Trigger download via metadata with feature
  ( cd "$RG" && cargo "+${MYOS_NIGHTLY:-nightly-2026-07-26}" metadata --features pcre2 -q >/dev/null )
  PCRE2_SYS="$(find_pcre2_sys)"
}
if [[ -z "${PCRE2_SYS:-}" || ! -d "$PCRE2_SYS" ]]; then
  echo "error: pcre2-sys-$PCRE2_SYS_VERSION not in cargo registry" >&2
  exit 1
fi
if ! grep -q 'PCRE2_LIB_DIR' "$PCRE2_SYS/build.rs"; then
  cp "$ROOT/patches/ripgrep/pcre2-sys-build.rs" "$PCRE2_SYS/build.rs"
  echo "patched pcre2-sys-$PCRE2_SYS_VERSION build.rs (prebuilt PCRE2 + no JIT on myos)"
else
  echo "pcre2-sys build.rs already myos-aware"
fi


# ignore: non-unix from_path returns "unsupported platform" so WalkBuilder never
# yields explicit file paths on myos. Rewrite stub to use fs::metadata (SYS_STAT).
IGNORE_WALK="$(find "$RG" -path '*/crates/ignore/src/walk.rs' | head -1)"
if [[ -n "$IGNORE_WALK" ]]; then
  python3 "$ROOT/scripts/patch-ignore-myos.py" "$IGNORE_WALK"
fi

echo "ripgrep myos tree ready at $RG"
