#!/bin/sh
echo '=== repro start ==='
rm -rf /tmp/ostest
cp -R /lib/os-test /tmp/ostest
cd /tmp/ostest
make SUITES=pwd 2>&1 | tail -5
echo "make rc=$?"
sh misc/myos-report.sh 2>/dev/null | head -20
echo '=== repro end ==='
