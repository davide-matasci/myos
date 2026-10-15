# nginx for myos

Upstream **nginx 1.30.6** (stable; fetched at build, not vendored),
cross-built for the three arches without its `configure`, which compiles
and runs test programs a cross build cannot run. What `configure` would
write is hand-written instead (the vim, lynx and curl approach):

| File | Role |
|------|------|
| `ngx_auto_config.h`, `ngx_auto_headers.h` | `configure`'s results for newlib + libgloss: the generic POSIX path (no `NGX_LINUX`), the modules, the paths |
| `build.sh` | the sources of `auto/sources` for that path, `objs/ngx_modules.c` as `auto/modules` writes it, the link |
| `myos_compat.h`, `myos_stubs.c` | what newlib lacks that has no honest implementation: rlimits (`getrlimit` reports the fd table, 256), `setpriority`'s declaration, `timezone` |
| `nginx.c.myos.patch` | creates `/tmp/nginx` at startup (myos has no `/var`) |
| `ngx_process.c.myos.patch` | no `FIOASYNC`/`F_SETOWN` on the workers' channels (no `SIGIO` on fds); no `si_pid` in the signal log line (issue #451) |

Modules: the default set (static, index, autoindex, proxy, fastcgi, uwsgi,
scgi, memcached, rewrite, map, geo, limit_*, auth_basic with nginx's own
password schemes, gzip, ssi, ...), events through `poll`, regular
expressions with `packages/pcre2`, gzip with `ports/zlib`.

## On the system

```sh
get-myos nginx
run-myos nginx            # serves /lib/nginx/html on port 80
run-myos nginx -t         # checks /lib/nginx/nginx.conf
run-myos nginx -c FILE    # another configuration
run-myos nginx -s quit    # stops it
```

The app's files are at the paths nginx was built for: the binary
`/bin/custom/nginx`, the configuration `/lib/nginx/nginx.conf` with
upstream's `mime.types`, the default pages `/lib/nginx/html`. A relative
path in a configuration is relative to `/lib/nginx`. nginx writes its
pid, its logs and its temporary files under `/tmp/nginx`.

A master forks the workers (`worker_processes`), which share the
listening sockets, a shared-memory accept mutex (`MAP_SHARED |
MAP_ANON`) and the channels the master talks to them over (`socketpair`,
fds passed with `SCM_RIGHTS`). `listen` takes any address, 127.0.0.0/8 or
the host's own (`docs/sockets-curl.md`). The `user` directive defaults to
root: myos has no superuser to give up, and `setuid` changes nothing.

## Test

`test.sh` (the full test list, after the install): the default
configuration checks; a server of the test's own on 127.0.0.1 answers a
page, a 404, a directory listing, a regex location, a rewrite and a
gzip-compressed response, logs them, and stops on `nginx -s quit`.
