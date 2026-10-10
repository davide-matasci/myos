# clear, installed as an app: it writes the terminal's clear capability to
# stdout. getty sets TERM=linux and TERMCAP=/lib/termcap, whose linux entry has
# cl=\E[H\E[J (home, then erase-to-end-of-display) and no E3 scrollback cap.
clear_run() {
	# -T linux pins the entry, so the bytes are fixed regardless of $TERM.
	out=$(run-myos clear -T linux)
	exp=$(printf '\033[H\033[J')
	[ "$out" = "$exp" ] || { echo "clear -T linux: unexpected output"; return 1; }
	# The default path (getty's TERM/TERMCAP) must also succeed.
	run-myos clear >/dev/null || return 1
	# -V prints the ncurses version it was built from.
	run-myos clear -V | grep -q '^ncurses '
}
t clear clear_run
