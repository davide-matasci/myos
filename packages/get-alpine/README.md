# get-alpine

The Alpine Linux package fetcher of the optional Linux layer
(`docs/linux-compat.md`): it downloads Alpine packages with their run-time
dependencies into a Linux root, checks them and unpacks them, for
`linux --root ROOT PROGRAM` to run with the `linux` module loaded.

A native myos program (newlib, the zlib port and get-myos's download, tar
and gzip code, `user/get-myos/pkgtools.c`), so a package: not in the
image, whatever the build's features.

```sh
get-myos get-alpine                 # install it (an app, docs/packages.md)
insmod /lib/modules/linux           # unless the image loads the layer at boot
run-myos get-alpine jq              # into /data/alpine (/tmp/alpine without /data)
linux --root /data/alpine jq -n '1+1'
```

`test.sh` (full mode, after the install) fetches jq into a scratch root and
checks the files and the records in `ROOT/.get-alpine`; running what it
fetched is the Linux layer's test (`lx_alpine` in `user/tests/kernel.sh`).
