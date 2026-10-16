#!/usr/bin/env python3
"""The acme package's test ACME server (RFC 8555), the host's side of its
boot test (host.sh, docs/testing.md): what a client has to get right to be
issued a certificate, with nothing but Python's standard library and the
host's openssl command.

    acme-server.py --listen 0.0.0.0:14000 --url http://10.0.2.2:14000 \\
        --http01 127.0.0.1:5002 [--timeout SECONDS]

It serves the directory at URL/dir and the ACME resources below URL/acme/,
checks every request's JWS (a fresh nonce of this server's, the request's
URL, the signature with the account key: ES256 and ES384 on an EC key,
RS256 on an RSA one, verified by openssl against the key rebuilt from the
JWK), validates an http-01 challenge by fetching the token from
HTTP01HOST (the guest's web server, through QEMU's port forward: the
identifier's name is not resolved) and checking the key authorization,
and signs the CSR of a finalized order with a CA made at startup (an EC
key, a certificate for a day) once its names are the order's. The chain
comes back as application/pem-certificate-chain; the CA certificate is at
URL/ca.crt for the client's side to verify against. GET URL/stop ends the
server (exit 0); so does the timeout (exit 1), so a failed guest test
cannot leave it behind. Every exchange is logged on stderr.

Not an ACME CA: no DNS, no rate limits, no revocation, no key change, no
account deactivation (those endpoints answer with a malformed error), no
pre-authorization, no wildcard names, a 2-second retryAfter nowhere.
"""
import argparse
import base64
import hashlib
import http.server
import json
import os
import secrets
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request


