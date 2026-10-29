# Security

myos has no superuser. Every process runs **for a user, in a domain**.
Every file has a **label**, which its path gives it. The **policy**
(`/etc/policy`) says what each domain may do to each label. Whatever the
policy does not grant is refused, for every user, root included. On top
of that, a process can be given a **namespace**: only part of the tree,
with fewer rights than the policy would allow, as in Plan 9.

What a process may do to a file is the intersection of three things:

1. its namespace names the file (no name, no access);
2. the binding the name goes through allows the right;
3. the policy grants the right to the process's domain on the file's label.

The code: `kernel/src/sec` (the policy language and its checks),
`kernel/src/task/ns.rs` (namespaces), and the checks in the syscalls
(`kernel/src/user/syscall.rs`) and in the file calls modules make for a
process (`kernel/src/modules/mod.rs`: the Linux layer). The tests are in
`user/tests/kernel.sh` (tag `sec`), run as root in `admin`.

## The policy

`etc/policy` in the repository is the default. The image carries it as
`/etc/policy`, and the kernel has its own copy, used when the image's
does not parse. `sec load FILE` replaces it until the next boot; it needs
`write` on `kernel.policy`. Running processes keep their user and domain
by name; one whose user or domain the new policy lacks has no rights.

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

- **Users.** `user NAME` takes `groups:`, `domains:` (where exec may move
  its processes), `login:` (the domain `setuser` enters, else the
  `login ->` line's; `none` means nobody enters the user), `home:`
  (default `/`) and `password:`. A user's uid is the position of its
  line, from 0. `/proc/sys/security/users` lists them for libgloss's
  `getpwnam` and friends.
- **Labels.** `label PATTERN KIND[(OWNER)]`. Each path component is a
  literal, `*` (one component), `$name` (one, captured) or a final `**`
  (any number, none included). The **last** matching rule wins, so the
  general rules come first; a file no rule matches is `unlabeled`.
- **Domains.** `domain NAME:` is followed by `KIND(OWNER) {rights}` rules.
  The owner is `self` (the process's user), `group` (any of its groups),
  `*` (anyone, or no owner), a fixed name, or absent (the label has no
  owner). `* {rights}` covers every label. A domain's rights on a label
  are the union of its matching rules.
- **Rights**: `read`, `write` (covers `append`), `append`, `create`,
  `remove`, `exec`, `setattr` (file times), `mount`, `signal`, `all`.
- **Transitions.** `exec PATTERN -> DOMAIN`: exec of a matching program
  moves the process into the domain when its user lists the domain in
  `domains:`; otherwise the program runs in the caller's domain. The last
  matching rule counts.

Labels come from the file's **canonical** path, the one bind mounts
resolve to, so a file reached through a bind or a namespace has the label
of where it is stored. `get-myos`'s apps (`/data/apps`, `/tmp/apps`) and
`get-alpine`'s root (`/data/alpine`) are `sys.pkg` whatever path
`run-myos` shows them at; `/dev/shm` is `/tmp/.shm`, labelled `tmp`.

### Kernel objects

| Label | What it guards |
|---|---|
| `kernel.modules` | `insmod`, `rmmod`: `write` |
| `kernel.clock` | `settimeofday`: `write` |
| `kernel.policy` | `sec load`: `write` |
| `kernel.users` | entering a user who has no password: `write` |
| `kernel.power` | `poweroff`, `reboot`, `halt`: `write` (`docs/power.md`) |
| `kernel.mounts` | `mount`, `umount`: `write` |
| `proc(USER)` | signalling that user's processes, their threads included: `signal` |

A mount changes the tree for every process, so `mount` on the directory
is not enough: `kernel.mounts` is needed too. A process may always signal
itself.

### What each operation needs

| Operation | Rights |
|---|---|
| `openat` | `read` and/or `write` (`append` with `O_APPEND`, `write` with `O_TRUNC`); a new file `create` too |
| `statat`, `statfsat` | any right (a file the caller has none on is not there for it); none on an fd's own file |
| `listdirat`, `chdirat`, `readlinkat` | `read` |
| `mknodat`, `symlinkat` | `create` |
| `unlinkat` | `remove` |
| `renameat` | `remove` on the old name, `create` on the new (`remove` too when it replaces a file); for a directory, the same for every name beneath it |
| `execat` | `exec` (a script's interpreter and a Linux program's dynamic linker too) |
| `utimensat` | `setattr` |
| `mount`, `umount` | `write` on `kernel.mounts`, `mount` on the directory (a bind's target); a disk `read write`, a bind's source `read` |
| `insmod`, `rmmod` | `read` on the module, `write` on `kernel.modules` |
| `kill` | `signal` on `proc(target's user)` |
| `power` | `write` on `kernel.power` |

### File descriptors

An fd keeps the access it was opened with, and passing one to another
process (by inheritance, or into a namespace that cannot name the file)
is a deliberate grant, not checked again. A call on an fd's own file
(`futimens`, `fdopendir`, `fstat`'s bits) checks the rights its opener's
namespace had there; `ftruncate` needs an fd opened for writing. An fd a
process could only `append` to takes every write at the end (`pwrite`'s
offset is ignored on `O_APPEND`, as on Linux). To keep an fd from the
programs it execs, a program marks it close-on-exec; libc's `opendir` and
Rust's `std` do.

### What `stat` shows

`stat` shows what the caller may do: the owner's bits are `r` (read), `w`
(write, or create in a directory) and `x` (exec, or read for a
directory); the group's and others' are clear. `st_uid` is the label's
owner when it is a user, else 0, so `ls -l` shows who owns a home's
files. `chmod` and `chown` change nothing.

### Refusals

Refusals are logged on the console, a few per second (`would deny` in
permissive mode):

```
sec: denied alice/untrusted write,create on /home/alice/x [home(alice)]
```

## Users and passwords

The kernel checks passwords. `setuser(name, password)` (libgloss
`myos_setuser`, ubase `login`, `sec as`) runs the calling process as the
user, in its login domain. The policy keeps `sha256:SALT:HEX`, the
SHA-256 of the salt followed by the password:

```sh
printf '%s' "SALTpassword" | sha256sum
```

A user without a password can be entered only from a domain with `write`
on `kernel.users` (`login`, where getty and login run, and `admin`), so
nothing else becomes root, which has no password by default. Give root a
password in the policy to require one at the console. A `login: none`
user, such as `system`, is entered by nobody.

## The default policy

`etc/policy` is commented; in short:

- **`system`** runs init, netd, the sound server, getty and login, init's
  `mount -a` and the boot smoke, each in its own domain. Only `sndd` and
  `admin` open `/dev/sound`'s streams (`docs/sound.md`).
- **`/etc/rc`** runs in `rc`, which may only read the system and enter a
  passwordless user: it enters root at once (`docs/install.md`).
- **`root`** logs in to `admin`: the whole system, but not other users'
  `secret`s, such as dropbear's keys (`secret(root)`, `docs/ssh.md`).
- **`shell`**, a user's domain: the system, their own home and secrets,
  `/tmp`, their terminals, the network, their own processes.
- **`untrusted`** reads the system and its user's home, nothing more.

`/net` is every user's, but a conversation (`/net/tcp/N`, `/net/unix/N`:
`docs/net.md`, `docs/sockets-unix.md`) belongs to the user whose process
made it; an accepted one, to the listener's user. The event files
(`/dev/event/N`, `/dev/timer/N`, `/dev/wait/N`, `/dev/sig/N`:
`docs/events.md`) are `dev.common` and belong the same way to the user
whose process opened `clone`.

### Terminals

A terminal (`docs/tty.md`) is `dev.tty(USER)`, where the user is the
leader's of the session that claimed it (for an unclaimed pty, its
opener's); an unclaimed kernel tty and the console are nobody's. `login`
enters the user in getty's process, the session leader, so a login hands
the terminal to its user. `shell` and `untrusted` have `dev.tty(self)`,
so a user opens neither another session's terminal nor an unclaimed one,
where the next login's password is typed; the system's domains and
`admin` have `dev.tty(*)`. The console keyboard and pointer are `dev`,
the administrator's.

A kernel tty hangs up when it changes hands, as with Linux's `vhangup`:
older fds on its `data` read and write `EIO`, and its `ctl` is checked at
every write. So a program left behind by a session cannot reach the next
login's. A pty does not hang up, and a pair is not reused while an fd
holds it.

## Namespaces

A process without a namespace sees the whole tree. `sec ns BIND... --
CMD` (the `ns` syscall, libgloss `myos_ns`) gives the command only the
bindings. Each is `PATH[=SOURCE][:RIGHTS]`: `SOURCE` (as the caller names
it; default `PATH`) appears at `PATH`, with at most `RIGHTS` (default:
all the caller's rights there).

```sh
# B gets /dev/sda/data read-write and the programs it needs, and no other disk:
sec ns /bin:read,exec /lib:read /dev/sda/data:read,write -- B
```

- A path under no binding does not exist. A directory above bindings is
  made up and lists only them; a bound directory lists its own entries
  plus the names bound directly below it (how `run-myos` adds an app to
  `/bin/custom`, `docs/packages.md`).
- A namespace only narrows: a source must be nameable by the caller, its
  rights are at most the caller's, and `mount` and `bind` need names too.
  The policy still applies.
- It is inherited on fork and kept across exec. `chroot DIR` (libc) is a
  namespace of one binding, `DIR` at `/`.
- `/proc/self/fd/N` names a file as the namespace does; a file it cannot
  name has no link to re-open.
- An open fd handed over still works: it is the grant
  (`sec ns ... -- B < /dev/sdb/data`).

### Directory capabilities

A directory handed over as an fd the namespace cannot name is a
**capability** (`sec ns ... -- B 3< /srv/data`). The `*at` calls work
beneath it, with the rights its opener's namespace had on it, and the
policy still applies. Paths stay beneath it: `..` above it, absolute
paths and symlinks leading out fail. Directories opened beneath it are
capabilities too. It has no name in the holder's view, so it cannot be
the cwd (`fchdir` fails) nor hold a program to exec.

## Limits

- Labels are path rules only: no label on a single file (no extended
  attributes).
- No hard links, on purpose: a file has one name, so one label, and no
  second name lets a namespace reach it. `link` fails with `EPERM`. (A
  file with several names on a foreign ext2 disk gets each name's label.)
- `chmod` and `chown` do nothing; libc has one group entry (root). The
  policy's groups only feed `shared(group)` rules.
- Linux programs see uid and gid 0; their rights are still their user's
  and domain's.
- A new policy needs `sec load` or a reboot.
- Per-user `/tmp` is a namespace away (bind `/tmp/USER` at `/tmp`), but
  login does not set one up yet.
- A process another user starts in a session (`sec as`) reaches its
  terminal only through inherited fds and `/dev/tty`.
- A directory rename checks every name beneath it, as the labels may
  differ: a large tree takes a moment, and one whose listing exceeds
  256 KiB is refused.
