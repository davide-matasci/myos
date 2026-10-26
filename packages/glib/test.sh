# GLib, installed as an app: the smoke (smoke.c) runs the main loop with
# a thread feeding it, threads over an async queue, a GObject's property
# and signal, GRegex, GIO's files, a GSocket echo over loopback, g_spawn
# and g_convert; then gio itself on a file.
glib_smoke() {
	run-myos glib:glib-smoke /tmp || return 1
	echo "gio on myos" > /tmp/gio-test.txt
	out=$(run-myos glib:gio cat /tmp/gio-test.txt) || return 1
	rm -f /tmp/gio-test.txt
	[ "$out" = "gio on myos" ] || { echo "gio cat: '$out'"; return 1; }
}
t -t pkg,glib,mlibc glib glib_smoke
