#!/bin/sh
# TEMP boot-test runner (removed before commit).
# /lib is read-only (initramfs libfs), so copy the tree to tmpfs first.
rm -rf /tmp/ostest
cp -R /lib/os-test /tmp/ostest || exit 1
cd /tmp/ostest || exit 1
make
sh misc/myos-report.sh
