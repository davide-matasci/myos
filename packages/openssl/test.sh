# openssl, installed as an app: its version, an EC key and a self-signed
# certificate made and read back, a digest and base64 against known
# values (what acme asks of it).
OPENSSL_T=/tmp/openssl-test

openssl_run() {
	rm -rf $OPENSSL_T
	mkdir -p $OPENSSL_T
	run-myos openssl version | grep '^LibreSSL 4' || return 1
	run-myos openssl ecparam -name prime256v1 -genkey -noout -out $OPENSSL_T/key.pem || return 1
	run-myos openssl req -new -x509 -key $OPENSSL_T/key.pem -sha256 -days 2 \
		-subj /CN=openssl.test -out $OPENSSL_T/cert.pem || return 1
	run-myos openssl x509 -in $OPENSSL_T/cert.pem -noout -subject | grep -q 'CN *= *openssl.test' || return 1
	run-myos openssl verify -CAfile $OPENSSL_T/cert.pem $OPENSSL_T/cert.pem | grep -q ': OK' || return 1
	d=$(printf 'abc' | run-myos openssl dgst -sha256 | cut -d ' ' -f 2)
	echo "sha256 $d"
	[ "$d" = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad ] || return 1
	b=$(printf 'myos' | run-myos openssl base64)
	echo "base64 $b"
	[ "$b" = bXlvcw== ] || return 1
	[ "$(echo bXlvcw== | run-myos openssl base64 -d)" = myos ]
}
t -t crypto openssl openssl_run
