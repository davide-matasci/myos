# lynx, installed as a package.
lynx_version() {
	lynx -version | grep -q "Lynx Version"
}
t lynx lynx_version
