# nginx, installed as an app: its default configuration checks, then a
# server of the test's own on 127.0.0.1 (master and a worker) answers a
# page, a 404, a directory listing, a regex location, a rewrite and gzip,
# logs the requests, and stops on `nginx -s quit`.
NGINX_T=/tmp/nginx-test

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
	[ "$(dd if=$NGINX_T/big.gz bs=2 count=1 2>/dev/null | od -A n -t x1 | tr -d ' \n')" = 1f8b ] || { echo "gzip magic"; ok=1; }
	[ "$(wc -c < $NGINX_T/big.gz)" -lt "$(wc -c < $NGINX_T/www/big.txt)" ] || { echo "gzip size"; ok=1; }
	[ "$(grep -c '"GET ' $NGINX_T/access.log)" -ge 6 ] || { echo "access log"; cat $NGINX_T/access.log; ok=1; }
	run-myos nginx -c $NGINX_T/nginx.conf -s quit || ok=1
	i=0
	while [ -f $NGINX_T/nginx.pid ] && [ $i -lt 30 ]; do
		sleep 1
		i=$((i + 1))
	done
	[ ! -f $NGINX_T/nginx.pid ] || { echo "still running"; ok=1; }
	[ $ok = 0 ] || cat $NGINX_T/error.log
	return $ok
}

t nginx_conf nginx_conf
t nginx_serve nginx_serve
