# vim, installed as an app: run-myos runs it with its vimrc at its image
# path, in its view only.
vim_version() {
	run-myos vim --version | head -n 1 | grep -q "VIM" || return 1
	run-myos vim:/bin/sh -c '[ -f /lib/vim/vimrc ]' && [ ! -e /lib/vim ]
}
t vim vim_version
