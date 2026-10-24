# python, installed as an app: the interpreter's version, then the library
# through it: the platform, a thread, a subprocess (the interpreter again),
# a socket pair, zlib, json, a hash, floating point and a regexp, in one
# line compared whole.
python_run() {
	run-myos python -c 'import sys; print(sys.version)' | grep '^3\.13\.' || return 1
	out=$(run-myos python -c '
import sys, json, zlib, threading, subprocess, socket, re, math, hashlib
assert sys.platform == "myos", sys.platform
r = []
t = threading.Thread(target=lambda: r.append(sum(range(100))))
t.start()
t.join()
p = subprocess.run([sys.executable, "-c", "print(6 * 7)"], capture_output=True, text=True)
a, b = socket.socketpair()
a.sendall(b"ping")
m = b.recv(4)
d = zlib.decompress(zlib.compress(b"myos" * 100))
print(r[0], p.stdout.strip(), m.decode(), len(d), json.dumps({"k": [1, 2.5]}),
      hashlib.sha256(b"x").hexdigest()[:8], math.sqrt(2) > 1.41, re.sub(r"o", "0", "foo"))
') || return 1
	echo "$out"
	[ "$out" = '4950 42 ping 400 {"k": [1, 2.5]} 2d711642 True f00' ]
}
t -t pkg,python python_run python_run

# The modules over the ports' libraries: _ssl and _hashlib over LibreSSL
# (its default CA file the image's /lib/cacert.pem, a PBKDF2 through it),
# sqlite3 on a file (a second connection held off by the first's write
# lock), curses and readline (ncurses, libedit) imported, and pip.
python_modules() {
	rm -f /tmp/python-test.db
	out=$(run-myos python -c '
import ssl, hashlib, _hashlib, sqlite3, curses, _curses_panel, readline
ctx = ssl.create_default_context()
a = sqlite3.connect("/tmp/python-test.db", isolation_level=None)
a.execute("create table t (k integer)")
a.execute("begin immediate")
a.execute("insert into t values (42)")
b = sqlite3.connect("/tmp/python-test.db", timeout=0)
try:
    b.execute("begin immediate")
    locked = False
except sqlite3.OperationalError:
    locked = True
a.execute("commit")
print(ssl.OPENSSL_VERSION.split()[0], ssl.get_default_verify_paths().cafile,
      ctx.cert_store_stats()["x509_ca"] > 100,
      hashlib.pbkdf2_hmac("sha256", b"myos", b"salt", 1000).hex()[:16],
      locked, b.execute("select k from t").fetchone()[0],
      curses.has_key is not None, readline.backend)
') || return 1
	rm -f /tmp/python-test.db
	echo "$out"
	[ "$out" = 'LibreSSL /lib/cacert.pem True 9edb222f0b6813f3 True 42 True editline' ] || return 1
	run-myos python -m pip --version | grep '^pip [0-9]'
}
t -t pkg,python python_modules python_modules

# urllib over _ssl: an HTTPS GET of example.com, the certificate checked
# against the image's bundle.
python_https() {
	out=$(run-myos python -c '
import urllib.request
with urllib.request.urlopen("https://example.com/", timeout=60) as r:
    print(r.status, b"Example Domain" in r.read())
') || return 1
	echo "$out"
	[ "$out" = '200 True' ]
}
t -t net,pkg,python python_https python_https
