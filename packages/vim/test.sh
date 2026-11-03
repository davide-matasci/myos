# vim, installed as an app: run-myos runs it with its vimrc at its image
# path, in its view only.
vim_version() {
	run-myos vim --version | head -n 1 | grep -q "VIM" || return 1
	run-myos vim:/bin/sh -c '[ -f /lib/vim/vimrc ]' && [ ! -e /lib/vim ]
}
t vim vim_version
# :syntax and :filetype (FEAT_NORMAL) with the runtime files at
# /lib/vim/runtime: a C file is detected and its syntax file loaded.
vim_syntax() {
	echo 'int main(void) { return 0; }' > /tmp/vim_syntax.c
	run-myos vim -es -c 'syntax on' -c 'filetype on' -c 'edit /tmp/vim_syntax.c' \
		-c 'if &filetype !=# "c" || get(b:, "current_syntax", "") !=# "c" | cquit | endif' \
		-c 'qa!'
	r=$?
	rm -f /tmp/vim_syntax.c
	return $r
}
t -t vim vim_syntax vim_syntax
# Colors on the console: the termcap entry of TERM=linux (getty's,
# /lib/termcap) says 8 colors, which vim needs to color its syntax
# (without Co it takes none). vim reads the termcap only on a terminal of
# its own, not in Ex mode: it runs as usual, its input and output not a
# tty (it says so and goes on), and writes &t_Co to a file.
vim_colors() {
	TERM=linux TERMCAP=/lib/termcap run-myos vim \
		-c 'call writefile([&t_Co], "/tmp/vim_colors")' -c 'qa!' \
		< /dev/null > /dev/null 2>&1
	r=$(cat /tmp/vim_colors 2>/dev/null)
	rm -f /tmp/vim_colors
	echo "t_Co: $r"
	[ "$r" = 8 ]
}
t -t vim,termcap vim_colors vim_colors
