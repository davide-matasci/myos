#!/usr/bin/env bash
# Cross-build WebKitGTK (the webkit2gtk-4.1 API: GTK 3 and libsoup 3) and
# the libraries it needs beyond the other packages' against mlibc for the
# three arches (issue #616), static, into one prefix per arch,
# target/webkitgtk-mlibc-<arch> (lib/ with the archives and their
# pkg-config files, include/): ICU (its data built in), libgpg-error,
# libgcrypt, libtasn1, HarfBuzz's ICU library (packages/cairo-pango's
# HarfBuzz, built again with it), then WebKit, with JavaScriptCore's JIT.
# Over the mlibc builds of GLib, the image, text and X libraries, GTK 3,
# libsoup, LibreSSL and SQLite. Then the package's program, one binary
# that is the browser and WebKit's web and network processes (smoke.c).
#
# WebKit draws with Skia on the CPU and never loads GL: there is no EGL
# (webkitgtk-*.myos.patch).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/webkitgtk/port.env
source "$HERE/port.env" # PORT_LIBC: the libc myos_write_cross_cc links
# shellcheck source=packages/webkitgtk/versions.env
source "$HERE/versions.env"
# shellcheck source=packages/cairo-pango/versions.env
source "$ROOT/packages/cairo-pango/versions.env"
# shellcheck source=ports/libressl/versions.env
source "$ROOT/ports/libressl/versions.env"

if myos_webkitgtk_is_current; then
  echo "webkitgtk up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/mlibc/build.sh"
"$ROOT/toolchain/libcxx/build.sh"
"$ROOT/packages/glib/build.sh"
"$ROOT/packages/gtk3/build.sh"
"$ROOT/packages/libsoup/build.sh"
myos_ensure_llvm_bin
# The binutils: newlib's tool wrappers, <triple>-ar and -strip, over
# LLVM's (as packages/glib).
export PATH="$ROOT/target/newlib-bin:$PATH"
# GLib's generators for the build host (packages/glib's host build).
export PATH="$ROOT/target/glib-host/bin:$PATH"
for tool in meson ninja cmake pkg-config make perl ruby gperf unifdef python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "error: $tool not found (README.md)" >&2
    exit 1
  fi
done

T="$ROOT/target"
GUESS="$(sh "$T/libgpg-error-src/build-aux/config.guess")"

# ICU's tools, built for the build host: a cross build of ICU runs them
# (--with-cross-build) to package its data.
build_icu_host() {
  local host="$T/icu-host"
  [[ -f "$host/.done-$ICU_VERSION" ]] && return
  echo "==> ICU (build host)"
  rm -rf "$host"
  mkdir -p "$host"
  (cd "$host" && CC=clang CXX=clang++ "$T/icu-src/source/configure" --disable-tests \
    --disable-samples --disable-extras >configure.log 2>&1) \
    || { tail -30 "$host/configure.log" >&2; exit 1; }
  make -C "$host" -j"$(nproc)" >"$host/make.log" 2>&1 \
    || { grep -n "error" "$host/make.log" | head -20 >&2; exit 1; }
  touch "$host/.done-$ICU_VERSION"
}

# autotools DIR SRC HOST [CONFIGURE-OPTION...]: SRC configured in DIR for
# the arch's myos (configure's HOST) and installed into $out, static.
autotools() {
  local dir="$1" src="$2" host="$3"
  shift 3
  mkdir -p "$dir"
  (cd "$dir" && CC="$build/cc" CXX="$build/c++" AR="${arch}-unknown-myos-ar" \
    RANLIB="${arch}-unknown-myos-ranlib" PKG_CONFIG_LIBDIR="$pc" "$src/configure" \
    --host="$host" --build="$GUESS" --prefix="$out" --enable-static --disable-shared "$@" \
    >configure.log 2>&1) \
    || { tail -30 "$dir/configure.log" >&2; exit 1; }
  make -C "$dir" -j"$(nproc)" install >"$dir/make.log" 2>&1 \
    || { grep -n "error" "$dir/make.log" | head -20 >&2 || true; tail -20 "$dir/make.log" >&2; exit 1; }
  echo "${dir##*/} -> $out"
}

