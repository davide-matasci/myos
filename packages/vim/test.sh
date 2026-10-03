# vim, installed as a package: the binary and its vimrc at their image paths.
vim_version() {
	[ -f /lib/vim/vimrc ] && vim --version | head -n 1 | grep -q "VIM"
}
t vim vim_version
