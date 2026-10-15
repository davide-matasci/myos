#!/usr/bin/env python3
"""Make the nginx test's certificates and keys (test.sh): self-signed,
valid from 2000 to 2125, with keys nothing else uses. Needs the Python
cryptography package; the output is checked in, run this only to remake
it."""
import datetime
import ipaddress

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec, rsa
from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID


def make(base, key, names, usage, sans):
    subject = x509.Name([x509.NameAttribute(oid, value) for oid, value in names])
    builder = (
        x509.CertificateBuilder()
        .subject_name(subject)
        .issuer_name(subject)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(datetime.datetime(2000, 1, 1))
        .not_valid_after(datetime.datetime(2125, 1, 1))
        .add_extension(x509.ExtendedKeyUsage([usage]), critical=False)
    )
    if sans:
        builder = builder.add_extension(x509.SubjectAlternativeName(sans), critical=False)
    cert = builder.sign(key, hashes.SHA256())
    with open(base + ".crt", "wb") as f:
        f.write(cert.public_bytes(serialization.Encoding.PEM))
    with open(base + ".key", "wb") as f:
        f.write(key.private_bytes(serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))


make("localhost", ec.generate_private_key(ec.SECP256R1()),
    [(NameOID.COMMON_NAME, "localhost")], ExtendedKeyUsageOID.SERVER_AUTH,
    [x509.DNSName("localhost"), x509.IPAddress(ipaddress.ip_address("127.0.0.1"))])
make("other", rsa.generate_private_key(public_exponent=65537, key_size=2048),
    [(NameOID.ORGANIZATION_NAME, "myos test"), (NameOID.COMMON_NAME, "other.test")],
    ExtendedKeyUsageOID.SERVER_AUTH, [x509.DNSName("other.test")])
make("client", ec.generate_private_key(ec.SECP256R1()),
    [(NameOID.ORGANIZATION_NAME, "myos test"), (NameOID.COMMON_NAME, "client")],
    ExtendedKeyUsageOID.CLIENT_AUTH, [])