build_libs() {
  # ICU's common and i18n libraries. Its data (every locale, 33 MB) is a
  # file of its own, icudt<version>l.dat, that ICU opens from
  # /lib/webkitgtk (ICU_DATA_DIR), where the package puts it
  # (share/icu-data): built in, it would take the program past what a
  # tmpfs file holds (128 MiB), where the boot tests install apps. Built
  # as for Linux (its make fragment and its code's platform): mlibc is
  # Linux-shaped, and ICU knows no other platform myos would be.
  CPPFLAGS='-D__linux__ -DICU_DATA_DIR=\"/lib/webkitgtk\"' autotools "$build/icu" \
    "$T/icu-src/source" "${arch}-unknown-linux-gnu" --with-cross-build="$T/icu-host" \
    --with-data-packaging=archive --disable-tools --disable-tests --disable-samples \
    --disable-extras --disable-icuio --disable-layoutex
  mkdir -p "$out/share/icu-data"
  cp "$out/share/icu/$ICU_VERSION"/icudt*l.dat "$out/share/icu-data/"

  # libgpg-error's lock type for the target: configure makes it only for
  # Linux cross builds, so its own generator writes it, from what the
  # compiler lays out, into a copy of the source, where its header
  # generator looks (syscfg/, the host's OS: "none").
  # It reads the mutex's size with llvm-objdump (every arch's objects),
  # from clang's own directory when it is not on PATH (Ubuntu's versioned
  # LLVM, CI's image), and says nothing when it cannot: an empty size is
  # refused here.
  local objdump lock="$build/libgpg-error-src/src/syscfg/lock-obj-pub.none.h"
  objdump="$(command -v llvm-objdump || echo "$(dirname "$(readlink -f "$(command -v clang)")")/llvm-objdump")"
  cp -r "$T/libgpg-error-src" "$build/libgpg-error-src"
  LOCK_ABI_VERSION=1 host="${arch}-unknown-none" host_alias="${arch}-unknown-none" \
    CC="$build/cc" OBJDUMP="$objdump" ac_ext=c ac_objext=o AWK=awk \
    sh "$build/libgpg-error-src/src/gen-lock-obj.sh" >"$lock"
  grep -q '_priv\[[1-9]' "$lock" || { echo "error: libgpg-error's lock type ($objdump)" >&2; exit 1; }
  # mlibc has getdents64 but no struct dirent64: the generic way to find
  # the open fds a spawn closes.
  autotools "$build/libgpg-error" "$build/libgpg-error-src" "${arch}-unknown-none" --disable-doc \
    --disable-tests --disable-languages --disable-nls ac_cv_func_getdents64=no
  # The digests, ciphers and random numbers WebKit hashes and checks with;
  # C only (no assembly: its x86 and ARM files assume ELF targets'
  # assemblers in their own ways), over the generic code.
  autotools "$build/libgcrypt" "$T/libgcrypt-src" "${arch}-unknown-none" --with-libgpg-error-prefix="$out" \
    --disable-asm --disable-doc --disable-jent-support --disable-padlock-support
  autotools "$build/libtasn1" "$T/libtasn1-src" "${arch}-unknown-none" --disable-doc --disable-valgrind-tests

  # HarfBuzz's ICU library alone: the rest of HarfBuzz is cairo-pango's.
  myos_mlibc_meson_build "$build" "$T/harfbuzz-src" "$build/harfbuzz-out" -Dicu=enabled \
    -Dglib=disabled -Dgobject=disabled -Dfreetype=disabled -Dcairo=disabled \
    -Dgraphite2=disabled -Dchafa=disabled -Dpng=disabled -Dzlib=disabled -Draster=disabled \
    -Dvector=disabled -Dgpu=disabled -Dsubset=disabled -Dtests=disabled \
    -Dintrospection=disabled -Ddocs=disabled -Dutilities=disabled -Dbenchmark=disabled \
    || exit 1
  cp "$build/harfbuzz-out/lib/libharfbuzz-icu.a" "$out/lib/"
  cp "$build/harfbuzz-out/lib/pkgconfig/harfbuzz-icu.pc" "$out/lib/pkgconfig/"
  cp "$build/harfbuzz-out/include/harfbuzz/hb-icu.h" "$out/include/"
}

