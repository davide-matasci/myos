#!/usr/bin/env bash
# The port descriptors: every directory with a `port.env` under ports/,
# user/ and toolchain/ (in the image) or packages/ (built and published,
# not in the image) is a port. This library reads them for the build, the
# registry and CI, so a port moves between the two roles by moving its
# directory. See docs/ports.md for the keys.
#
#   source scripts/ports.sh; myos_port_load vim; echo "$PORT_FILES"
#   scripts/ports.sh --list [image|package|all]   names, one per line
#   scripts/ports.sh --outputs NAME               cached outputs (target/...)
#   scripts/ports.sh --image-files NAME [ARCH]    files the image (or the package) needs (target/...)
#   scripts/ports.sh --all-files [image|all]      the same for every port of the role
#   scripts/ports.sh --all-outputs [image|all]    the outputs of every port of the role
#   scripts/ports.sh --stamps                     version stamps of the image ports
#   scripts/ports.sh --tests [image|all]          the boot test scripts of the ports (PORT_TEST), repo-relative
#   scripts/ports.sh --build-list [image|all]     `<name> <script>` of the ports to build, in order
#   scripts/ports.sh --matrix                     ci-ports.yml ports matrix (JSON)

MYOS_PORTS_ROOT="${MYOS_PORTS_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
MYOS_ARCHES=(x86_64 aarch64 riscv64)

# `<role> <name> <dir>` per port, image ports first. The name is the
# directory's unless the descriptor sets PORT_NAME.
myos_port_dirs() {
  local role base d name
  for role in image package; do
    local bases="ports user toolchain"
    [[ "$role" == package ]] && bases="packages"
    for base in $bases; do
      for d in "$MYOS_PORTS_ROOT/$base"/*/; do
        [[ -f "$d/port.env" ]] || continue
        d="${d%/}"
        name="$(sed -n 's/^PORT_NAME=//p' "$d/port.env" | head -1)"
        echo "$role ${name:-${d##*/}} $d"
      done
    done
  done | sort -u -k2,2
}

myos_port_names() {
  local want="${1:-all}" role name dir
  while read -r role name dir; do
    if [[ "$want" == all || "$want" == "$role" ]]; then
      echo "$name"
    fi
  done < <(myos_port_dirs)
}

# Load NAME's descriptor into PORT_* (PORT_NAME, PORT_DIR, PORT_ROLE added).
myos_port_load() {
  local want="$1" role name dir
  PORT_KIND=port PORT_CORE=0 PORT_DEPS="" PORT_BUILD="" PORT_STAMP="" PORT_OUTPUTS="" PORT_FILES=""
  PORT_READY="" PORT_BIN="" PORT_EMBED="" PORT_IMAGE_BASE=0 PORT_WATCH="" PORT_TEST="" PORT_HOST=""
  while read -r role name dir; do
    if [[ "$name" == "$want" ]]; then
      PORT_NAME="$name" PORT_DIR="$dir" PORT_ROLE="$role"
      # shellcheck disable=SC1091
      source "$dir/port.env"
      return 0
    fi
  done < <(myos_port_dirs)
  echo "error: no port named $want (ports/*/port.env, packages/*/port.env)" >&2
  return 1
}

# Expand {arch} {none} {myos} {kernel} in a path for one arch.
myos_port_expand() {
  local s="$1" arch="$2" kernel
  case "$arch" in
    x86_64) kernel="x86_64-unknown-none" ;;
    aarch64) kernel="aarch64-unknown-none-softfloat" ;;
    riscv64) kernel="riscv64imac-unknown-none-elf" ;;
    *) kernel="$arch-unknown-none" ;;
  esac
  s="${s//\{arch\}/$arch}"
  s="${s//\{none\}/$arch-unknown-none}"
  s="${s//\{myos\}/$arch-unknown-myos}"
  s="${s//\{kernel\}/$kernel}"
  echo "$s"
}

