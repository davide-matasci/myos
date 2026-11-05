/* The program WebKit's build links (WEBKIT_MYOS_PROGRAM): only so that its
 * link names every object and archive of WebKit's, which build.sh gathers
 * into libwebkit2gtk-4.1.a (install_webkit). The archive leaves this file
 * out; the programs are linked from the archive by their own packages
 * (webkitgtk-smoke, surf), so that what they link and WebKit never
 * compiles against (GIO's TLS backend, LibreSSL) can change without
 * WebKit's build (issue #664). */
int main(int argc, char **argv) {
    (void)argc;
    (void)argv;
    return 0;
}
