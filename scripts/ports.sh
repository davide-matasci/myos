#!/bin/sh
# The port descriptors: every directory with a `port.env` under ports/,
# user/ and toolchain/ (in the image) or packages/ (built and published,
# not in the image) is a port. This library reads them for the build, the
# registry and CI, so a port moves between the two roles by moving its
# directory. See docs/ports.md for the keys.
#
#   ROOT=...; . "$ROOT/scripts/ports.sh"; myos_port_load vim; echo "$PORT_FILES"
#   scripts/ports.sh --list [image|package|all]   names, one per line
#   scripts/ports.sh --outputs NAME               cached outputs (target/...)
#   scripts/ports.sh --image-files NAME [ARCH]    files the image (or the package) needs (target/...)
#   scripts/ports.sh --all-files [image|all]      the same for every port of the role
#   scripts/ports.sh --source-files [image|all]   the checked-in files the ports ship (`file:` entries), repo-relative
#   scripts/ports.sh --kernel-files               the files of the ports kernel/build.rs builds
#   scripts/ports.sh --all-outputs [image|all]    the outputs of every port of the role
#   scripts/ports.sh --stamps                     version stamps of the image ports
#   scripts/ports.sh --tests [image|all]          the boot test scripts of the ports (PORT_TEST), repo-relative
#   scripts/ports.sh --build-list [image|all]     `<name> <script>` of the ports to build, in order
#   scripts/ports.sh --deps NAME                  the ports NAME builds against, transitively, in build order
#   scripts/ports.sh --matrix                     ci-ports.yml ports matrix (JSON)

# A sourced file cannot tell where it is: the script that sources this one
# says where the checkout is (MYOS_PORTS_ROOT, MYOS_ROOT or ROOT).
if [ "${0##*/}" = ports.sh ]; then
  MYOS_PORTS_ROOT="${MYOS_PORTS_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
else
  MYOS_PORTS_ROOT="${MYOS_PORTS_ROOT:-${ROOT:-${MYOS_ROOT:?set ROOT to the checkout before sourcing scripts/ports.sh}}}"
fi
MYOS_ARCHES="x86_64 aarch64 riscv64"

# There is no `local` in POSIX sh: a function's own variables are named
# `_<function's initials>_<name>`, so that none overwrites a caller's.

# `<role> <name> <dir>` per port, image ports first. The name is the
# directory's unless the descriptor sets PORT_NAME.
myos_port_scan() {
  # The loop is one side of a pipe, a subshell: its variables stay in it.
  for role in image package; do
    bases="ports user toolchain"
    [ "$role" = package ] && bases="packages"
    for base in $bases; do
      for d in "$MYOS_PORTS_ROOT/$base"/*/; do
        [ -f "$d/port.env" ] || continue
        d="${d%/}"
        name="$(sed -n 's/^PORT_NAME=//p' "$d/port.env" | head -1)"
        echo "$role ${name:-${d##*/}} $d"
      done
    done
  done | sort -u -k2,2
}

# The scan runs once, when this file is sourced: every lookup below
# (myos_port_load, in loops over every port) reads the list from memory.
# Rescanning the descriptors per lookup made a single `--build-list all`
# take seconds, and the CI registry script runs it on every pull.
MYOS_PORT_DIRS="$(myos_port_scan)"

myos_port_names() {
  _pn_want="${1:-all}"
  while read -r _pn_role _pn_name _pn_dir; do
    if [ "$_pn_want" = all ] || [ "$_pn_want" = "$_pn_role" ]; then
      echo "$_pn_name"
    fi
  done <<EOF
$MYOS_PORT_DIRS
EOF
}

# Load NAME's descriptor into PORT_* (PORT_NAME, PORT_DIR, PORT_ROLE added).
myos_port_load() {
  PORT_KIND=port PORT_CORE=0 PORT_DEPS="" PORT_RDEPS="" PORT_BUILD="" PORT_STAMP="" PORT_OUTPUTS="" PORT_FILES="" PORT_TEST_FILES=""
  PORT_READY="" PORT_BIN="" PORT_WATCH="" PORT_PREPARE="" PORT_TEST="" PORT_HOST=""
  PORT_ARCH_JOBS=0 PORT_TIMEOUT=45 PORT_LINK_OUTPUTS="" PORT_LIBRARY=0
  while read -r _pl_role _pl_name _pl_dir; do
    if [ "$_pl_name" = "$1" ]; then
      PORT_NAME="$_pl_name" PORT_DIR="$_pl_dir" PORT_ROLE="$_pl_role"
      # shellcheck disable=SC1091
      . "$_pl_dir/port.env"
      return 0
    fi
  done <<EOF
$MYOS_PORT_DIRS
EOF
  echo "error: no port named $1 (ports/*/port.env, packages/*/port.env)" >&2
  return 1
}

