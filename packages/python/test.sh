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
t -t pkg,python python_run