# The build script of the loaded port, repo-relative ("" = nothing to
# build): PORT_BUILD with a `/` is repo-relative, otherwise in the port dir.
myos_port_build_script() {
  [[ -n "$PORT_BUILD" ]] || return 0
  if [[ "$PORT_BUILD" == */* ]]; then
    echo "$PORT_BUILD"
  else
    echo "${PORT_DIR#"$MYOS_PORTS_ROOT"/}/$PORT_BUILD"
  fi
}

# The version stamp of the loaded port (target/...): what its build script
# writes when done, `.myos-<name>-version` unless PORT_STAMP says otherwise.
myos_port_stamp() {
  echo "target/${PORT_STAMP:-.myos-$PORT_NAME-version}"
}

# Everything the registry caches for NAME: the stamp and PORT_OUTPUTS for
# every arch (files or directories under target/).
myos_port_outputs() {
  myos_port_load "$1" || return 1
  [[ -n "$PORT_BUILD" ]] || return 0
  myos_port_stamp
  local o arch path
  for o in $PORT_OUTPUTS; do
    for arch in "${MYOS_ARCHES[@]}"; do
      path="target/$(myos_port_expand "$o" "$arch")"
      if [[ "$path" == *\** ]]; then
        # A glob (sbase-*-{none}): only what exists, never a `-src` tree.
        compgen -G "$MYOS_PORTS_ROOT/$path" | sed "s|^$MYOS_PORTS_ROOT/||" | grep -v -- '-src$\|-build$' || true
      else
        echo "$path"
      fi
    done | sort -u
  done
}

# The target/ files the image (or the package) of NAME is packed from, for
# ARCH ("" = all arches): what a boot job must have. A user program the
# kernel embeds (PORT_EMBED) is served from the kernel when its file is
# absent, so it is not listed.
myos_port_image_files() {
  myos_port_load "$1" || return 1
  [[ -z "$PORT_EMBED" ]] || return 0
  local only="${2:-}" spec kind a b c arch
  for spec in $PORT_FILES; do
    IFS=: read -r kind a b c <<<"$spec"
    for arch in "${MYOS_ARCHES[@]}"; do
      [[ -z "$only" || "$only" == "$arch" ]] || continue
      case "$kind" in
        bin|data|tree) echo "target/$(myos_port_expand "$a" "$arch")" ;;
        manifest)
          # The manifest and every ELF it names (`name:/path/to/elf` lines;
          # the registry pull rewrites the paths to this checkout).
          echo "target/$(myos_port_expand "$a" "$arch")"
          if [[ -f "$MYOS_PORTS_ROOT/target/$(myos_port_expand "$a" "$arch")" ]]; then
            sed -n 's/^[^:]*://p' "$MYOS_PORTS_ROOT/target/$(myos_port_expand "$a" "$arch")" \
              | sed 's|.*/target/|target/|'
          fi
          ;;
        multicall)
          echo "target/$(myos_port_expand "$a" "$arch")"
          echo "target/$(myos_port_expand "$b" "$arch")"
          ;;
        file) ;;
        *) echo "error: $PORT_NAME: unknown PORT_FILES kind '$kind'" >&2; return 1 ;;
      esac
    done
  done | sort -u
}

# The boot test scripts (PORT_TEST, docs/testing.md) of the ports of ROLE,
# repo-relative: the image packs them under /lib/myos-tests/ports/ and the
# kernel-inputs hash of the CI build covers them.
myos_port_tests() {
  local role="${1:-image}" name
  for name in $(myos_port_names "$role"); do
    myos_port_load "$name"
    [[ -n "$PORT_TEST" ]] || continue
    echo "${PORT_DIR#"$MYOS_PORTS_ROOT"/}/$PORT_TEST"
  done
}

