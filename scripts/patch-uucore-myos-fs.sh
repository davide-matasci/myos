#!/usr/bin/env bash
# uucore fs.rs: treat myos like unix for FileInformation (rustix stat backend).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fs="$ROOT/user/uutils-coreutils/src/uucore/src/lib/features/fs.rs"
fsext="$ROOT/user/uutils-coreutils/src/uucore/src/lib/features/fsext.rs"

[[ -f "$fs" ]] || exit 0

stamp="$(dirname "$fs")/.myos-fs-patch-done"
[[ -f "$stamp" ]] && exit 0

sed -i \
  -e 's/#\[cfg(unix)\]/#[cfg(any(unix, target_os = "myos"))]/g' \
  -e 's/#\[cfg(not(unix))\]/#[cfg(not(any(unix, target_os = "myos")))]/g' \
  -e 's/#\[cfg(all(unix,/#[cfg(all(any(unix, target_os = "myos"),/g' \
  -e 's/#\[cfg(any(unix,/#[cfg(any(unix, target_os = "myos",/g' \
  "$fs"

sed -i '/#\[cfg(windows)\]/,/return self.0.number_of_links();/{ /return self.0.number_of_links();/a\
        #[cfg(target_os = "myos")]\
        return self.0.st_nlink;
}' "$fs"

if [[ -f "$fsext" ]] && ! grep -q 'target_os = "myos"' "$fsext"; then
  sed -i 's/target_os = "aix",/target_os = "aix",\n        target_os = "myos",/' "$fsext"
fi

touch "$stamp"
