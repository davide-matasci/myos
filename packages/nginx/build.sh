#!/usr/bin/env bash
# Cross-build nginx (poll events, PCRE2, zlib, SSL over LibreSSL) for the
# three arches, without its configure (ngx_auto_config.h is hand-written).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/nginx/versions.env
source "$HERE/versions.env"

if myos_nginx_is_current; then
  echo "nginx ELFs up to date"
  exit 0
fi

"$HERE/fetch.sh"
"$ROOT/toolchain/newlib/build.sh"
"$ROOT/ports/zlib/build.sh"
"$(myos_port_dir pcre2)/build.sh"
"$ROOT/ports/libressl/build.sh"
export PATH="$ROOT/target/newlib-bin:$PATH"

WORK="$ROOT/target/nginx-myos-build"
rm -rf "$WORK"
cp -a "$ROOT/target/nginx-src" "$WORK"
for p in "$HERE"/*.myos.patch; do
  patch -s -d "$WORK" -p1 <"$p"
done

# The modules, in the order configure lists them (auto/modules): the
# filters' order is their order in the output chain.
MODULES=(
  ngx_core_module ngx_errlog_module ngx_conf_module
  ngx_openssl_module ngx_openssl_cache_module ngx_regex_module
  ngx_events_module ngx_event_core_module ngx_poll_module
  ngx_http_module ngx_http_core_module ngx_http_log_module
  ngx_http_upstream_module ngx_http_static_module ngx_http_autoindex_module
  ngx_http_index_module ngx_http_mirror_module ngx_http_try_files_module
  ngx_http_auth_basic_module ngx_http_access_module
  ngx_http_limit_conn_module ngx_http_limit_req_module ngx_http_geo_module
  ngx_http_map_module ngx_http_split_clients_module ngx_http_referer_module
  ngx_http_rewrite_module ngx_http_ssl_module
  ngx_http_proxy_module ngx_http_fastcgi_module ngx_http_uwsgi_module
  ngx_http_scgi_module ngx_http_memcached_module ngx_http_empty_gif_module
  ngx_http_browser_module ngx_http_upstream_hash_module
  ngx_http_upstream_ip_hash_module ngx_http_upstream_least_conn_module
  ngx_http_upstream_random_module ngx_http_upstream_keepalive_module
  ngx_http_upstream_zone_module ngx_http_upstream_sticky_module
  ngx_http_write_filter_module ngx_http_header_filter_module
  ngx_http_chunked_filter_module ngx_http_range_header_filter_module
  ngx_http_gzip_filter_module ngx_http_postpone_filter_module
  ngx_http_ssi_filter_module ngx_http_charset_filter_module
  ngx_http_userid_filter_module ngx_http_headers_filter_module
  ngx_http_copy_filter_module ngx_http_range_body_filter_module
  ngx_http_not_modified_filter_module
)

# auto/sources: the core, the events (poll, SSL), the generic POSIX unix
# layer, HTTP and its modules.
SRCS=(
  core/nginx.c core/ngx_log.c core/ngx_palloc.c core/ngx_array.c
  core/ngx_list.c core/ngx_hash.c core/ngx_buf.c core/ngx_queue.c
  core/ngx_output_chain.c core/ngx_string.c core/ngx_parse.c
  core/ngx_parse_time.c core/ngx_inet.c core/ngx_file.c core/ngx_crc32.c
  core/ngx_murmurhash.c core/ngx_md5.c core/ngx_sha1.c core/ngx_rbtree.c
  core/ngx_radix_tree.c core/ngx_slab.c core/ngx_times.c core/ngx_shmtx.c
  core/ngx_connection.c core/ngx_cycle.c core/ngx_spinlock.c
  core/ngx_rwlock.c core/ngx_cpuinfo.c core/ngx_conf_file.c
  core/ngx_module.c core/ngx_resolver.c core/ngx_open_file_cache.c
  core/ngx_crypt.c core/ngx_proxy_protocol.c core/ngx_syslog.c
  core/ngx_regex.c
  event/ngx_event.c event/ngx_event_timer.c event/ngx_event_posted.c
  event/ngx_event_accept.c event/ngx_event_udp.c event/ngx_event_connect.c
  event/ngx_event_pipe.c event/modules/ngx_poll_module.c
  event/ngx_event_openssl.c event/ngx_event_openssl_cache.c
  event/ngx_event_openssl_stapling.c
  os/unix/ngx_time.c os/unix/ngx_errno.c os/unix/ngx_alloc.c
  os/unix/ngx_files.c os/unix/ngx_socket.c os/unix/ngx_recv.c
  os/unix/ngx_readv_chain.c os/unix/ngx_udp_recv.c os/unix/ngx_send.c
  os/unix/ngx_writev_chain.c os/unix/ngx_udp_send.c
  os/unix/ngx_udp_sendmsg_chain.c os/unix/ngx_channel.c
  os/unix/ngx_shmem.c os/unix/ngx_process.c os/unix/ngx_daemon.c
  os/unix/ngx_setaffinity.c os/unix/ngx_setproctitle.c
  os/unix/ngx_posix_init.c os/unix/ngx_user.c os/unix/ngx_dlopen.c
  os/unix/ngx_process_cycle.c
  http/ngx_http.c http/ngx_http_core_module.c http/ngx_http_special_response.c
  http/ngx_http_request.c http/ngx_http_parse.c http/ngx_http_upstream.c
  http/ngx_http_upstream_round_robin.c http/ngx_http_file_cache.c
  http/ngx_http_write_filter_module.c http/ngx_http_header_filter_module.c
  http/ngx_http_postpone_filter_module.c http/ngx_http_copy_filter_module.c
  http/ngx_http_variables.c http/ngx_http_script.c http/ngx_http_request_body.c
  http/modules/ngx_http_log_module.c http/modules/ngx_http_static_module.c
  http/modules/ngx_http_autoindex_module.c http/modules/ngx_http_index_module.c
  http/modules/ngx_http_mirror_module.c http/modules/ngx_http_try_files_module.c
  http/modules/ngx_http_auth_basic_module.c http/modules/ngx_http_access_module.c
  http/modules/ngx_http_limit_conn_module.c http/modules/ngx_http_limit_req_module.c
  http/modules/ngx_http_geo_module.c http/modules/ngx_http_map_module.c
  http/modules/ngx_http_split_clients_module.c http/modules/ngx_http_referer_module.c
  http/modules/ngx_http_rewrite_module.c http/modules/ngx_http_ssl_module.c
  http/modules/ngx_http_proxy_module.c
  http/modules/ngx_http_fastcgi_module.c http/modules/ngx_http_uwsgi_module.c
  http/modules/ngx_http_scgi_module.c http/modules/ngx_http_memcached_module.c
  http/modules/ngx_http_empty_gif_module.c http/modules/ngx_http_browser_module.c
  http/modules/ngx_http_upstream_hash_module.c
  http/modules/ngx_http_upstream_ip_hash_module.c
  http/modules/ngx_http_upstream_least_conn_module.c
  http/modules/ngx_http_upstream_random_module.c
  http/modules/ngx_http_upstream_keepalive_module.c
  http/modules/ngx_http_upstream_zone_module.c
  http/modules/ngx_http_upstream_sticky_module.c
  http/modules/ngx_http_chunked_filter_module.c
  http/modules/ngx_http_range_filter_module.c
  http/modules/ngx_http_gzip_filter_module.c
  http/modules/ngx_http_ssi_filter_module.c
  http/modules/ngx_http_charset_filter_module.c
  http/modules/ngx_http_userid_filter_module.c
  http/modules/ngx_http_headers_filter_module.c
  http/modules/ngx_http_not_modified_filter_module.c
)

# objs/: what configure writes, the config headers ours, ngx_modules.c as
# auto/modules makes it.
mkdir -p "$WORK/objs"
cp "$HERE/ngx_auto_config.h" "$HERE/ngx_auto_headers.h" "$WORK/objs/"
{
  printf '#include <ngx_config.h>\n#include <ngx_core.h>\n\n'
  printf 'extern ngx_module_t  %s;\n' "${MODULES[@]}"
  printf '\nngx_module_t *ngx_modules[] = {\n'
  printf '    &%s,\n' "${MODULES[@]}"
  printf '    NULL\n};\n\nchar *ngx_module_names[] = {\n'
  printf '    "%s",\n' "${MODULES[@]}"
  printf '    NULL\n};\n\nchar *ngx_module_order[] = {\n    NULL\n};\n'
} >"$WORK/objs/ngx_modules.c"

build_arch() {
  local arch="$1"
  local triple="${arch}-unknown-myos"
  local cc="${triple}-cc"
  local prefix="$ROOT/target/newlib-${arch}"
  local lib="$prefix/${triple}/lib"
  local obj="$ROOT/target/nginx-obj-${arch}"
  local out="$ROOT/target/nginx-${arch}-unknown-none"
  local cflags=(
    -ffreestanding -fPIC -O2 -g0 -std=gnu99
    -ffunction-sections -fdata-sections
    -Wno-unused-parameter -Wno-deprecated-declarations
    -nostdinc -isystem "$(clang -print-resource-dir)/include"
    -isystem "$prefix/${triple}/include"
    -I"$WORK/src/core" -I"$WORK/src/event" -I"$WORK/src/event/modules"
    -I"$WORK/src/os/unix" -I"$WORK/objs" -I"$WORK/src/http"
    -I"$WORK/src/http/modules"
    -I"$ROOT/target/zlib-${arch}/include"
    -I"$ROOT/target/pcre2-${arch}/include" -DPCRE2_STATIC
    -I"$ROOT/target/libressl-${arch}/include"
    -I"$HERE" -include myos_compat.h
  )
  local f o objs=()

  echo "==> nginx ($triple)"
  rm -rf "$obj"
  mkdir -p "$obj"
  for f in "${SRCS[@]}" ../objs/ngx_modules.c "$HERE/myos_stubs.c"; do
    [[ "$f" == /* ]] || f="$WORK/src/$f"
    o="$obj/$(basename "$f" .c).o"
    if ! "$cc" "${cflags[@]}" -c "$f" -o "$o" 2>"$o.err"; then
      echo "FAIL $f" >&2
      head -30 "$o.err" >&2
      return 1
    fi
    objs+=("$o")
  done

  local extra=()
  if [[ "$arch" == "aarch64" || "$arch" == "riscv64" ]]; then
    "$cc" -ffreestanding -fPIC -O2 -isystem "$prefix/${triple}/include" \
      -c "$ROOT/ports/sbase/trunctfdf2.c" -o "$obj/trunctfdf2.o"
    extra+=("$obj/trunctfdf2.o")
  fi

  "${triple}-ld" -pie --no-dynamic-linker --gc-sections -o "$out" \
    --entry=_start -z max-page-size=4096 \
    "$lib/crt0.o" "${objs[@]}" "${extra[@]+"${extra[@]}"}" \
    -L"$lib" -L"$ROOT/target/zlib-${arch}/lib" -L"$ROOT/target/pcre2-${arch}/lib" \
    -L"$ROOT/target/libressl-${arch}/lib" \
    --start-group -lpcre2-8 -lz -lssl -lcrypto -lc -lm -lgloss -lg --end-group
  "${triple}-strip" -s "$out" 2>/dev/null || true
  ls -lh "$out"
}

for arch in ${MYOS_NGINX_ARCHES:-x86_64 aarch64 riscv64}; do
  build_arch "$arch"
done

# The configuration's files for the app: upstream's types and pages.
mkdir -p "$ROOT/target/nginx-conf"
cp "$WORK/conf/mime.types" "$ROOT/target/nginx-conf/mime.types"
cp "$WORK/html/index.html" "$WORK/html/50x.html" "$ROOT/target/nginx-conf/"

echo "$(myos_nginx_version_hash)" >"$MYOS_NGINX_VERSION"
echo "nginx build ok ($NGINX_VERSION)"
