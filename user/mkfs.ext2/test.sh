# ext2 on the scratch disk (/dev/nvme1n1, the launcher's sparse 4 GiB
# target/scratch.img, empty at every boot): format and mount it and copy a
# directory of programs onto it; rename the directory, run one of them from
# the disk and read it through a symlink; then a file past the direct and
# single-indirect blocks (7-10 MB, under the tmpfs file cap it is built in),
# compared with its source, and remove the directory. The host checks the
# disk with `e2fsck -fn` after the boot.
ext2_disk() {
	mkfs.ext2 /dev/nvme1n1 && mount /dev/nvme1n1 /disk ext2 && cp -r /bin/sbase /disk/s
}
ext2_link() {
	mv /disk/s /disk/t && /disk/t/ls -d /disk/t && ln -s t/ls /disk/l && readlink /disk/l && cmp /disk/l /disk/t/ls
}
ext2_big() {
	cat /disk/t/[a-m]* > /tmp/big && cp /tmp/big /disk/big && cmp /tmp/big /disk/big \
		&& rm /disk/t/* /tmp/big && rmdir /disk/t
}
# A file's modification time, as a Linux program's stat sees it (ext2 keeps
# times; the in-kernel filesystems do not).
ext2_mtime() {
	echo x > /disk/m && linux /bin/linux/linux-smoke mtime /disk/m && rm /disk/m
}
t ext2_disk ext2_disk
t ext2_link ext2_link
t ext2_big ext2_big
if grep -q "^linux$" /proc/modules && [ -x /bin/linux/linux-smoke ]; then
	t ext2_mtime ext2_mtime
fi