# The ports of ROLE (image|package|all) with a build script, in build order:
# the toolchains first (newlib, the sysroot), then every port once the
# ports it names in PORT_DEPS are out.
myos_port_build_order() {
  local role="${1:-image}" name done="" todo="" progress
  for name in $(myos_port_names "$role"); do
    myos_port_load "$name"
    [[ -n "$PORT_BUILD" ]] || continue
    if [[ "$PORT_KIND" == toolchain ]]; then
      echo "$name"
      done="$done $name"
    else
      todo="$todo $name"
    fi
  done
  while [[ -n "$todo" ]]; do
    progress=0
    for name in $todo; do
      myos_port_load "$name"
      local dep ready=1
      for dep in $PORT_DEPS; do
        # A dependency outside ROLE (or a toolchain) is built by other means.
        if [[ " $todo " == *" $dep "* ]]; then ready=0; fi
      done
      [[ $ready -eq 1 ]] || continue
      echo "$name"
      done="$done $name"
      todo="${todo/ $name/}"
      progress=1
    done
    if [[ $progress -eq 0 ]]; then
      echo "error: dependency cycle among ports:$todo" >&2
      return 1
    fi
  done
}

# The ci-ports.yml matrix: every port with a build script (image and package
# ports: packages are built and cached the same way), with its dependencies
# (`deps`: ci-build-port.sh builds a dependency the registry does not have),
# plus the optional Linux layer (linux-compat/, not a port: its files are in
# the image only with `--features linux_compat`), built and cached like one.
# The toolchains have their own jobs (sysroot, newlib) and are not in it:
# `needs_sysroot` says which ports wait for the sysroot artifact.
myos_ports_matrix() {
  local name dep deps needs_sysroot
  printf '['
  for name in $(myos_port_build_order all); do
    myos_port_load "$name"
    [[ "$PORT_KIND" != toolchain ]] || continue
    deps="" needs_sysroot=0
    for dep in $PORT_DEPS; do
      case "$dep" in
        sysroot) needs_sysroot=1 ;;
        newlib) ;;
        *) deps="${deps:+$deps }$dep" ;;
      esac
    done
    printf '{"port":"%s","script":"./%s","needs_sysroot":"%s","deps":"%s"},' \
      "$name" "$(myos_port_build_script)" "$needs_sysroot" "$deps"
  done
  printf '{"port":"linux-compat","script":"./linux-compat/build.sh","needs_sysroot":"0","deps":"zlib"}]\n'
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  case "${1:-}" in
    --list) myos_port_names "${2:-all}" ;;
    --outputs) myos_port_outputs "$2" ;;
    --image-files) myos_port_image_files "$2" "${3:-}" ;;
    --all-files)
      # Every file the image ports ship, or (`all`) the package ports' too:
      # what a boot job packs its initramfs and its packages from.
      for n in $(myos_port_names "${2:-image}"); do
        myos_port_image_files "$n"
      done | sort -u
      ;;
    --all-outputs)
      # The stamps and outputs of the ports (what the registry caches): the
      # pack list carries them so a boot job's `cargo build` (the ISO job)
      # finds every port built.
      for n in $(myos_port_names "${2:-image}"); do
        myos_port_outputs "$n"
      done | sort -u
      ;;
    --stamps)
      for n in $(myos_port_names image); do
        myos_port_load "$n"
        [[ -n "$PORT_BUILD" ]] && myos_port_stamp
      done
      ;;
    --matrix) myos_ports_matrix ;;
    --tests) myos_port_tests "${2:-image}" ;;
    --dir) myos_port_load "$2" && echo "$PORT_DIR" ;;
    --script) myos_port_load "$2" && myos_port_build_script ;;
    --build-list)
      # Every port of the role with a build script, in build order:
      # `<name> <script>` (the registry pull/push and the build job's build
      # loop; `all` includes the packages, which the build job packs too).
      for n in $(myos_port_build_order "${2:-image}"); do
        myos_port_load "$n"
        echo "$n $(myos_port_build_script)"
      done
      ;;
    *) sed -n '2,15p' "$0"; exit 2 ;;
  esac
fi
