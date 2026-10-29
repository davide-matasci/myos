# SSH into myos

The image carries dropbear (`ports/dropbear`), an SSH server and client,
installed as `/bin/custom/dropbear` with `dbclient` (also `ssh`, which git
runs for an SSH remote, `packages/git/README.md`) and `dropbearkey`. The
boot starts the server on port 2222, so you can log in to a running myos
once you have put your public key on it.

## Logging in

The boot script's default `rc.local` (`docs/install.md`) runs `dropbear
-R -D /etc/dropbear -p 2222`. Edit `/data/etc/rc.local` for another port,
another key directory, or no SSH at all.

Password logins are compiled out (`localoptions.h`): a public key is the
only way in, and with no `authorized_keys` nobody gets in. Add your key:

```sh
echo 'ssh-ed25519 AAAA... you@laptop' >> /etc/dropbear/authorized_keys
```

Then `ssh -p 2222 root@<address>` (`-t` for a pty). Under QEMU (`cargo
run`) the launcher forwards the host's port 2222, when free, so `ssh -p
2222 root@localhost` works. At a VPS's web console, paste the key or
fetch it with `curl -o /etc/dropbear/authorized_keys
https://github.com/<you>.keys` rather than typing it.

## Keys and `/etc/dropbear`

`/etc/dropbear` is in the read-only image; the boot script binds a
writable directory over it: `/data/dropbear` on the data partition
(kept across reboots and upgrades), or `/tmp/dropbear` without one. Only
root can read it (`secret(root)`, `docs/security.md`).

- **The host key** (`dropbear_ed25519_host_key`) is made by `-R` at the
  first connection. The image carries none, and on the data partition it
  stays the same from then on.
- **`authorized_keys`** lists the keys that may log in as root. The image
  carries none: a key committed to the repository would be known to
  anyone.

## Test

`ports/dropbear/test.sh` (full mode) logs in to the boot's dropbear with
the repository's test key (`testkey.pub`, under `/lib/myos-tests`), which
it puts in `authorized_keys` only while the test runs.
