# acme for myos

**acme.sh 3.1.6** (fetched at build, not vendored; `versions.env` pins the
release tag and its commit), the ACME client: certificates from Let's
Encrypt and the other ACME CAs, as a POSIX shell script over `curl` (in
the image) and the `openssl` app (`packages/openssl`, installed with it).
The app is `/lib/acme` (the script with its `dnsapi/`, `deploy/` and
`notify/` hooks, which it finds next to itself) and `/bin/custom/acme`,
a launcher that gives it `--home /data/acme` unless `--home` or
`LE_WORKING_DIR` says otherwise: acme.sh's own default, `~/.acme.sh`, is
nowhere to write on myos (`HOME` is `/`, the root read-only), and `/data`
is the installed system's disk (`docs/install.md`). The one myos patch
is the script's first line: `#!/bin/sh` for `#!/usr/bin/env sh`.

## A certificate for nginx, on an installed system

The name must point at the machine (an `A` record), and nginx must serve
the challenge directory on port 80, the only port the CA's http-01
validation knocks on:

```nginx
server {
	listen 80;
	root /data/www;
}
```

```sh
get-myos acme                                   # and openssl, which it needs
run-myos acme --set-default-ca --server letsencrypt   # once; acme.sh's default is ZeroSSL
run-myos acme --issue -d example.org -w /data/www
run-myos acme --install-cert -d example.org --ecc \
	--key-file /data/nginx/example.org.key \
	--fullchain-file /data/nginx/example.org.crt \
	--reloadcmd "run-myos nginx -c /data/nginx/nginx.conf -s reload"
```

`--issue` makes an EC P-256 account key and certificate key (`--keylength
2048` for RSA, slower on a small machine), writes the challenge token
under the webroot's `.well-known/acme-challenge/`, and keeps the
certificate, its key and the chain under `/data/acme/example.org_ecc/`.
`--install-cert` copies them where nginx reads them and runs the reload
command, now and at every renewal:

```nginx
server {
	listen 443 ssl;
	ssl_certificate /data/nginx/example.org.crt;
	ssl_certificate_key /data/nginx/example.org.key;
	root /data/www;
}
```

Let's Encrypt's certificates last 90 days and acme.sh renews at 60. myos
has no cron: run the renewal yourself, about every two months, or from
`/data/etc/rc.local` at a boot. It renews what is due, nothing otherwise,
and reloads nginx:

```sh
run-myos acme --cron
```

`run-myos acme --list` shows the certificates and their renewal dates.
Other validation methods (`--dns dns_cf`, the DNS API hooks, for a name
without a reachable port 80) and the deploy hooks are acme.sh's own; see
its documentation. Do not `--upgrade` acme.sh: the package is its version.

## The test

`test.sh` (full mode: the packages installed) issues a certificate from
the boot test's own ACME server on the host (`acme-server.py`, run by
`host.sh` for the guest's `HOST acme serve` line: RFC 8555 over Python's
standard library and the host's openssl, with the JWS signatures verified
and the http-01 token fetched from the guest's nginx through QEMU's
forward of port 5002), verifies the chain against the server's CA with
the openssl app, installs it with `--install-cert` into an nginx HTTPS
server that curl then checks for the name, and renews it. The server is
not a CA: no DNS (it fetches the token from the forwarded port whatever
the name), no rate limits, no revocation.

License: GPL-3.0 (`licenses/GPL-3.0.txt`; `THIRD_PARTY_NOTICES.md`).
