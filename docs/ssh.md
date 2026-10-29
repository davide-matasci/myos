# SSH into myos

The image carries dropbear (`ports/dropbear`), an SSH server and client.
The boot starts the server on port 2222, so you can log in to a running
myos from another machine once you have put your public key on it.

## Programs

dropbear is installed as `/bin/custom/dropbear`, with:

- `dbclient`, also installed as `ssh`. git runs it for an SSH remote
  (`packages/git/README.md`).
- `dropbearkey`, which makes keys.

## Logging in

The boot script's default `rc.local` (`docs/install.md`) starts the
server:

```sh
dropbear -R -D /etc/dropbear -p 2222
```

Password logins are compiled out (`localoptions.h`). A public key is the
only way in, and with no `authorized_keys` nobody gets in. Add your key:

```sh
echo 'ssh-ed25519 AAAA... you@laptop' >> /etc/dropbear/authorized_keys
```

Then, from your machine:

- `ssh -p 2222 root@<address>`;
- `ssh -t` for a login on a pty;
- under QEMU (`cargo run`), `ssh -p 2222 root@localhost`. The launcher
  forwards the host's port 2222 to the guest's, when that host port is
  free.

Typing a key at a VPS's web console is error-prone. Paste it if the
console lets you, or fetch it from a server you control with `curl -o
/etc/dropbear/authorized_keys` (`https://github.com/<you>.keys` works).

To use another port, another key directory, or no SSH at all, edit
`/data/etc/rc.local`.

## Keys and `/etc/dropbear`

`/etc/dropbear` is a directory of the read-only image. The boot script
binds a writable directory over it:

- `/data/dropbear` on the data partition, which survives reboots and
  upgrades;
- `/tmp/dropbear` when there is no data partition, which lasts until the
  next boot.

Only root can read it (`secret(root)`, `docs/security.md`). It holds two
kinds of key:

- **The host key** is dropbear's own identity, which `ssh` remembers in
  `known_hosts`. The image carries none. `-R` makes one at the first
  connection (`/etc/dropbear/dropbear_ed25519_host_key`). On the data
  partition it stays the same from then on.
- **`authorized_keys`** lists the public keys that may log in, as root
  (`-D` names its directory). The image carries none either: anyone would
  know a key committed to the repository, so you bring your own.

## Test

The boot test (`ports/dropbear/test.sh`, full mode) logs in to the boot's
dropbear with the repository's test key (`testkey.pub`, packed as test
data under `/lib/myos-tests`). It puts the key in `authorized_keys` only
while the test runs.