# Expand {arch} {none} {myos} {kernel} in a path for one arch.
myos_port_expand() {
  case "$2" in
    x86_64) _pe_kernel="x86_64-unknown-none" ;;
    aarch64) _pe_kernel="aarch64-unknown-none-softfloat" ;;
    riscv64) _pe_kernel="riscv64imac-unknown-none-elf" ;;
    *) _pe_kernel="$2-unknown-none" ;;
  esac
  myos_port_replace "$1" "{arch}" "$2"
  myos_port_replace "$_pr_out" "{none}" "$2-unknown-none"
  myos_port_replace "$_pr_out" "{myos}" "$2-unknown-myos"
  myos_port_replace "$_pr_out" "{kernel}" "$_pe_kernel"
  echo "$_pr_out"
}

# _pr_out = $1 with every $2 replaced by $3 (plain strings, no patterns),
# without a `sed` per path: the expansions run by the hundred.
myos_port_replace() {
  _pr_out="" _pr_rest="$1"
  while :; do
    case "$_pr_rest" in
      *"$2"*)
        _pr_out="$_pr_out${_pr_rest%%"$2"*}$3"
        _pr_rest="${_pr_rest#*"$2"}"
        ;;
      *) _pr_out="$_pr_out$_pr_rest"; return 0 ;;
    esac
  done
}

# Split a PORT_FILES entry (`kind:a:b`) into _ps_kind, _ps_a and _ps_b.
myos_port_spec() {
  _ps_ifs="$IFS"
  IFS=:
  set -f
  # shellcheck disable=SC2086
  set -- $1
  set +f
  IFS="$_ps_ifs"
  _ps_kind="${1:-}" _ps_a="${2:-}" _ps_b="${3:-}"
}

# The build script of the loaded port, repo-relative ("" = nothing to
# build): PORT_BUILD with a `/` is repo-relative, otherwise in the port dir.
myos_port_build_script() {
  [ -n "$PORT_BUILD" ] || return 0
  case "$PORT_BUILD" in
    */*) echo "$PORT_BUILD" ;;
    *) echo "${PORT_DIR#"$MYOS_PORTS_ROOT"/}/$PORT_BUILD" ;;
  esac
}

# The version stamp of the loaded port (target/...): what its build script
# writes when done, `.myos-<name>-version` unless PORT_STAMP says otherwise;
# one per arch for a port built an arch at a time (PORT_ARCH_JOBS,
# `.myos-<name>-{arch}-version`), for the ARCHes given (default: all).
myos_port_stamp() {
  _st_stamp="${PORT_STAMP:-.myos-$PORT_NAME-version}"
  [ "$PORT_ARCH_JOBS" = 1 ] && [ -z "$PORT_STAMP" ] && _st_stamp=".myos-$PORT_NAME-{arch}-version"
  # shellcheck disable=SC2086
  [ $# -gt 0 ] || set -- $MYOS_ARCHES
  for _st_arch in "$@"; do
    echo "target/$(myos_port_expand "$_st_stamp" "$_st_arch")"
  done | sort -u
}

# The outputs stamp of the loaded port (`<input hash> <output hash>`,
# docs/ports.md "Cache keys"): `.myos-<name>-outputs`, one per arch
# (`.myos-<name>-<arch>-outputs`) for a port built an arch at a time, for
# the ARCHes given (default: all).
myos_port_outputs_stamp() {
  if [ "$PORT_ARCH_JOBS" = 1 ]; then
    # shellcheck disable=SC2086
    [ $# -gt 0 ] || set -- $MYOS_ARCHES
    for _os_arch in "$@"; do
      echo "target/.myos-$PORT_NAME-$_os_arch-outputs"
    done
  else
    echo "target/.myos-$PORT_NAME-outputs"
  fi
}

# The outputs of NAME that the ports over it only link, never compile
# against (PORT_LINK_OUTPUTS, under target/), for every arch or for ARCH
# alone: what its compile flavour leaves out (docs/ports.md "Cache keys").
myos_port_link_outputs() {
  myos_port_load "$1" || return 1
  _lo_arches="${2:-$MYOS_ARCHES}"
  for _lo_o in $PORT_LINK_OUTPUTS; do
    for _lo_arch in $_lo_arches; do
      echo "target/$(myos_port_expand "$_lo_o" "$_lo_arch")"
    done
  done | sort -u
}

# Everything the registry caches for NAME: the stamps and PORT_OUTPUTS for
# every arch, or for ARCH alone (files or directories under target/).
myos_port_outputs() {
  myos_port_load "$1" || return 1
  [ -n "$PORT_BUILD" ] || return 0
  _po_arches="${2:-$MYOS_ARCHES}"
  # shellcheck disable=SC2086
  myos_port_stamp $_po_arches
  # shellcheck disable=SC2086
  myos_port_outputs_stamp $_po_arches
  for _po_o in $PORT_OUTPUTS; do
    for _po_arch in $_po_arches; do
      _po_path="target/$(myos_port_expand "$_po_o" "$_po_arch")"
      case "$_po_path" in
        *\**)
          # A glob (sbase-*-{none}): only what exists, never a `-src` tree.
          for _po_f in "$MYOS_PORTS_ROOT"/$_po_path; do
            [ -e "$_po_f" ] || continue
            echo "${_po_f#"$MYOS_PORTS_ROOT"/}"
          done | grep -v -- '-src$\|-build$' || true
          ;;
        *) echo "$_po_path" ;;
      esac
    done | sort -u
  done
}

# The target/ files the image (or the package) of NAME is packed from, for
# ARCH ("" = all arches): what a boot job must have, the tests module's
# (PORT_TEST_FILES) too.
myos_port_image_files() {
  myos_port_load "$1" || return 1
  _if_only="${2:-}"
  for _if_spec in $PORT_FILES $PORT_TEST_FILES; do
    myos_port_spec "$_if_spec"
    for _if_arch in $MYOS_ARCHES; do
      [ -z "$_if_only" ] || [ "$_if_only" = "$_if_arch" ] || continue
      case "$_ps_kind" in
        bin|data|tree|source) echo "target/$(myos_port_expand "$_ps_a" "$_if_arch")" ;;
        manifest)
          # The manifest and every ELF it names (`name:/path/to/elf` lines;
          # the registry pull rewrites the paths to this checkout).
          echo "target/$(myos_port_expand "$_ps_a" "$_if_arch")"
          if [ -f "$MYOS_PORTS_ROOT/target/$(myos_port_expand "$_ps_a" "$_if_arch")" ]; then
            sed -n 's/^[^:]*://p' "$MYOS_PORTS_ROOT/target/$(myos_port_expand "$_ps_a" "$_if_arch")" \
              | sed 's|.*/target/|target/|'
          fi
          ;;
        multicall)
          echo "target/$(myos_port_expand "$_ps_a" "$_if_arch")"
          echo "target/$(myos_port_expand "$_ps_b" "$_if_arch")"
          ;;
        file) ;;
        *) echo "error: $PORT_NAME: unknown PORT_FILES kind '$_ps_kind'" >&2; return 1 ;;
      esac
    done
  done | sort -u
}

