# SSH into myos

dropbear (`ports/dropbear`) is in the image, as `/bin/custom/dropbear`,
`dbclient` (also `ssh`: what git runs for an SSH remote,
`packages/git/README.md`) and `dropbearkey`. The boot script's default `rc.local`
(`docs/install.md`) starts it on port 2222:

```sh
dropbear -R -D /etc/dropbear -p 2222
```

`/etc/dropbear` is a directory of the read-only image that the boot script
binds `/data/dropbear` over (the data partition: it survives reboots and
upgrades), or `/tmp/dropbear` when there is no data partition (until the
next boot). Only root can read it (`secret(root)`, `docs/security.md`).

- **The host key** is dropbear's own identity, which `ssh` remembers in
  `known_hosts`. The image carries none: `-R` makes one at the first
  connection (`/etc/dropbear/dropbear_ed25519_host_key`), and on the data
  partition it stays the same from then on.
- **`authorized_keys`** lists the public keys that may log in, as root
  (`-D` names its directory). The image carries none either: anyone would
  know a key committed to the repository, so you bring your own. Password
  logins are compiled out (`localoptions.h`): a public key is the only way
  in, and with no `authorized_keys` nobody gets in.

```sh
echo 'ssh-ed25519 AAAA... you@laptop' >> /etc/dropbear/authorized_keys
```

Then `ssh -p 2222 root@<address>` from your machine; `ssh -t` for a login
on a pty. Under QEMU (`cargo run`) the launcher forwards the host's port
2222 to the guest's: `ssh -p 2222 root@localhost`. Typing a key at a VPS's
web console is error-prone: paste it if the console lets you, or `curl -o
/etc/dropbear/authorized_keys` it from a server you control
(`https://github.com/<you>.keys` works).

Another port, another key directory, or no SSH at all: edit
`/data/etc/rc.local`.

The boot test (`ports/dropbear/test.sh`, full mode) logs in to the boot's
dropbear with the repository's test key (`testkey.pub`, packed as test
data under `/lib/myos-tests`), which it puts in `authorized_keys` only
while the test runs.