def b64u(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def b64u_decode(text):
    text = text + "=" * (-len(text) % 4)
    return base64.urlsafe_b64decode(text)


def log(msg):
    sys.stderr.write(time.strftime("%H:%M:%S ") + msg + "\n")
    sys.stderr.flush()


# --- DER, enough to rebuild a public key from a JWK (RFC 7518) -----------


def der(tag, body):
    n = len(body)
    if n < 0x80:
        length = bytes([n])
    else:
        enc = n.to_bytes((n.bit_length() + 7) // 8, "big")
        length = bytes([0x80 | len(enc)]) + enc
    return bytes([tag]) + length + body


def der_uint(data):
    data = data.lstrip(b"\x00") or b"\x00"
    if data[0] & 0x80:
        data = b"\x00" + data
    return der(0x02, data)


# SubjectPublicKeyInfo of an EC key, up to the uncompressed point: the
# curve's AlgorithmIdentifier and the BIT STRING header.
EC_SPKI_PREFIX = {
    "P-256": bytes.fromhex("3059301306072a8648ce3d020106082a8648ce3d030107034200"),
    "P-384": bytes.fromhex("3076301006072a8648ce3d020106052b81040022036200"),
}
EC_COORD_LEN = {"P-256": 32, "P-384": 48}
EC_DIGEST = {"P-256": "sha256", "P-384": "sha384"}
RSA_ALGID = bytes.fromhex("300d06092a864886f70d0101010500")


def jwk_to_pem(jwk):
    """The PEM public key of a JWK, and the digest its JWS signatures use."""
    kty = jwk.get("kty")
    if kty == "EC":
        crv = jwk.get("crv")
        if crv not in EC_SPKI_PREFIX:
            raise ValueError(f"unsupported curve {crv!r}")
        x = b64u_decode(jwk["x"])
        y = b64u_decode(jwk["y"])
        n = EC_COORD_LEN[crv]
        if len(x) != n or len(y) != n:
            raise ValueError("EC coordinate length")
        spki = EC_SPKI_PREFIX[crv] + b"\x04" + x + y
        digest = EC_DIGEST[crv]
    elif kty == "RSA":
        n = b64u_decode(jwk["n"])
        e = b64u_decode(jwk["e"])
        pub = der(0x30, der_uint(n) + der_uint(e))
        spki = der(0x30, RSA_ALGID + der(0x03, b"\x00" + pub))
        digest = "sha256"
    else:
        raise ValueError(f"unsupported key type {kty!r}")
    body = base64.encodebytes(spki).decode()
    return f"-----BEGIN PUBLIC KEY-----\n{body}-----END PUBLIC KEY-----\n", digest


def jwk_thumbprint(jwk):
    """RFC 7638: SHA-256 of the required members in lexicographic order."""
    if jwk["kty"] == "EC":
        members = {k: jwk[k] for k in ("crv", "kty", "x", "y")}
    else:
        members = {k: jwk[k] for k in ("e", "kty", "n")}
    canonical = json.dumps(members, separators=(",", ":"), sort_keys=True)
    return b64u(hashlib.sha256(canonical.encode()).digest())


def openssl(*args, stdin=None):
    """Run the host's openssl; its stdout, or an OSError with its stderr."""
    proc = subprocess.run(
        ["openssl", *args], input=stdin, capture_output=True, check=False
    )
    if proc.returncode != 0:
        raise OSError(f"openssl {' '.join(args)}: {proc.stderr.decode(errors='replace').strip()}")
    return proc.stdout


def verify_jws(jwk, alg, signing_input, signature):
    """The JWS signature over signing_input, with the account key."""
    pem, digest = jwk_to_pem(jwk)
    expected = {"sha256": ("ES256", "RS256"), "sha384": ("ES384",)}[digest]
    if alg not in expected or (jwk["kty"] == "RSA") != (alg == "RS256"):
        raise ValueError(f"alg {alg} does not fit the key")
    if jwk["kty"] == "EC":
        n = len(signature) // 2
        if len(signature) != 2 * EC_COORD_LEN[jwk["crv"]]:
            raise ValueError("ECDSA signature length")
        signature = der(0x30, der_uint(signature[:n]) + der_uint(signature[n:]))
    with tempfile.TemporaryDirectory() as d:
        key = os.path.join(d, "key.pem")
        sig = os.path.join(d, "sig")
        with open(key, "w") as f:
            f.write(pem)
        with open(sig, "wb") as f:
            f.write(signature)
        out = openssl("dgst", f"-{digest}", "-verify", key, "-signature", sig, stdin=signing_input)
    if b"Verified OK" not in out:
        raise ValueError("signature does not verify")


# --- the CA ---------------------------------------------------------------


class CA:
    def __init__(self, workdir):
        self.dir = workdir
        self.key = os.path.join(workdir, "ca.key")
        self.cert = os.path.join(workdir, "ca.crt")
        self.serial = secrets.randbits(63)
        openssl("ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", self.key)
        openssl(
            "req", "-x509", "-new", "-key", self.key, "-sha256", "-days", "2",
            "-subj", "/CN=myos acme test CA", "-addext", "basicConstraints=critical,CA:TRUE",
            "-addext", "keyUsage=critical,keyCertSign,cRLSign", "-out", self.cert,
        )
        with open(self.cert) as f:
            self.cert_pem = f.read()

    def csr_names(self, csr_der):
        """The DNS names a CSR asks for (its subjectAltName and its CN)."""
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "csr.der")
            with open(path, "wb") as f:
                f.write(csr_der)
            text = openssl("req", "-inform", "DER", "-in", path, "-noout", "-verify", "-text").decode()
        names = set()
        for line in text.splitlines():
            line = line.strip()
            if line.startswith("DNS:"):
                names.update(p.strip()[4:] for p in line.split(",") if p.strip().startswith("DNS:"))
            elif line.startswith("Subject:"):
                for part in line[8:].split(","):
                    k, _, v = part.strip().partition("=")
                    if k.strip() in ("CN", "commonName"):
                        names.add(v.strip())
        return names

    def sign(self, csr_der, names):
        """A certificate for the CSR with exactly these names, a day long."""
        self.serial += 1
        with tempfile.TemporaryDirectory() as d:
            csr = os.path.join(d, "csr.der")
            ext = os.path.join(d, "ext.cnf")
            with open(csr, "wb") as f:
                f.write(csr_der)
            with open(ext, "w") as f:
                f.write("basicConstraints=CA:FALSE\n")
                f.write("keyUsage=critical,digitalSignature\n")
                f.write("extendedKeyUsage=serverAuth\n")
                f.write("subjectAltName=" + ",".join(f"DNS:{n}" for n in sorted(names)) + "\n")
            return openssl(
                "x509", "-req", "-inform", "DER", "-in", csr, "-CA", self.cert, "-CAkey", self.key,
                "-set_serial", str(self.serial), "-days", "1", "-sha256", "-extfile", ext,
            ).decode()


# --- the ACME state --------------------------------------------------------


class State:
    def __init__(self, base, http01, ca):
        self.base = base.rstrip("/")
        self.http01 = http01
        self.ca = ca
        self.lock = threading.Lock()
        self.nonces = set()
        self.accounts = {}  # url -> jwk
        self.orders = {}  # id -> order dict
        self.authzs = {}  # id -> authz dict (with "order", "account", "token")
        self.certs = {}  # id -> pem chain
        self.next_id = 1
        self.issued = 0
        self.stop = threading.Event()

    def url(self, *parts):
        return self.base + "/" + "/".join(parts)

    def new_nonce(self):
        nonce = b64u(secrets.token_bytes(16))
        self.nonces.add(nonce)
        return nonce

    def take_nonce(self, nonce):
        if nonce in self.nonces:
            self.nonces.remove(nonce)
            return True
        return False

    def fresh_id(self):
        n = self.next_id
        self.next_id += 1
        return str(n)

    def directory(self):
        return {
            "newNonce": self.url("acme", "new-nonce"),
            "newAccount": self.url("acme", "new-acct"),
            "newOrder": self.url("acme", "new-order"),
            "revokeCert": self.url("acme", "revoke-cert"),
            "keyChange": self.url("acme", "key-change"),
            "meta": {"termsOfService": self.url("tos"), "website": "https://github.com/davide-matasci/myos"},
        }

    def validate_http01(self, authz):
        """Fetch the token from the client's web server and check the key
        authorization: TOKEN.THUMBPRINT of its account key."""
        token = authz["token"]
        name = authz["identifier"]["value"]
        want = token + "." + jwk_thumbprint(self.accounts[authz["account"]])
        url = f"http://{self.http01}/.well-known/acme-challenge/{token}"
        req = urllib.request.Request(url, headers={"Host": name})
        last = ""
        for _ in range(3):
            try:
                with urllib.request.urlopen(req, timeout=10) as resp:
                    body = resp.read(4096).decode(errors="replace").strip()
                if body == want:
                    return None
                last = f"key authorization mismatch: got {body[:80]!r}"
            except Exception as e:  # noqa: BLE001 - reported to the client
                last = f"{url}: {e}"
            time.sleep(1)
        return last


# --- HTTP ------------------------------------------------------------------


class Problem(Exception):
    def __init__(self, status, kind, detail):
        super().__init__(detail)
        self.status = status
        self.kind = kind
        self.detail = detail


class Handler(http.server.BaseHTTPRequestHandler):
    server_version = "myos-acme-test/1"
    protocol_version = "HTTP/1.1"
    state: State = None  # set by main

    def log_message(self, fmt, *args):
        log(f"{self.client_address[0]} {fmt % args}")

    # Replies. Every ACME reply carries a fresh nonce and the index link.
    def reply(self, status, body=None, content_type="application/json", headers=()):
        self.send_response(status)
        self.send_header("Replay-Nonce", self.state.new_nonce())
        self.send_header("Cache-Control", "no-store")
        self.send_header("Link", f'<{self.state.url("dir")}>;rel="index"')
        for k, v in headers:
            self.send_header(k, v)
        if body is None:
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if isinstance(body, (dict, list)):
            body = json.dumps(body).encode()
        elif isinstance(body, str):
            body = body.encode()
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def problem(self, p):
        log(f"-> {p.status} {p.kind}: {p.detail}")
        self.reply(
            p.status,
            {"type": "urn:ietf:params:acme:error:" + p.kind, "detail": p.detail, "status": p.status},
            content_type="application/problem+json",
        )

    def do_HEAD(self):
        self.do_GET()

    def do_GET(self):
        path = self.path.split("?")[0]
        st = self.state
        if path == "/dir":
            self.reply(200, st.directory())
        elif path == "/acme/new-nonce":
            self.reply(200 if self.command == "HEAD" else 204)
        elif path == "/tos":
            self.reply(200, "The myos acme test server's terms: none.\n", "text/plain")
        elif path == "/ca.crt":
            self.reply(200, st.ca.cert_pem, "application/x-pem-file")
        elif path == "/stop":
            self.reply(200, "stopping\n", "text/plain")
            log(f"stop requested after {st.issued} certificate(s)")
            st.stop.set()
        else:
            self.problem(Problem(405, "malformed", "ACME resources take POST (a POST-as-GET)"))

    def do_POST(self):
        path = self.path.split("?")[0]
        st = self.state
        try:
            length = int(self.headers.get("Content-Length", "0"))
            raw = self.rfile.read(length)
            with st.lock:
                self.handle_post(path, raw)
        except Problem as p:
            self.problem(p)
        except Exception as e:  # noqa: BLE001 - the client sees every failure
            log(f"error on {path}: {e!r}")
            self.problem(Problem(500, "serverInternal", repr(e)))

    def parse_jws(self, path, raw, want_jwk):
        """The verified JWS of a request: (protected header, payload bytes,
        account url or None, jwk)."""
        try:
            jws = json.loads(raw)
            protected = json.loads(b64u_decode(jws["protected"]))
            payload = b64u_decode(jws["payload"])
            signature = b64u_decode(jws["signature"])
        except (KeyError, ValueError, TypeError) as e:
            raise Problem(400, "malformed", f"not a JWS: {e}") from e
        nonce = protected.get("nonce")
        if not nonce or not self.state.take_nonce(nonce):
            raise Problem(400, "badNonce", "nonce unknown or used")
        if protected.get("url") != self.state.url(*path.strip("/").split("/")):
            raise Problem(400, "unauthorized", f"url {protected.get('url')!r} is not this request's")
        if "jwk" in protected and "kid" in protected:
            raise Problem(400, "malformed", "both jwk and kid")
        if want_jwk:
            if "jwk" not in protected:
                raise Problem(400, "malformed", "new-account takes a jwk")
            jwk, account = protected["jwk"], None
        else:
            kid = protected.get("kid")
            if kid not in self.state.accounts:
                raise Problem(400, "accountDoesNotExist", f"kid {kid!r}")
            jwk, account = self.state.accounts[kid], kid
        try:
            verify_jws(jwk, protected.get("alg"), (jws["protected"] + "." + jws["payload"]).encode(), signature)
        except (ValueError, KeyError, OSError) as e:
            raise Problem(400, "badSignatureAlgorithm" if "alg" in str(e) else "malformed", str(e)) from e
        return protected, payload, account, jwk

    def handle_post(self, path, raw):
        st = self.state
        parts = path.strip("/").split("/")
        if parts[:2] == ["acme", "new-acct"]:
            protected, payload, _, jwk = self.parse_jws(path, raw, want_jwk=True)
            body = json.loads(payload or b"{}")
            thumb = jwk_thumbprint(jwk)
            existing = [u for u, k in st.accounts.items() if jwk_thumbprint(k) == thumb]
            if existing:
                url = existing[0]
                status = 200
            elif body.get("onlyReturnExisting"):
                raise Problem(400, "accountDoesNotExist", "no account with this key")
            else:
                url = st.url("acme", "acct", st.fresh_id())
                st.accounts[url] = jwk
                status = 201
                log(f"account {url} ({jwk.get('kty')} {jwk.get('crv', '')}, contact {body.get('contact')})")
            self.reply(status, {"status": "valid", "contact": body.get("contact", []), "orders": url + "/orders"},
                       headers=[("Location", url)])
            return

        protected, payload, account, _ = self.parse_jws(path, raw, want_jwk=False)
        body = json.loads(payload) if payload else None  # None: POST-as-GET

        if parts[:2] == ["acme", "new-order"]:
            idents = body.get("identifiers") if body else None
            if not idents or any(i.get("type") != "dns" or not i.get("value") for i in idents):
                raise Problem(400, "malformed", "identifiers: dns names only")
            if any("*" in i["value"] for i in idents):
                raise Problem(400, "rejectedIdentifier", "no wildcard names here")
            oid = st.fresh_id()
            authz_ids = []
            for ident in idents:
                aid = st.fresh_id()
                st.authzs[aid] = {
                    "identifier": ident, "status": "pending", "order": oid, "account": account,
                    "token": b64u(secrets.token_bytes(32)), "error": None,
                }
                authz_ids.append(aid)
            st.orders[oid] = {"status": "pending", "identifiers": idents, "authzs": authz_ids,
                              "account": account, "cert": None, "error": None}
            log(f"order {oid} for {[i['value'] for i in idents]}")
            self.reply(201, self.order_json(oid), headers=[("Location", st.url("acme", "order", oid))])
            return

        if parts[:2] == ["acme", "order"] and len(parts) == 3:
            oid = self.owned(st.orders, parts[2], account, "order")
            self.reply(200, self.order_json(oid))
            return

        if parts[:2] == ["acme", "authz"] and len(parts) == 3:
            aid = self.owned(st.authzs, parts[2], account, "authorization")
            self.reply(200, self.authz_json(aid))
            return

        if parts[:2] == ["acme", "chall"] and len(parts) == 3:
            aid = self.owned(st.authzs, parts[2], account, "challenge")
            authz = st.authzs[aid]
            if body is not None and authz["status"] == "pending":
                # The client says the token is in place: validate now.
                error = st.validate_http01(authz)
                order = st.orders[authz["order"]]
                if error is None:
                    authz["status"] = "valid"
                    if all(st.authzs[a]["status"] == "valid" for a in order["authzs"]):
                        order["status"] = "ready"
                    log(f"authz {aid} ({authz['identifier']['value']}) valid")
                else:
                    authz["status"] = "invalid"
                    authz["error"] = error
                    order["status"] = "invalid"
                    log(f"authz {aid} ({authz['identifier']['value']}) invalid: {error}")
            self.reply(200, self.challenge_json(aid),
                       headers=[("Link", f'<{st.url("acme", "authz", aid)}>;rel="up"')])
            return

        if parts[:2] == ["acme", "finalize"] and len(parts) == 3:
            oid = self.owned(st.orders, parts[2], account, "order")
            order = st.orders[oid]
            if order["status"] != "ready":
                raise Problem(403, "orderNotReady", f"order is {order['status']}")
            if body is None or "csr" not in body:
                raise Problem(400, "malformed", "finalize takes a csr")
            csr = b64u_decode(body["csr"])
            try:
                names = st.ca.csr_names(csr)
            except OSError as e:
                raise Problem(400, "badCSR", str(e)) from e
            want = {i["value"] for i in order["identifiers"]}
            if names != want:
                raise Problem(400, "badCSR", f"CSR names {sorted(names)} are not the order's {sorted(want)}")
            cid = st.fresh_id()
            st.certs[cid] = st.ca.sign(csr, want) + st.ca.cert_pem
            order["cert"] = cid
            order["status"] = "valid"
            st.issued += 1
            log(f"order {oid} issued certificate {cid} for {sorted(want)}")
            self.reply(200, self.order_json(oid), headers=[("Location", st.url("acme", "order", oid))])
            return

        if parts[:2] == ["acme", "cert"] and len(parts) == 3:
            cid = parts[2]
            if cid not in st.certs:
                raise Problem(404, "malformed", "no such certificate")
            self.reply(200, st.certs[cid], "application/pem-certificate-chain")
            return

        if parts[:2] == ["acme", "acct"] and len(parts) == 3:
            url = st.url("acme", "acct", parts[2])
            if url != account:
                raise Problem(401, "unauthorized", "not your account")
            self.reply(200, {"status": "valid", "orders": url + "/orders"})
            return

        raise Problem(400, "malformed", f"no such resource: {path}")

    def owned(self, table, key, account, what):
        if key not in table or table[key]["account"] != account:
            raise Problem(404, "malformed", f"no such {what} of yours")
        return key

    def order_json(self, oid):
        st = self.state
        order = st.orders[oid]
        out = {
            "status": order["status"],
            "expires": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + 3600)),
            "identifiers": order["identifiers"],
            "authorizations": [st.url("acme", "authz", a) for a in order["authzs"]],
            "finalize": st.url("acme", "finalize", oid),
        }
        if order["cert"]:
            out["certificate"] = st.url("acme", "cert", order["cert"])
        return out

    def authz_json(self, aid):
        st = self.state
        authz = st.authzs[aid]
        return {
            "status": authz["status"],
            "expires": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + 3600)),
            "identifier": authz["identifier"],
            "challenges": [self.challenge_json(aid)],
        }

    def challenge_json(self, aid):
        st = self.state
        authz = st.authzs[aid]
        out = {"type": "http-01", "url": st.url("acme", "chall", aid), "status": authz["status"],
               "token": authz["token"]}
        if authz["status"] == "valid":
            out["validated"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        if authz["error"]:
            out["error"] = {"type": "urn:ietf:params:acme:error:unauthorized", "detail": authz["error"]}
        return out


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--listen", default="0.0.0.0:14000", help="address to listen on")
    ap.add_argument("--url", required=True, help="the base URL clients reach this server at")
    ap.add_argument("--http01", required=True, help="host:port the http-01 tokens are fetched from")
    ap.add_argument("--timeout", type=int, default=600, help="seconds to serve at most (exit 1)")
    args = ap.parse_args()
    host, _, port = args.listen.rpartition(":")
    with tempfile.TemporaryDirectory(prefix="myos-acme-ca.") as workdir:
        ca = CA(workdir)
        Handler.state = State(args.url, args.http01, ca)
        server = http.server.ThreadingHTTPServer((host, int(port)), Handler)
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        log(f"serving {args.url}/dir on {args.listen}, http-01 against {args.http01}")
        stopped = Handler.state.stop.wait(args.timeout)
        server.shutdown()
        if not stopped:
            log(f"timeout after {args.timeout}s with {Handler.state.issued} certificate(s) issued")
            return 1
        return 0


if __name__ == "__main__":
    sys.exit(main())
