# lynx, installed as an app.
lynx_version() {
	run-myos lynx -version | grep -q "Lynx Version"
}
t lynx lynx_version
