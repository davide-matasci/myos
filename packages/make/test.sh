# GNU make, installed as an app: it runs at its image path in its view,
# and nowhere else.
make_version() {
	run-myos make --version | grep -q "GNU Make" && [ ! -e /bin/custom/make ]
}
t make make_version
