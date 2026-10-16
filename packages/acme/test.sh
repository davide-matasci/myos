# acme, installed as an app, with the openssl and nginx apps: a certificate
# issued by the test ACME server on the host (host.sh: acme-server.py, an
# ACME CA of the boot test's own), the http-01 challenge answered by nginx
# on 5002, the port QEMU forwards to the host (src/main.rs), and the
# certificate verified against the server's CA, installed for nginx with
# `--install-cert`, served over HTTPS to curl, then renewed.
ACME_T=/tmp/acme-test
ACME_SRV=http://10.0.2.2:14000
ACME_HOME=$ACME_T/home

# acme_wait_server: the host's server answers its directory.
acme_wait_server() {
	i=0
	while [ $i -lt 60 ]; do
		curl -sf -o /dev/null $ACME_SRV/dir && return 0
		sleep 1
		i=$((i + 1))
	done
	echo "no ACME server at $ACME_SRV"
	return 1
}

# acme_stop: tell the server to exit, stop nginx.
acme_stop() {
	curl -sf -o /dev/null $ACME_SRV/stop
	run-myos nginx -c $ACME_T/nginx.conf -s quit
	i=0
	while [ -f $ACME_T/nginx.pid ] && [ $i -lt 30 ]; do
		sleep 1
		i=$((i + 1))
	done
}

# acme_nginx HTTPS: write nginx.conf (the webroot on :5002, with the
# issued certificate on 127.0.0.1:8443 when HTTPS is 1) and start or
# reload nginx.
acme_nginx() {
	{
		cat <<CONF
worker_processes 1;
pid $ACME_T/nginx.pid;
error_log $ACME_T/error.log info;
events { worker_connections 32; }
http {
	server {
		listen 5002;
		root $ACME_T/www;
	}
CONF
		[ "$1" = 1 ] && cat <<CONF
	server {
		listen 127.0.0.1:8443 ssl;
		ssl_certificate $ACME_T/cert/fullchain.pem;
		ssl_certificate_key $ACME_T/cert/key.pem;
		root $ACME_T/www;
	}
CONF
		echo "}"
	} > $ACME_T/nginx.conf
	if [ -f $ACME_T/nginx.pid ]; then
		run-myos nginx -c $ACME_T/nginx.conf -s reload
	else
		run-myos nginx -c $ACME_T/nginx.conf
	fi || { cat $ACME_T/error.log; return 1; }
}

acme_issue() {
	rm -rf $ACME_T
	mkdir -p $ACME_T/www $ACME_HOME $ACME_T/cert
	echo "hello from acme" > $ACME_T/www/index.html
	acme_nginx 0 || return 1
	echo "HOST acme serve" >&3
	acme_wait_server || { acme_stop; return 1; }
	curl -sf -o $ACME_T/ca.crt $ACME_SRV/ca.crt || { acme_stop; return 1; }
	# The order: an EC account key and an EC certificate key, both made
	# by the openssl app; the server's CA signs for a day.
	if ! run-myos acme --home $ACME_HOME --server $ACME_SRV/dir --issue \
		-d acme.test -w $ACME_T/www > $ACME_T/issue.log 2>&1; then
		cat $ACME_T/issue.log
		acme_stop
		return 1
	fi
	cert=$ACME_HOME/acme.test_ecc
	run-myos openssl verify -CAfile $ACME_T/ca.crt $cert/fullchain.cer || { acme_stop; return 1; }
	run-myos openssl x509 -in $cert/acme.test.cer -noout -text | grep -q 'DNS:acme.test' || { acme_stop; return 1; }
	first=$(run-myos openssl x509 -in $cert/acme.test.cer -noout -serial)
	echo "issued $first"
	# Installed for nginx: the files it names, the reload command run (a
	# run-myos of its own, from inside the acme app's namespace).
	if ! run-myos acme --home $ACME_HOME --install-cert -d acme.test --ecc \
		--key-file $ACME_T/cert/key.pem --fullchain-file $ACME_T/cert/fullchain.pem \
		--reloadcmd "run-myos nginx -c $ACME_T/nginx.conf -s reload && touch $ACME_T/reloaded" \
		> $ACME_T/install.log 2>&1; then
		cat $ACME_T/install.log
		acme_stop
		return 1
	fi
	[ -f $ACME_T/reloaded ] && [ -s $ACME_T/cert/key.pem ] && [ -s $ACME_T/cert/fullchain.pem ] || { acme_stop; return 1; }
	# Served: nginx over LibreSSL with the certificate, curl verifying it
	# against the test CA for the name it was issued to.
	acme_nginx 1 || { acme_stop; return 1; }
	out=$(curl -s --cacert $ACME_T/ca.crt --resolve acme.test:8443:127.0.0.1 https://acme.test:8443/index.html)
	echo "https: $out"
	[ "$out" = "hello from acme" ] || { curl -v --cacert $ACME_T/ca.crt --resolve acme.test:8443:127.0.0.1 https://acme.test:8443/ 2>&1 | tail -20; acme_stop; return 1; }
	# Renewed (forced: the certificate is a day old at most): a new
	# certificate, the installed files and nginx updated.
	if ! run-myos acme --home $ACME_HOME --renew -d acme.test --ecc --force > $ACME_T/renew.log 2>&1; then
		cat $ACME_T/renew.log
		acme_stop
		return 1
	fi
	second=$(run-myos openssl x509 -in $cert/acme.test.cer -noout -serial)
	echo "renewed $second"
	[ "$first" != "$second" ] || { acme_stop; return 1; }
	installed=$(run-myos openssl x509 -in $ACME_T/cert/fullchain.pem -noout -serial)
	[ "$installed" = "$second" ] || { echo "installed $installed"; acme_stop; return 1; }
	acme_stop
}
t -t netfs,nginx,openssl acme_issue acme_issue
