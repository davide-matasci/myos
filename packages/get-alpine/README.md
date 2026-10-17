# get-alpine

The Alpine Linux package installer of the optional Linux layer
(`docs/linux-compat.md`): it bootstraps a Linux root with Alpine's own
package manager, `apk` (downloaded with its dependencies, checked and
unpacked natively), and has apk, run under the layer, install the packages
asked for, scripts and triggers included, for `linux --root ROOT PROGRAM`
to run. From then on `linux --root ROOT apk ...` manages the root.

A native myos program (newlib, the zlib port and get-myos's download, tar
and gzip code, `user/get-myos/pkgtools.c`), so a package: not in the
image, whatever the build's features.

```sh
get-myos get-alpine                 # install it (an app, docs/packages.md)
insmod /lib/modules/linux           # unless the image loads the layer at boot
run-myos get-alpine jq              # into /data/alpine (/tmp/alpine without /data)
linux --root /data/alpine jq -n '1+1'
linux --root /data/alpine apk add python3   # or get-alpine python3
```

The root needs room: a boot disk written from the image has a 64 MiB
`/data` (`docs/install.md`, issue #488), enough for get-myos's apps, not
for an Alpine root (a browser's takes some hundreds of MiB); `get-myos
--install` gives the data partition the rest of the disk. On a full
filesystem apk stops with `No space left on device`.

Besides the packages asked for, a root always gets the bootstrap set:
`apk-tools`, `alpine-keys`, `busybox`, `busybox-binsh`, `alpine-baselayout`
and Alpine's CA bundle (`ca-certificates-bundle`, the trust store of TLS
clients such as Python, curl and wget, which do not depend on it).
`-n` unpacks the packages natively instead, without apk (and without their
install scripts): what `linux-compat/alpine-disk.sh` does on the host.

`test.sh` (full mode, after the install) installs jq into a scratch root
and checks the files, apk's database and busybox's trigger; running what
it installed is the Linux layer's test (`lx_alpine` in
`user/tests/kernel.sh`).
