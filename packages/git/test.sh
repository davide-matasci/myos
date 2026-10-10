# git, installed as an app (heap's full mode runs its porcelain in git's
# view).
git_version() {
	run-myos git --version | grep -q "git version"
}
t git git_version