# The boot test scripts (PORT_TEST, docs/testing.md) of the ports of ROLE,
# repo-relative: the image packs them under /lib/myos-tests/ports/ and the
# kernel-inputs hash of the CI build covers them.
myos_port_tests() {
  for _pt_name in $(myos_port_names "${1:-image}"); do
    myos_port_load "$_pt_name"
    [ -n "$PORT_TEST" ] || continue
    echo "${PORT_DIR#"$MYOS_PORTS_ROOT"/}/$PORT_TEST"
  done
}

# The ports of ROLE (image|package|all) with a build script, in build order:
# the toolchains first (newlib, the sysroot), then every port once the
# ports it names in PORT_DEPS are out.
myos_port_build_order() {
  _bo_todo=""
  for _bo_name in $(myos_port_names "${1:-image}"); do
    myos_port_load "$_bo_name"
    [ -n "$PORT_BUILD" ] || continue
    if [ "$PORT_KIND" = toolchain ]; then
      echo "$_bo_name"
    else
      _bo_todo="$_bo_todo $_bo_name"
    fi
  done
  while [ -n "$_bo_todo" ]; do
    _bo_progress=0
    for _bo_name in $_bo_todo; do
      myos_port_load "$_bo_name"
      _bo_ready=1
      for _bo_dep in $PORT_DEPS; do
        # A dependency outside ROLE (or a toolchain) is built by other means.
        case " $_bo_todo " in *" $_bo_dep "*) _bo_ready=0 ;; esac
      done
      [ $_bo_ready -eq 1 ] || continue
      echo "$_bo_name"
      # Out of the list: its first ` name`, as `${todo/ $name/}` took it.
      case "$_bo_todo" in
        *" $_bo_name"*) _bo_todo="${_bo_todo%%" $_bo_name"*}${_bo_todo#*" $_bo_name"}" ;;
      esac
      _bo_progress=1
    done
    if [ $_bo_progress -eq 0 ]; then
      echo "error: dependency cycle among ports:$_bo_todo" >&2
      return 1
    fi
  done
}

