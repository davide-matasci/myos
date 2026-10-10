# Security

myos has no superuser. Every process runs **for a user, in a domain**;
every file has a **label** that its path gives it; and the **policy**
(`/etc/policy`) says what each domain may do to each label. Whatever the
policy does not grant is refused, for every user, root included. On top of
that, a process can be given a **namespace**: only part of the tree, with
fewer rights than the policy would allow (Plan 9's per-process namespaces).

What a process may do to a file is the intersection of three things:

1. its namespace names the file (no name, no access);
2. the binding the name goes through allows the right;
3. the policy grants the right to the process's domain on the file's label.

The code is `kernel/src/sec` (the policy and its checks),
`kernel/src/task/ns.rs` (namespaces) and the checks in the syscalls
(`kernel/src/user/syscall.rs`) and in the file calls modules make for a
process (`kernel/src/modules/mod.rs`: the Linux layer).

## The policy

`etc/policy` in the repository is the default: the image carries it as
`/etc/policy`, and the kernel has its own copy, used when the image's does
not parse. `sec load FILE` replaces the policy until the next boot; it needs
`write` on `kernel.policy`. Running processes keep their user and domain by
name; one the new policy no longer has is left with no rights.

```
mode enforcing                  # or permissive: refusals are logged, not made
boot system init                # the first process (init): user and domain
login -> shell                  # the domain `setuser` enters by default

user root    domains: admin shell untrusted   login: admin
user system  domains: init netd login mount   login: none
user alice   groups: dev   domains: shell untrusted   home: /home/alice   password: sha256:SALT:HEX

label /**                 sys.file
label /bin/**             sys.bin
label /home/$u/**         home($u)
label /home/$u/.ssh/**    secret($u)

domain shell:
    sys.bin {read exec}   home(self) {all}   secret(self) {read}
    shared(group) {read write create}   proc(self) {signal}
domain untrusted:
    sys.bin {read exec}   home(self) {read}

exec /bin/custom/netd -> netd
```

- **Users.** `user NAME` with `groups:`, `domains:` (the domains exec may
  move the user's processes into), `login:` (the domain `setuser` puts them
  in, else the `login ->` line's; `none`: `setuser` never enters the user,
  for an account like `system` that only runs what init starts), `home:`
  (default `/`) and `password:`. A
  user's uid is the position of its line, from 0. `/proc/sys/security/users`
  lists them for libgloss (`getpwnam`, `getpwuid`, `getpwent`).
- **Labels.** `label PATTERN KIND[(OWNER)]`. A pattern is an absolute path
  whose components are literal, `*` (one component), `$name` (one
  component, captured) or a final `**` (any number, none included). The
  **last** rule that matches a file's path gives its label, so the general
  rules come first. The owner is a captured component (`$u`) or a fixed
  name. A file no rule matches is `unlabeled`. Labels are taken from the
  file's **canonical** path, the one bind mounts resolve to, so a file
  reached through a bind has the label of where it is stored (`get-myos`'s
  packages live under `/tmp/pkg`, labelled `sys.pkg`; `/dev/shm`, POSIX
  shared memory, is `/tmp/.shm`, labelled `tmp`).
- **Domains.** `domain NAME:` and rules on the same line or on the indented
  lines below: `KIND(OWNER) {rights}`. The owner is `self` (the process's
  user), `group` (any group of the user), `*` (anyone, or no owner), a fixed
  name, or absent (the label has no owner). `* {rights}` covers every
  label. The rights a domain has on a label are the union of its matching
  rules.
- **Rights.** `read`, `write` (covers `append`), `append`, `create`,
  `remove`, `exec`, `setattr` (file times), `mount`, `signal`, and `all`.
- **Kernel objects** are labels without a path: `kernel.modules` (`insmod`,
  `rmmod`: `write`), `kernel.clock` (`settimeofday`: `write`),
  `kernel.policy` (`sec load`: `write`), `kernel.users` (entering a user who
  has no password: `write`), `kernel.power` (`poweroff`, `reboot`, `halt`:
  `write`, `docs/power.md`), `kernel.mounts` (`mount`, `umount`: `write`; a
  mount or bind changes the tree for every process, so `mount` on a
  directory alone, which `all` grants, is not enough), `proc(USER)`
  (signalling that user's processes, their threads included: `signal`). A
  process may always signal itself.
- **Transitions.** `exec PATTERN -> DOMAIN`: exec of a matching program
  moves the process into the domain, when its user lists it in `domains:`
  (otherwise the program runs in the caller's domain). The last matching
  rule counts.

### What each operation needs

| Operation | Rights |
|---|---|
| `openat` | `read` and/or `write` (`append` with `O_APPEND`, `write` with `O_TRUNC`); a new file `create` too |
| `statat` | any right (a file the caller has none on is not there for it); none for an fd's own file (`fstat`) |
| `listdirat`, `chdirat`, `readlinkat` | `read` |
| `mknodat`, `symlinkat` | `create` (on the new name) |
| `unlinkat` | `remove` |
| `renameat` | `remove` on the old name, `create` on the new one (`remove` too when it replaces a file); for a directory, `remove` on every name beneath it and `create` on where each goes |
| `execat` | `exec` (a script's interpreter and a Linux program's dynamic linker too) |
| `utimensat` | `setattr` |
| `mount`, `umount` | `write` on `kernel.mounts`, `mount` on the directory; a disk `read write`, a bind's source `read` |
| `insmod`, `rmmod` | `read` on the module, `write` on `kernel.modules` |
| `kill` | `signal` on `proc(target's user)` |
| `power` (`poweroff`, `reboot`, `halt`) | `write` on `kernel.power` |

An fd keeps the access it was opened with (one opened `O_WRONLY` does not
read, whatever its holder may do): passing it to another process
(inheritance, a namespace that cannot name the file) is a deliberate grant,
not checked again. A call on an fd's own file (`futimens`, `fdopendir`,
`fstat`'s permission bits) checks the policy against the rights its
opener's namespace had there, not the caller's.
`ftruncate` needs an fd opened for writing, by a process that may `write`
the file: an fd a process could only `append` to is append-only, and takes
every write at the end (`pwrite`'s offset is ignored on an `O_APPEND` fd,
as on Linux). Because an fd is a grant, a
program can keep one from the programs it execs: an fd marked
close-on-exec (`O_CLOEXEC`, `FD_CLOEXEC`) is closed by the exec. libc's
`opendir` and Rust's `std` open their fds so.

`stat` shows what the caller may do: the owner's permission bits are
`r` (read), `w` (write, or create in a directory), `x` (exec, or read for a
directory); the group's and others' are clear. `st_uid` is the label's
owner when it is a user (`home(alice)`: alice's uid), else 0, so `ls -l`
shows who owns a home's files. `chmod` and `chown` change nothing.

Refusals are logged on the console, a few per second:
`sec: denied alice/untrusted write,create on /home/alice/x [home(alice)]`
(`would deny` in permissive mode).

## Users and passwords

The kernel checks passwords. `setuser(name, password)` (libgloss
`myos_setuser`, ubase `login`, `sec as`) runs the calling process as the
user, in its login domain, when the password is right. The policy keeps
`sha256:SALT:HEX`, the SHA-256 of the salt followed by the password:

```sh
printf '%s' "SALTpassword" | sha256sum
```

A user without a password can only be entered from a domain with `write`
on `kernel.users`: `login` (the domain getty and login run in) and
`admin`. So nothing else can become root, which has no password by
default; give it one in the policy to require it at the console. A user
with `login: none` is entered by nobody: `system`, whose processes init
starts, is one, so it cannot be typed at the console.

## The default policy

- `system` runs init (`init`), netd (`netd`), getty and login (`login`),
  init's `mount -a` (`mount`: the ESP at `/boot`, the fstab's partitions,
  `docs/install.md`) and the boot smoke `/bin/custom/ok` (`smoke`), each
  with what it uses.
- `root` logs in to `admin`: the whole system (the image, `/tmp`, devices,
  `/proc`, `/net`, the kernel objects, every home) but not other users'
  `secret`s. It can also run `shell` and `untrusted` programs.
- `shell` is a user's: read and run the system, their own home and secrets,
  `/tmp`, the terminals, the network, their own processes.
- `untrusted` reads the system and its user's home, and nothing more: no
  writes anywhere, no `/tmp`, no network, no secrets.
- `/net` is every user's (`net {read write}`), but a conversation
  (`/net/tcp/N`, `/net/unix/N`: `docs/sockets-curl.md`,
  `docs/sockets-unix.md`) belongs to the user whose process made it (an
  accepted connection to the listener's): only that user's processes open
  its files. The console keyboard itself (`/dev/console/kbd`) is `dev`:
  the administrator's, not every terminal user's.

## Namespaces

A process without a namespace sees the whole tree. `sec ns BIND... --
CMD` (the `ns` syscall, libgloss `myos_ns`) gives the command only the
bindings, each `PATH[=SOURCE][:RIGHTS]`: the real `SOURCE` (named as the
caller sees it; default `PATH`) at `PATH`, with at most `RIGHTS` (default:
all of the caller's there):

```sh
# B gets /dev/sda/data read-write and the programs it needs, and no other disk:
sec ns /bin:read,exec /lib:read /dev/sda/data:read,write -- B
```

- A path under no binding does not exist; a directory above bindings (`/`,
  `/dev` above `/dev/sda/data`) is made up and lists only them.
- A directory a binding leads to lists its own entries and the names bound
  directly below it: with `/` bound to `/` and a file bound at
  `/bin/custom/vim`, `ls /bin/custom` shows vim among the image's programs
  (`run-myos`, `docs/packages.md`).
- A binding's source must be one the caller can name, and its rights are at
  most the caller's there, so a namespace only narrows. There is no way back
  to a name the namespace lacks: `mount` and `bind` need names too.
- The policy still applies: the namespace hides, the policy refuses.
- A namespace is inherited on fork and kept across exec. `chroot DIR`
  (libc) is a namespace of one binding, `DIR` at `/`.
- `/proc/self/fd/N` names a file as the namespace does; a file it cannot
  name has no link to re-open.
- A file handed over as an open fd (`sec ns ... -- B < /dev/sdb/data`) works:
  the fd is the grant.
- A directory handed over as an open fd that the namespace cannot name is
  a **capability** (`sec ns ... -- B 3< /srv/data`): the `*at` calls on it
  (`openat(3, "x/y", ...)`, `mkdirat`, `unlinkat`, `renameat`, ...) work
  beneath it, with the rights its opener's namespace had on the directory
  when it was opened (all of them without a namespace) in place of the
  namespace's, and the policy still applies. Paths stay beneath it: `..`
  above it, an absolute path, an absolute symlink target or a relative one
  leading out fails. A directory opened beneath it is a capability with the
  same rights, beneath which the same holds. It has no name in the
  holder's view, so it cannot be the cwd (`fchdir` fails) nor hold a
  program to exec.

## Limits

- Labels are path rules only: a label cannot be set on a single file (no
  extended attributes).
- No hard links, on purpose: a file has one name, so its label (from that
  name) is the only one it has. A second name elsewhere would give the
  same file another label, and another binding of a namespace could reach
  it. `link`/`linkat` fail with `EPERM`; git renames its objects into
  place instead. A file with several names on an ext2 disk made elsewhere
  is reached by any of them, each its own label.
- `chmod`/`chown` do nothing, and there is one group database entry (root)
  for libc; the policy's groups only feed `shared(group)` rules.
- Linux programs see uid and gid 0 whoever runs them (their rights are
  still their user's and domain's).
- A new policy needs `sec load` (or a reboot), and processes keep their user
  and domain by name across it.
- Per-user `/tmp` is a namespace away (bind `/tmp/USER` at `/tmp`); login
  does not set one up yet.
- The terminals share one label, `dev.tty`: a domain that may open its own
  pty (`/dev/pts/N/data`) may open any user's, and the console (its input
  and its control file, the keymap included). Keep `dev.tty` out of the
  domains of programs that need no terminal.
- A rename of a directory walks its tree to check every name (the labels
  beneath the new name may differ): a large tree takes a moment, and one
  whose directory listing exceeds 256 KiB is refused.
