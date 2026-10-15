# nginx, installed as an app: its default configuration checks, then a
# server of the test's own on 127.0.0.1 (master and a worker) answers a
# page, a 404, a directory listing, a regex location, a rewrite and gzip,
# logs the requests, and stops on `nginx -s quit`; then HTTPS (the
# certificates are test/mkcerts.py's): a page over TLS 1.2, the server of
# the name the client sends (SNI, an RSA certificate), a verified client
# certificate, a session resumed from the shared cache, plain HTTP to the
# HTTPS port refused, and nginx proxying to itself over HTTPS with the
# upstream's certificate and name verified.
NGINX_T=/tmp/nginx-test
NGINX_CERTS=${MYOS_APPS:-/tmp/apps}/nginx/lib/myos-tests/nginx

# nginx_stop CONF: `nginx -s quit`, then wait for the pid file to go.
nginx_stop() {
	run-myos nginx -c $1 -s quit || return 1
	i=0
	while [ -f $NGINX_T/nginx.pid ] && [ $i -lt 30 ]; do
		sleep 1
		i=$((i + 1))
	done
	[ ! -f $NGINX_T/nginx.pid ] || { echo "still running"; return 1; }
}

nginx_conf() {
	run-myos nginx -t
}

nginx_serve() {
	rm -rf $NGINX_T
	mkdir -p $NGINX_T/www/dir
	echo "hello from nginx" > $NGINX_T/www/index.html
	echo "a file" > $NGINX_T/www/dir/a.txt
	i=0
	while [ $i -lt 200 ]; do
		echo "line $i of a text that compresses well"
		i=$((i + 1))
	done > $NGINX_T/www/big.txt
	cat > $NGINX_T/nginx.conf <<CONF
worker_processes 1;
pid $NGINX_T/nginx.pid;
error_log $NGINX_T/error.log info;
events { worker_connections 32; }
http {
	include /lib/nginx/mime.types;
	access_log $NGINX_T/access.log;
	gzip on;
	gzip_types text/plain;
	gzip_min_length 100;
	server {
		listen 127.0.0.1:8080;
		root $NGINX_T/www;
		location /dir/ { autoindex on; }
		location ~ ^/re/([a-z]+)\$ { return 200 "regex \$1\n"; }
		rewrite ^/old\$ /index.html last;
	}
}
CONF
	run-myos nginx -c $NGINX_T/nginx.conf || { cat $NGINX_T/error.log; return 1; }
	ok=0
	curl -fsS --max-time 30 http://127.0.0.1:8080/ | grep -qx "hello from nginx" || { echo "index"; ok=1; }
	code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 http://127.0.0.1:8080/nope)
	[ "$code" = 404 ] || { echo "404: $code"; ok=1; }
	curl -fsS --max-time 30 http://127.0.0.1:8080/dir/ | grep -q 'href="a.txt"' || { echo "autoindex"; ok=1; }
	curl -fsS --max-time 30 http://127.0.0.1:8080/re/abc | grep -qx "regex abc" || { echo "regex"; ok=1; }
	curl -fsS --max-time 30 http://127.0.0.1:8080/old | grep -qx "hello from nginx" || { echo "rewrite"; ok=1; }
	curl -fsS --max-time 30 -H 'Accept-Encoding: gzip' -D $NGINX_T/h -o $NGINX_T/big.gz http://127.0.0.1:8080/big.txt || { echo "gzip fetch"; ok=1; }
	grep -qi '^content-encoding: gzip' $NGINX_T/h || { echo "gzip header"; cat $NGINX_T/h; ok=1; }
	# The image has no gunzip: a gzip stream, smaller than the file.
	printf '\037\213' > $NGINX_T/magic
	dd if=$NGINX_T/big.gz bs=2 count=1 2>/dev/null | cmp -s - $NGINX_T/magic || { echo "gzip magic"; ok=1; }
	[ "$(wc -c < $NGINX_T/big.gz)" -lt "$(wc -c < $NGINX_T/www/big.txt)" ] || { echo "gzip size"; ok=1; }
	[ "$(grep -c '"GET ' $NGINX_T/access.log)" -ge 6 ] || { echo "access log"; cat $NGINX_T/access.log; ok=1; }
	nginx_stop $NGINX_T/nginx.conf || ok=1
	[ $ok = 0 ] || cat $NGINX_T/error.log
	return $ok
}

nginx_https() {
	c=$NGINX_CERTS
	rm -rf $NGINX_T
	mkdir -p $NGINX_T
	cat > $NGINX_T/nginx.conf <<CONF
worker_processes 1;
pid $NGINX_T/nginx.pid;
error_log $NGINX_T/error.log info;
events { worker_connections 32; }
http {
	access_log off;
	ssl_session_cache shared:SSL:1m;
	server {
		listen 127.0.0.1:8443 ssl;
		server_name localhost;
		ssl_certificate $c/localhost.crt;
		ssl_certificate_key $c/localhost.key;
		ssl_client_certificate $c/client.crt;
		ssl_verify_client optional;
		location /reused { return 200 "\$ssl_session_reused\n"; }
		location / {
			return 200 "\$ssl_protocol \$ssl_server_name \$ssl_client_verify \$ssl_client_s_dn\n";
		}
	}
	server {
		listen 127.0.0.1:8443 ssl;
		server_name other.test;
		ssl_certificate $c/other.crt;
		ssl_certificate_key $c/other.key;
		location / { return 200 "other \$ssl_server_name\n"; }
	}
	server {
		listen 127.0.0.1:8081;
		location / {
			proxy_pass https://127.0.0.1:8443;
			proxy_ssl_verify on;
			proxy_ssl_trusted_certificate $c/localhost.crt;
			proxy_ssl_name localhost;
			proxy_ssl_server_name on;
			# The name sent (SNI) and the Host must agree (421 otherwise).
			proxy_set_header Host localhost;
		}
	}
}
CONF
	run-myos nginx -c $NGINX_T/nginx.conf || { cat $NGINX_T/error.log; return 1; }
	ok=0
	out=$(curl -fsS --max-time 30 --cacert $c/localhost.crt https://localhost:8443/)
	[ "$out" = "TLSv1.2 localhost NONE " ] || { echo "tls: $out"; ok=1; }
	out=$(curl -fsS --max-time 30 --cacert $c/other.crt --resolve other.test:8443:127.0.0.1 https://other.test:8443/)
	[ "$out" = "other other.test" ] || { echo "sni: $out"; ok=1; }
	out=$(curl -fsS --max-time 30 --cacert $c/localhost.crt --cert $c/client.crt --key $c/client.key https://localhost:8443/)
	[ "$out" = "TLSv1.2 localhost SUCCESS CN=client,O=myos test" ] || { echo "client certificate: $out"; ok=1; }
	# Two connections of one curl: the second resumes the first's session
	# (nginx's shared cache).
	out=$(curl -fsS --max-time 30 --cacert $c/localhost.crt -H 'Connection: close' \
		https://localhost:8443/reused https://localhost:8443/reused | tr '\n' ' ')
	[ "$out" = ". r " ] || { echo "session reuse: $out"; ok=1; }
	code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 http://127.0.0.1:8443/)
	[ "$code" = 400 ] || { echo "plain http to https: $code"; ok=1; }
	out=$(curl -fsS --max-time 30 http://127.0.0.1:8081/)
	[ "$out" = "TLSv1.2 localhost NONE " ] || { echo "proxy over https: $out"; ok=1; }
	nginx_stop $NGINX_T/nginx.conf || ok=1
	[ $ok = 0 ] || cat $NGINX_T/error.log
	return $ok
}

t -t netfs nginx_conf nginx_conf
t -t netfs nginx_serve nginx_serve
t -t netfs nginx_https nginx_https
