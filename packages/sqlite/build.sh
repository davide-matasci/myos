#!/usr/bin/env bash
# Cross-build SQLite's amalgamation (one file, sqlite3.c) as a static
# libsqlite3.a against mlibc for the three arches: target/sqlite-mlibc-<arch>
# (lib/libsqlite3.a, include/sqlite3.h, sqlite3ext.h), what python's sqlite3
# module links. Threads on (mlibc's pthreads), no loadable extensions (a
# static program has no dlopen); the unix VFS takes its locks with fcntl,
# which myos has.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=versions.env
source "$HERE/versions.env"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"

if myos_sqlite_is_current; then
  echo "sqlite libs up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
myos_ensure_llvm_bin

SRC="$ROOT/target/sqlite-src"

build_arch() {
  local arch="$1"
  local out="$ROOT/target/sqlite-mlibc-${arch}"
  echo "==> sqlite ($arch, mlibc)"
  rm -rf "$out"
  mkdir -p "$out/lib" "$out/include"
  # shellcheck disable=SC2046
  clang $(myos_mlibc_cflags "$arch") -O2 -D_GNU_SOURCE \
    -DSQLITE_THREADSAFE=1 -DSQLITE_OMIT_LOAD_EXTENSION -DHAVE_USLEEP=1 \
    -DHAVE_LOCALTIME_R=1 -DHAVE_GMTIME_R=1 -DSQLITE_ENABLE_FTS5 \
    -c "$SRC/sqlite3.c" -o "$out/sqlite3.o"
  "$(command -v llvm-ar 2>/dev/null || echo ar)" rcs "$out/lib/libsqlite3.a" "$out/sqlite3.o"
  rm -f "$out/sqlite3.o"
  cp "$SRC/sqlite3.h" "$SRC/sqlite3ext.h" "$out/include/"
  echo "sqlite -> $out/lib/libsqlite3.a"
}

for arch in ${MYOS_SQLITE_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

echo "$(myos_sqlite_version_hash)" >"$MYOS_SQLITE_VERSION"
echo "sqlite build ok ($SQLITE_VERSION)"