# pc DIR NAME VERSION PREFIX LIBS: a pkg-config file for a library built
# without one (LibreSSL's and SQLite's mlibc builds; as packages/libsoup).
pc() {
  cat >"$1/$2.pc" <<EOF
Name: $2
Description: $2 ($PORT_LIBC build)
Version: $3
Cflags: -I$4/include
Libs: -L$4/lib $5
EOF
}

# WebKit with CMake over the arch's prefixes, and the program
# (WEBKIT_MYOS_PROGRAM, webkitgtk-*.myos.patch: smoke.c, with WebKit's
# processes in it). Its sources are compiled as Linux's (WebKit's and
# CMake's platform; __linux__ and __linux, not __unix__, which would have
# libc++ use message catalogs mlibc lacks), against mlibc: the patch takes
# the Linux system calls and EGL away, compat/ has the Linux headers mlibc
# lacks. Its allocator is mlibc's malloc: WebKit's own (bmalloc, libpas,
# and mimalloc, riscv64's default) are made of Linux system calls.
# The build tree is kept until the build succeeds, so a failed run goes
# on where it stopped.
build_webkit() {
  local cmake="$T/webkitgtk-cmake-$arch" prefixes="" m
  local ssl="$T/libressl-mlibc-$arch" sqlite="$T/sqlite-mlibc-$arch"
  for m in "$out" "$T/zlib-mlibc-$arch" "$T/glib-mlibc-$arch" "$T/image-libs-mlibc-$arch" \
    "$T/cairo-pango-mlibc-$arch" "$T/x11-xft-mlibc-$arch" "$T/x11-xft-mlibc-$arch/lib/x11" \
    "$T/gtk3-mlibc-$arch" "$T/libsoup-mlibc-$arch" "$ssl" "$sqlite"; do
    prefixes+="${prefixes:+ }$m"
  done
  mkdir -p "$cmake" "$build/pc"
  pc "$build/pc" libcrypto "$LIBRESSL_VERSION" "$ssl" -lcrypto
  pc "$build/pc" libssl "$LIBRESSL_VERSION" "$ssl" "-lssl -lcrypto"
  pc "$build/pc" openssl "$LIBRESSL_VERSION" "$ssl" "-lssl -lcrypto"
  pc "$build/pc" sqlite3 3 "$sqlite" -lsqlite3
  # The links' response files (@file), spelled out for the cross driver,
  # which takes flags only on its command line; and no dynamic symbol
  # table (--export-dynamic, megabytes a static program never looks in).
  cat >"$build/link" <<'EOF'
#!/usr/bin/env bash
args=()
for a in "$@"; do
  if [[ "$a" == @* ]]; then
    read -d "" -ra words <"${a#@}" || true
    args+=("${words[@]}")
  else
    args+=("$a")
  fi
done
for i in "${!args[@]}"; do
  [[ "${args[$i]}" == -Wl,--export-dynamic ]] && unset 'args[i]'
done
exec "${args[@]}"
EOF
  chmod +x "$build/link"
  cat >"$build/toolchain.cmake" <<EOF
set(CMAKE_SYSTEM_NAME Linux)
set(CMAKE_SYSTEM_PROCESSOR $arch)
set(CMAKE_C_COMPILER $build/cc)
set(CMAKE_CXX_COMPILER $build/c++)
set(CMAKE_AR $(command -v "${arch}-unknown-myos-ar"))
set(CMAKE_RANLIB $(command -v "${arch}-unknown-myos-ranlib"))
set(CMAKE_FIND_ROOT_PATH $prefixes)
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
set(CMAKE_FIND_LIBRARY_SUFFIXES .a)
set(CMAKE_C_LINKER_LAUNCHER $build/link)
set(CMAKE_CXX_LINKER_LAUNCHER $build/link)
EOF
  # Built for size (-Os): the program is one binary, and JavaScript's speed
  # is the JIT's. Not built: the media stack (GStreamer), WebGL and the GPU process, the
  # sandbox, WebDriver, spell checking, speech, gamepads, introspection,
  # the optional image and font formats, and every library myos lacks.
  # The X stage's headers (X11, fontconfig) where Linux has them, on every
  # compile: some of WebKit's targets count on finding them there.
  # What the program links besides WebKit's own: the static libraries'
  # dependencies (CMake asks pkg-config for shared libraries' flags: GTK's
  # X libraries, libsoup's, libwebp's) and GIO's TLS backend, which
  # MultiCall.cpp registers.
  local libs
  libs="$(PKG_CONFIG_LIBDIR="$pc:$build/pc:$build/x11-pc:$T/libsoup-mlibc-$arch/lib/gio/modules/pkgconfig" \
    pkg-config --static --libs gtk+-3.0 libsoup-3.0 gioopenssl libwebp libwebpdemux)"
  local flags="-D__linux__ -D__linux -DEGL_NO_PLATFORM_SPECIFIC_TYPES -I$HERE/compat"
  flags+=" -isystem $T/x11-xft-mlibc-$arch/lib/x11/include"
  # JavaScriptCore on riscv64 is its C interpreter (CLoop), no JIT and no
  # WebAssembly: this WebKit builds neither there (the in-place WebAssembly
  # interpreter has no RISC-V half, the JIT's headers break without it),
  # and the JIT's code would need every hart's instruction cache flushed,
  # where myos's __clear_cache flushes the caller's (issue #652). WTF turns WebAssembly
  # on for RISC-V whatever CMake says but with the patch.
  # Nowhere the sampling profiler: it reads a signal handler's ucontext,
  # which myos does not give (the patch: no HAVE_MACHINE_CONTEXT; issue #651).
  local jsc=(-DENABLE_SAMPLING_PROFILER=OFF)
  [[ "$arch" != riscv64 ]] || jsc+=(-DENABLE_C_LOOP=ON -DENABLE_JIT=OFF -DENABLE_FTL_JIT=OFF
    -DENABLE_DFG_JIT=OFF -DENABLE_WEBASSEMBLY=OFF)
  PKG_CONFIG_LIBDIR="$pc:$build/pc:$build/x11-pc" cmake -G Ninja -S "$T/webkitgtk-src" -B "$cmake" \
    -DCMAKE_TOOLCHAIN_FILE="$build/toolchain.cmake" -DCMAKE_BUILD_TYPE=MinSizeRel \
    -DCMAKE_C_FLAGS="$flags" -DCMAKE_CXX_FLAGS="$flags" -DCMAKE_INSTALL_PREFIX="$out" \
    -DPORT=GTK -DWEBKIT_MYOS=ON -DWEBKIT_MYOS_PROGRAM="$HERE/smoke.c" \
    -DWEBKIT_MYOS_PROGRAM_NAME=webkitgtk-smoke -DWEBKIT_MYOS_PROGRAM_LIBRARIES="${libs// /;}" \
    -DUSE_GTK4=OFF -DENABLE_X11_TARGET=ON "${jsc[@]}" \
    -DENABLE_WAYLAND_TARGET=OFF -DENABLE_QUARTZ_TARGET=OFF -DENABLE_INTROSPECTION=OFF \
    -DENABLE_DOCUMENTATION=OFF -DENABLE_JOURNALD_LOG=OFF -DENABLE_MINIBROWSER=OFF \
    -DENABLE_WEBDRIVER=OFF -DENABLE_GAMEPAD=OFF -DENABLE_SPELLCHECK=OFF \
    -DENABLE_SPEECH_SYNTHESIS=OFF -DUSE_FLITE=OFF -DENABLE_BUBBLEWRAP_SANDBOX=OFF \
    -DENABLE_MEMORY_SAMPLER=OFF -DENABLE_RESOURCE_USAGE=OFF -DENABLE_PDFJS=OFF \
    -DENABLE_VIDEO=OFF -DENABLE_WEB_AUDIO=OFF -DENABLE_WEB_CODECS=OFF -DENABLE_MEDIA_STREAM=OFF \
    -DENABLE_MEDIA_RECORDER=OFF -DENABLE_ENCRYPTED_MEDIA=OFF -DENABLE_WEB_RTC=OFF \
    -DUSE_GSTREAMER=OFF -DENABLE_WEBGL=OFF -DENABLE_GPU_PROCESS=OFF -DENABLE_WEBXR=OFF \
    -DENABLE_XSLT=OFF -DUSE_GBM=OFF -DUSE_LIBDRM=OFF -DUSE_VULKAN=OFF -DUSE_LIBSECRET=OFF \
    -DUSE_LIBHYPHEN=OFF -DUSE_LIBBACKTRACE=OFF -DUSE_SYSPROF_CAPTURE=OFF \
    -DUSE_SYSTEM_SYSPROF_CAPTURE=OFF -DUSE_AVIF=OFF -DUSE_JPEGXL=OFF -DUSE_WOFF2=OFF \
    -DUSE_LCMS=OFF -DUSE_LIBRICE=OFF -DUSE_SYSTEM_UNIFDEF=ON -DUSE_SYSTEM_MALLOC=ON -DUSE_MIMALLOC=OFF \
    >"$cmake/cmake.log" 2>&1 || { tail -40 "$cmake/cmake.log" >&2; exit 1; }
  # The network process's directory listing includes a header WebKitGTK
  # generates, which its objects do not wait for: made first.
  ninja -C "$cmake" WebKitGTK/DerivedSources/WebKitDirectoryInputStreamData.h >"$cmake/ninja.log" 2>&1 \
    || { tail -20 "$cmake/ninja.log" >&2; exit 1; }
  ninja -C "$cmake" -d keeprsp -j"$(nproc)" MyosProgram >>"$cmake/ninja.log" 2>&1 \
    || { grep -n "error" "$cmake/ninja.log" | head -30 >&2 || true; tail -20 "$cmake/ninja.log" >&2; exit 1; }
  install_webkit
  "${arch}-unknown-myos-strip" -o "$T/webkitgtk-smoke-mlibc-${arch}-unknown-none" \
    "$cmake/bin/webkitgtk-smoke"
  rm -rf "$cmake"
}

