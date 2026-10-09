# limine: the tool runs (`get-myos --install` runs its bios-install on x86,
# user/get-myos/test.sh, and the boot test boots the disk it wrote).
limine_version() {
	limine --version
	limine --version | grep -q '^Limine [0-9][0-9.]*$'
}
t limine_version limine_version
