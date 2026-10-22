#!/bin/sh
# The Raspberry Pi 3's boot files in target/rpi3 (docs/raspberry-pi.md):
#   armstub8-myos.bin  the stub every core starts in (armstub8.S), to EL1
#   u-boot.bin         U-Boot (rpi_3_defconfig), which starts Limine
#   bcm2710-rpi-3-b.dtb  the board's device tree
# U-Boot is built with the GNU aarch64 cross compiler
# (aarch64-linux-gnu-gcc, bison, flex); a file already there is kept, as
# is a stub built from the same source (a CI boot job has the files the
# build job made, `scripts/ci-pack-build-artifacts.sh`, and no compiler to
# spare).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
out="$root/target/rpi3"
. "$here/versions.env"
mkdir -p "$out"

# The stub: clang and lld, as the rest of the tree.
stub_sum=$(sha256sum "$here/armstub8.S" | cut -d' ' -f1)
if [ ! -f "$out/armstub8-myos.bin" ] || [ "$(cat "$out/armstub8.sha256" 2>/dev/null)" != "$stub_sum" ]; then
	clang --target=aarch64-none-elf -c "$here/armstub8.S" -o "$out/armstub8.o"
	ld.lld -Ttext=0 --entry=_start "$out/armstub8.o" -o "$out/armstub8.elf"
	# LLVM's objcopy, else the GNU aarch64 one (CI's image has binutils).
	objcopy=$(command -v llvm-objcopy || command -v aarch64-linux-gnu-objcopy)
	"$objcopy" -O binary "$out/armstub8.elf" "$out/armstub8-myos.bin"
	echo "$stub_sum" > "$out/armstub8.sha256"
fi

dtb="$out/bcm2710-rpi-3-b.dtb"
if ! echo "$DTB_SHA256  $dtb" | sha256sum -c - >/dev/null 2>&1; then
	curl -fsSL -o "$dtb.part" \
		"https://raw.githubusercontent.com/raspberrypi/firmware/$FIRMWARE_TAG/boot/bcm2710-rpi-3-b.dtb"
	echo "$DTB_SHA256  $dtb.part" | sha256sum -c - >/dev/null
	mv "$dtb.part" "$dtb"
fi

uboot="$out/u-boot.bin"
stamp="$out/u-boot.version"
if [ ! -f "$uboot" ] || [ "$(cat "$stamp" 2>/dev/null)" != "$UBOOT_VERSION" ]; then
	if ! command -v aarch64-linux-gnu-gcc >/dev/null; then
		echo "rpi3: U-Boot needs the GNU aarch64 cross compiler (aarch64-linux-gnu-gcc)" >&2
		exit 1
	fi
	src="$out/u-boot-$UBOOT_VERSION"
	tarball="$out/u-boot-$UBOOT_VERSION.tar.bz2"
	if [ ! -d "$src" ]; then
		curl -fsSL -o "$tarball" "https://ftp.denx.de/pub/u-boot/u-boot-$UBOOT_VERSION.tar.bz2"
		echo "$UBOOT_SHA256  $tarball" | sha256sum -c - >/dev/null
		tar -xjf "$tarball" -C "$out"
	fi
	make -s -C "$src" CROSS_COMPILE=aarch64-linux-gnu- rpi_3_defconfig
	make -s -C "$src" -j"$(nproc)" CROSS_COMPILE=aarch64-linux-gnu- u-boot.bin
	cp "$src/u-boot.bin" "$uboot"
	echo "$UBOOT_VERSION" > "$stamp"
fi