# What surf links (in $out): the headers and pkg-config files WebKit's
# install puts (its install's other steps want the shell and the
# processes as programs of their own, which myos does not build), and one
# archive, libwebkit2gtk-4.1.a, of everything the program links of
# WebKit's: its objects and its static libraries' (JavaScriptCore, WTF,
# WebCore, Skia, ...) and MultiCall.cpp's main, so a program linking it
# renames its own (webkit_myos_program_main). webkit2gtk-4.1.pc names the
# rest, the libraries' own static dependencies.
install_webkit() {
  local f objs=() o a
  {
    echo "set(CMAKE_INSTALL_PREFIX \"$out\")"
    find "$cmake" -name cmake_install.cmake -print0 | xargs -0 awk '
      /file\(INSTALL .*TYPE FILE / { p = 1 }
      p { print }
      p && /\)[[:space:]]*$/ { p = 0 }'
  } >"$cmake/install-files.cmake"
  cmake -P "$cmake/install-files.cmake" >"$cmake/install.log" 2>&1 \
    || { tail -20 "$cmake/install.log" >&2; exit 1; }
  read -d "" -ra words <"$cmake/CMakeFiles/MyosProgram.rsp" || true
  for f in "${words[@]}"; do
    case "$f" in
      *smoke.c.o) ;;
      *.o) objs+=("$cmake/$f") ;;
      lib/*.a)
        # WebKit's own archives are thin: their members, by path.
        a="$cmake/$f"
        while read -r o; do
          [[ "$o" == /* ]] || o="$(dirname "$a")/$o"
          objs+=("$o")
        done < <("${arch}-unknown-myos-ar" t "$a") ;;
    esac
  done
  rm -f "$out/lib/libwebkit2gtk-4.1.a"
  printf '%s\n' "${objs[@]}" >"$cmake/archive.list"
  "${arch}-unknown-myos-ar" qcs "$out/lib/libwebkit2gtk-4.1.a" @"$cmake/archive.list"
  # The other libraries the program links: the absolute paths and -l of
  # its link (not WebKit's own) and the static dependencies passed in.
  local private=""
  for f in "${words[@]}"; do
    case "$f" in
      /*.a|-l*) private+=" $f" ;;
    esac
  done
  # A program links it as WebKit links its own, with --gc-sections: code
  # it never calls refers to symbols nothing defines (Skia's
  # SkFont::makeStrikeRef), and lld reports those only from the sections
  # it keeps.
  cat >"$out/lib/pkgconfig/webkit2gtk-4.1.pc" <<EOF
prefix=$out
libdir=\${prefix}/lib
includedir=\${prefix}/include

Name: WebKitGTK
Description: Web content engine for GTK (myos: static, with JavaScriptCore and WebKit's processes; the program's main is webkit_myos_program_main)
Version: $WEBKITGTK_VERSION
Requires: glib-2.0 gtk+-3.0 libsoup-3.0
Libs: -Wl,--gc-sections -L\${libdir} -lwebkit2gtk-4.1
Libs.private:$private $libs
Cflags: -I\${includedir}/webkitgtk-4.1
EOF
  # JavaScriptCore is in the same archive.
  sed -i 's/^Libs: .*/Libs: -Wl,--gc-sections -L${libdir} -lwebkit2gtk-4.1/' "$out/lib/pkgconfig/javascriptcoregtk-4.1.pc"
  rm -f "$out/lib/pkgconfig/webkit2gtk-web-extension-4.1.pc"
}