# The ports NAME builds against, transitively (its PORT_DEPS and theirs),
# in build order, the toolchains with jobs of their own (newlib, the
# sysroot) left out: what a CI port job pulls or builds, each before the
# next, so that every one is keyed on the registry's outputs of the ones
# before it (scripts/ci-build-port.sh, docs/ports.md "Cache keys").
myos_port_deps_closure() {
  _dc_todo="$1" _dc_closure=""
  while [ -n "$_dc_todo" ]; do
    # The first of the list, and the list without it.
    _dc_name="${_dc_todo%% *}"
    case "$_dc_todo" in
      *" "*) _dc_todo="${_dc_todo#* }" ;;
      *) _dc_todo="" ;;
    esac
    myos_port_load "$_dc_name" || return 1
    for _dc_dep in $PORT_DEPS; do
      case "$_dc_dep" in sysroot|newlib) continue ;; esac
      case " $_dc_closure " in *" $_dc_dep "*) continue ;; esac
      _dc_closure="$_dc_closure $_dc_dep"
      _dc_todo="${_dc_todo:+$_dc_todo }$_dc_dep"
    done
  done
  for _dc_name in $(myos_port_build_order all); do
    case " $_dc_closure " in *" $_dc_name "*) echo "$_dc_name" ;; esac
  done
  return 0
}

# The CI jobs (and registry pieces) of NAME: NAME, or `NAME:<arch>` per
# arch for a port built an arch at a time (PORT_ARCH_JOBS=1: its build
# script builds the arches MYOS_<NAME>_ARCHES lists).
myos_port_jobs() {
  myos_port_load "$1" || return 1
  if [ "$PORT_ARCH_JOBS" = 1 ]; then
    for _pj_arch in $MYOS_ARCHES; do
      echo "$1:$_pj_arch"
    done
  else
    echo "$1"
  fi
}

# The ci-ports.yml matrix: every port with a build script (image and package
# ports: packages are built and cached the same way), with its dependencies
# (`deps`: ci-build-port.sh builds a dependency the registry does not have),
# plus the optional Linux layer (linux-compat/, not a port: its files are in
# the image only with `--features linux_compat`), built and cached like one.
# The toolchains have their own jobs (sysroot, newlib) and are not in it:
# `needs_sysroot` says which ports wait for the sysroot artifact. A port
# built an arch at a time (PORT_ARCH_JOBS) has a job per arch, named
# `<name>:<arch>` (the registry caches each arch apart); `timeout` is the
# job's limit in minutes (PORT_TIMEOUT).
myos_ports_matrix() {
  printf '['
  for _pm_name in $(myos_port_build_order all); do
    myos_port_load "$_pm_name"
    [ "$PORT_KIND" != toolchain ] || continue
    _pm_deps="" _pm_needs_sysroot=0
    for _pm_dep in $PORT_DEPS; do
      case "$_pm_dep" in
        sysroot) _pm_needs_sysroot=1 ;;
        newlib) ;;
        *) _pm_deps="${_pm_deps:+$_pm_deps }$_pm_dep" ;;
      esac
    done
    for _pm_job in $(myos_port_jobs "$_pm_name"); do
      printf '{"port":"%s","script":"./%s","needs_sysroot":"%s","deps":"%s","timeout":%s},' \
        "$_pm_job" "$(myos_port_build_script)" "$_pm_needs_sysroot" "$_pm_deps" "$PORT_TIMEOUT"
    done
  done
  printf '{"port":"linux-compat","script":"./linux-compat/build.sh","needs_sysroot":"0","deps":"","timeout":45}]\n'
}

if [ "${0##*/}" = ports.sh ]; then
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
    --source-files)
      # The checked-in files the ports ship as they are (`file:` in
      # PORT_FILES and PORT_TEST_FILES: a script, a configuration file):
      # packed from the checkout, so no stamp or output covers them.
      for n in $(myos_port_names "${2:-image}"); do
        myos_port_load "$n"
        for spec in $PORT_FILES $PORT_TEST_FILES; do
          myos_port_spec "$spec"
          [ "$_ps_kind" != file ] || echo "${PORT_DIR#"$MYOS_PORTS_ROOT"/}/$_ps_a"
        done
      done | sort -u
      ;;
    --kernel-files)
      # The files of the ports without a build script, the Rust user
      # programs kernel/build.rs builds (kind `user`): no registry port
      # carries them, so the CI build's kernels bundle does
      # (scripts/ci-build-kernels.sh --print-members).
      for n in $(myos_port_names all); do
        myos_port_load "$n"
        [ -z "$PORT_BUILD" ] || continue
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
        [ -z "$PORT_BUILD" ] || myos_port_stamp
      done
      ;;
    --matrix) myos_ports_matrix ;;
    --tests) myos_port_tests "${2:-image}" ;;
    --dir) myos_port_load "$2" && echo "$PORT_DIR" ;;
    --deps) myos_port_deps_closure "$2" ;;
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
