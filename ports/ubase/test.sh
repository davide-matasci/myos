# ubase's df: a line per mounted filesystem (/proc/mounts through
# getmntent), its size, used and available blocks (statvfs) and how full
# it is; /tmp (tmpfs: the kernel heap) has room, and -h prints sizes with
# their units.
ubase_df() {
	df > /tmp/df.out || { cat /tmp/df.out; return 1; }
	cat /tmp/df.out
	line=$(grep ' /tmp$' /tmp/df.out) || return 1
	set -- $line
	[ "$2" -gt 0 ] && [ "$4" -gt 0 ] || return 1
	df -h | grep -q ' /tmp$'
}
t ubase_df ubase_df
