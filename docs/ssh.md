# SSH into myos

dropbear (`ports/dropbear`) is in the image, as `/bin/custom/dropbear`,
`dbclient` and `dropbearkey`. The image carries no key: no
`authorized_keys`, no host key. Anyone would know a key committed to the
repository, so on a machine others can reach (a VPS) you bring your own.
Password logins are compiled out (`localoptions.h`): a public key is the
only way in.

The image's root is read-only, so the keys go to `/tmp` (the tmpfs), and
dropbear is told where they are: `-D` for the `authorized_keys` directory
and `-r` for the host key. `dropbear -R` does not help: it writes the keys
it makes under `/etc/dropbear`.

```sh
mkdir -p /tmp/ssh
echo 'ssh-ed25519 AAAA... you@laptop' > /tmp/ssh/authorized_keys
dropbearkey -t ed25519 -f /tmp/ssh/hostkey       # prints its fingerprint
dropbear -r /tmp/ssh/hostkey -D /tmp/ssh -p 22   # runs in the background
```

Then `ssh root@<address>` from your machine; `ssh -t` for a login on a
pty. Typing a key at a VPS's web console is error-prone: paste it if the
console lets you, or `curl -o /tmp/ssh/authorized_keys` it from a server
you control (`https://github.com/<you>.keys` works).

Nothing of this survives a reboot: `/tmp` is in memory, and so is the host
key (`ssh` warns that it changed). Nothing starts dropbear at boot either.

The boot test (`ports/dropbear/test.sh`, full mode) does the same with
the repository's test key (`testkey.pub`, packed as test data under
`/lib/myos-tests`, used only while the test runs) and a host key it makes.
