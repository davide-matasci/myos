# git, installed as a package (heap's full mode runs its porcelain).
git_version() {
	git --version | grep -q "git version"
}
t git git_version
