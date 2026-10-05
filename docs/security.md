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
  in, else the `login ->` line's), `home:` (default `/`) and `password:`. A
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
  packages live under `/tmp/pkg`, labelled `sys.pkg`).
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
  has no password: `write`), `proc(USER)` (signalling that user's
  processes: `signal`). A process may always signal itself.
- **Transitions.** `exec PATTERN -> DOMAIN`: exec of a matching program
  moves the process into the domain, when its user lists it in `domains:`
  (otherwise the program runs in the caller's domain). The last matching
  rule counts.

### What each operation needs

| Operation | Rights |
|---|---|
| `open` | `read` and/or `write` (`append` with `O_APPEND`, `write` with `O_TRUNC`); a new file `create` too |
| `stat`, `lstat` | any right (a file the caller has none on is not there for it) |
| `listdir`, `chdir`, `readlink` | `read` |
| `mkdir`, `mkfifo`, `symlink` | `create` (on the new name) |
| `unlink`, `rmdir` | `remove` |
| `rename` | `remove` on the old name, `create` on the new one (`remove` too when it replaces a file) |
| `exec` | `exec` (a script's interpreter too) |
| `utimensat`, `futimens` | `setattr` |
| `mount`, `umount` | `mount` on the directory; a disk `read write`, a bind's source `read` |
| `insmod`, `rmmod` | `read` on the module, `write` on `kernel.modules` |
| `kill` | `signal` on `proc(target's user)` |

An fd keeps the access it was opened with: passing it to another process
(inheritance, a namespace that cannot name the file) is a deliberate grant,
not checked again.

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
default; give it one in the policy to require it at the console.

## The default policy

- `system` runs init (`init`), netd (`netd`), getty and login (`login`) and
  the boot smoke `/bin/custom/ok` (`smoke`), each with what it uses.
- `root` logs in to `admin`: the whole system (the image, `/tmp`, devices,
  `/proc`, `/net`, the kernel objects, every home) but not other users'
  `secret`s. It can also run `shell` and `untrusted` programs.
- `shell` is a user's: read and run the system, their own home and secrets,
  `/tmp`, the terminals, the network, their own processes.
- `untrusted` reads the system and its user's home, and nothing more: no
  writes anywhere, no `/tmp`, no network, no secrets.

## Namespaces

A process without a namespace sees the whole tree. `sec ns BIND... --
CMD` (the `ns` syscall, libgloss `myos_ns`) gives the command only the
bindings, each `PATH[=SOURCE][:RIGHTS]`: the real `SOURCE` (named as the
caller sees it; default `PATH`) at `PATH`, with at most `RIGHTS` (default:
all of the caller's there):

```sh
# B gets /dev/sda read-write and the programs it needs, and no other disk:
sec ns /bin:read,exec /lib:read /dev/sda:read,write -- B
```

- A path under no binding does not exist; a directory above bindings (`/`,
  `/dev` above `/dev/sda`) is made up and lists only them.
- A binding's source must be one the caller can name, and its rights are at
  most the caller's there, so a namespace only narrows. There is no way back
  to a name the namespace lacks: `mount` and `bind` need names too.
- The policy still applies: the namespace hides, the policy refuses.
- A namespace is inherited on fork and kept across exec. `chroot DIR` is a
  namespace of one binding, `DIR` at `/`.
- `/proc/self/fd/N` names a file as the namespace does; a file it cannot
  name has no link to re-open.
- A file handed over as an open fd (`sec ns ... -- B < /dev/sdb`) works:
  the fd is the grant.

## Limits

- Labels are path rules only: a label cannot be set on a single file (no
  extended attributes).
- `chmod`/`chown` do nothing, and there is one group database entry (root)
  for libc; the policy's groups only feed `shared(group)` rules.
- Linux programs see uid and gid 0 whoever runs them (their rights are
  still their user's and domain's).
- A new policy needs `sec load` (or a reboot), and processes keep their user
  and domain by name across it.
- Per-user `/tmp` is a namespace away (bind `/tmp/USER` at `/tmp`); login
  does not set one up yet.
