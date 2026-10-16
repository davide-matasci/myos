#!/usr/bin/env bash
# Stage the acme app, target/acme-app: acme.sh is a POSIX shell script, the
# same on every arch, so there is nothing to compile. /lib/acme holds the
# script (with the myos patch: `#!/bin/sh`, the image has no /usr/bin/env
# to run) and its dnsapi/, deploy/ and notify/ hooks, which it finds next
# to itself; /bin/custom/acme is the launcher (this directory's `acme`).
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
# shellcheck source=scripts/myos-c-userspace-lib.sh
source "$ROOT/scripts/myos-c-userspace-lib.sh"
# shellcheck source=packages/acme/versions.env
source "$HERE/versions.env"

if myos_acme_is_current; then
  echo "acme up to date"
  exit 0
fi

"$HERE/fetch.sh"
SRC="$ROOT/target/acme-src"
APP="$ROOT/target/acme-app"
rm -rf "$APP"
mkdir -p "$APP/lib/acme" "$APP/bin/custom"
cp "$SRC/acme.sh" "$SRC/LICENSE.md" "$APP/lib/acme/"
cp -R "$SRC/dnsapi" "$SRC/deploy" "$SRC/notify" "$APP/lib/acme/"
patch -d "$APP/lib/acme" -p1 --forward --batch -s <"$HERE/acme.sh.myos.patch"
chmod 755 "$APP/lib/acme/acme.sh"
cp "$HERE/acme" "$APP/bin/custom/acme"
chmod 755 "$APP/bin/custom/acme"
echo "$(myos_acme_version_hash)" >"$MYOS_ACME_VERSION"
echo "acme staged ($ACME_VERSION)"
