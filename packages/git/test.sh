# git, installed as an app (heap's full mode runs its porcelain in git's
# view).
git_version() {
	run-myos git --version | grep -q "git version"
}
t git git_version

# A clone over SSH (README.md) from this system itself: dbclient as `ssh`
# to the dropbear the boot started, with a key made here, the remote side
# `git upload-pack` in git's app.
git_clone_ssh() {
	d=/tmp/git-ssh
	rm -rf $d && mkdir -p $d/src || return 1
	echo "over ssh" > $d/src/f.txt
	run-myos git -C $d/src init -q && run-myos git -C $d/src add f.txt \
		&& run-myos git -C $d/src -c user.email=t@myos -c user.name=t commit -q -m ssh || return 1
	dropbearkey -t ed25519 -f $d/key > /dev/null 2>&1 || return 1
	dropbearkey -y -f $d/key | grep "^ssh-ed25519" > /etc/dropbear/authorized_keys || return 1
	GIT_SSH_COMMAND="ssh -y -i $d/key" run-myos git clone -q \
		-u "run-myos -r $MYOS_APPS git upload-pack" ssh://root@127.0.0.1:2222$d/src $d/dst
	rc=$?
	rm /etc/dropbear/authorized_keys
	[ $rc = 0 ] && [ "$(cat $d/dst/f.txt)" = "over ssh" ]
}
t -t netfs,dropbear git_clone_ssh git_clone_ssh
