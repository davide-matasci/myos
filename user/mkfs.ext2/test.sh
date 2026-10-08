# ext2 on the scratch disk (/dev/nvme1n1/data, the launcher's sparse 4 GiB
# target/scratch.img, empty at every boot): format and mount it and copy
# programs into a directory on it (sixteen names of the sbase multicall
# ELF, a copy each); rename the directory, run one of them from the disk
# and read it through a symlink; then a file past the direct and
# single-indirect blocks (6-9 MB: fifteen of those copies, under the tmpfs
# file cap it is built in), compared with its source, and remove the
# directory; set a file's times; keep files an fd holds past an unlink and
# a rename over them; unmount it and mount it again, the files still there.
# The host checks the disk with `e2fsck -fn` after the boot.
# The block cache (kernel/src/blk/cache.rs) on the raw scratch disk, before
# it is formatted: what is read back is what was written, a write into the
# middle of a cached page included, and the cache keeps what was read.
blk_cache() {
	dd if=/bin/sbase/ls of=/tmp/a bs=4096 count=8 2>/dev/null \
		&& dd if=/tmp/a of=/dev/nvme1n1/data bs=4096 2>/dev/null \
		&& dd if=/dev/nvme1n1/data of=/tmp/b bs=4096 count=8 2>/dev/null && cmp /tmp/a /tmp/b \
		&& cp /tmp/a /tmp/c && echo cached | dd of=/tmp/c bs=1 seek=5000 conv=notrunc 2>/dev/null \
		&& echo cached | dd of=/dev/nvme1n1/data bs=1 seek=5000 conv=notrunc 2>/dev/null \
		&& dd if=/dev/nvme1n1/data of=/tmp/b bs=4096 count=8 2>/dev/null && cmp /tmp/c /tmp/b \
		&& grep -q "^BlockCacheKiB: [1-9]" /proc/meminfo && rm /tmp/a /tmp/b /tmp/c
}
ext2_disk() {
	mkdir -p /tmp/disk && mkfs.ext2 /dev/nvme1n1/data && mount /dev/nvme1n1/data /tmp/disk ext2 \
		&& mkdir /tmp/disk/s && cp /bin/sbase/[a-c]* /bin/sbase/ls /tmp/disk/s
}
ext2_link() {
	mv /tmp/disk/s /tmp/disk/t && /tmp/disk/t/ls -d /tmp/disk/t && ln -s t/ls /tmp/disk/l && readlink /tmp/disk/l && cmp /tmp/disk/l /tmp/disk/t/ls
}
ext2_big() {
	cat /tmp/disk/t/[a-c]* > /tmp/big && cp /tmp/big /tmp/disk/big && cmp /tmp/big /tmp/disk/big \
		&& rm /tmp/disk/t/* /tmp/big && rmdir /tmp/disk/t
}
# Setting a file's times on the disk (the ext2 module's set_times).
ext2_times() {
	/bin/sbase/touch -T 946782240 /tmp/disk/m && /bin/sbase/ls -l /tmp/disk/m | grep -q "Jan 02  2000" \
		&& rm /tmp/disk/m
}
# A file unlinked, or replaced by a rename, while an fd holds it stays
# readable through the fd (the module keeps its inode until the fd closes),
# apart from the new file that took its name.
ext2_held() {
	echo one > /tmp/disk/h
	echo two > /tmp/disk/r
	exec 4< /tmp/disk/h 5< /tmp/disk/r
	rm /tmp/disk/h
	echo new > /tmp/disk/h
	echo three > /tmp/disk/s
	mv /tmp/disk/s /tmp/disk/r
	got=$(cat <&4) got2=$(cat <&5)
	exec 4<&- 5<&-
	echo "unlinked: $got, replaced: $got2"
	[ "$got" = one ] && [ "$got2" = two ] && [ "$(cat /tmp/disk/h)" = new ] && [ "$(cat /tmp/disk/r)" = three ] \
		&& rm /tmp/disk/h /tmp/disk/r
}
# The file calls on the disk: O_EXCL, ftruncate (blocks given back, zeros
# when it grows again), pread/pwrite, close-on-exec (fileio_smoke.c).
ext2_files() {
	/bin/etc/fileio_smoke /tmp/disk
}
# Shared mappings of files on the disk, written back through the module
# (mmap_smoke.c).
ext2_mmap() {
	/bin/etc/mmap_smoke /tmp/disk
}
# A file's modification time, as a Linux program's stat sees it.
ext2_mtime() {
	echo x > /tmp/disk/m && linux /bin/linux/linux-smoke mtime /tmp/disk/m && rm /tmp/disk/m
}
t blk_cache blk_cache
t ext2_disk ext2_disk
t ext2_link ext2_link
t ext2_big ext2_big
t ext2_times ext2_times
t ext2_held ext2_held
t ext2_files ext2_files
t ext2_mmap ext2_mmap
if grep -q "^linux$" /proc/modules && [ -x /bin/linux/linux-smoke ]; then
	t ext2_mtime ext2_mtime
fi
# umount writes back what the module caches: the directory is empty while
# the disk is not mounted, the file is back once it is again.
ext2_remount() {
	echo kept > /tmp/disk/k && umount /tmp/disk || return 1
	[ -z "$(ls -A /tmp/disk)" ] || { echo "files left after umount"; return 1; }
	mount /dev/nvme1n1/data /tmp/disk ext2 && [ "$(cat /tmp/disk/k)" = kept ] && rm /tmp/disk/k
}
t ext2_remount ext2_remount
