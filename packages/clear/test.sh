# clear, installed as a package: it writes the terminal's clear capability to
# stdout. getty sets TERM=linux and TERMCAP=/lib/termcap, whose linux entry has
# cl=\E[H\E[J (home, then erase-to-end-of-display) and no E3 scrollback cap.
clear_run() {
	command -v clear >/dev/null 2>&1 || return 1
	# -T linux pins the entry, so the bytes are fixed regardless of $TERM.
	out=$(clear -T linux)
	exp=$(printf '\033[H\033[J')
	[ "$out" = "$exp" ] || { echo "clear -T linux: unexpected output"; return 1; }
	# The default path (getty's TERM/TERMCAP) must also succeed.
	clear >/dev/null || return 1
	# -V prints the ncurses version it was built from.
	clear -V | grep -q '^ncurses '
}
t clear clear_run
