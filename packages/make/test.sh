# GNU make, installed as a package: bound at its image path, runs.
make_version() {
	grep -q /bin/custom/make /proc/mounts && make --version | grep -q "GNU Make"
}
t make make_version