build_arch() {
  local arch="$1"
  local build="$T/webkitgtk-build-$arch"
  local out="$T/webkitgtk-mlibc-${arch}"
  local pc="$out/lib/pkgconfig" m
  for m in zlib glib image-libs cairo-pango x11-xft gtk3 libsoup; do
    pc+=":$T/$m-mlibc-$arch/lib/pkgconfig"
  done
  echo "==> webkitgtk ($arch, mlibc)"
  rm -rf "$build" "$out"
  mkdir -p "$out/lib/pkgconfig" "$out/include"
  myos_mlibc_meson_cross "$arch" "$build" "$out/lib/pkgconfig" "$T/glib-mlibc-$arch/lib/pkgconfig"
  build_libs
  build_webkit
  rm -rf "$build"
  echo "webkitgtk ($arch) -> $out"
}

# The arches MYOS_WEBKITGTK_ARCHES lists, a stamp each: CI builds one per
# job (PORT_ARCH_JOBS).
build_icu_host
hash="$(myos_webkitgtk_version_hash)"
for arch in ${MYOS_WEBKITGTK_ARCHES:-x86_64 aarch64 riscv64}; do
  if MYOS_WEBKITGTK_ARCHES="$arch" myos_webkitgtk_is_current; then
    echo "webkitgtk ($arch) up to date"
    continue
  fi
  build_arch "$arch"
  echo "$hash" >"$T/.myos-webkitgtk-$arch-version"
done
echo "webkitgtk build ok ($WEBKITGTK_VERSION)"
