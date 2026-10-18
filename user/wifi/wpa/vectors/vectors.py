#!/usr/bin/env python3
# The wpa crate's test vectors, from implementations other than its own:
# Python's hashlib/hmac and the `cryptography` package's AES-CCM, with the
# PRF, the EAPOL-Key layout and CCMP's nonce and AAD written out here from
# IEEE 802.11-2020 12.7.1.2, 12.7.2 and 12.5.3.3. Run it in this directory
# to rewrite the *.hex files.
import hashlib, hmac, struct
from cryptography.hazmat.primitives.ciphers.aead import AESCCM

def prf(key, label, data, n):
    out = b""
    i = 0
    while len(out) < n:
        out += hmac.new(key, label + b"\0" + data + bytes([i]), hashlib.sha1).digest()
        i += 1
    return out[:n]

def write(name, b):
    open(name, "w").write(b.hex() + "\n")

pmk = hashlib.pbkdf2_hmac("sha1", b"password", b"IEEE", 4096, 32)
aa = bytes.fromhex("02005e000106")
spa = bytes.fromhex("525400123457")
anonce, snonce = b"\x11" * 32, b"\x22" * 32
data = min(aa, spa) + max(aa, spa) + min(anonce, snonce) + max(anonce, snonce)
write("ptk.hex", prf(pmk, b"Pairwise key expansion", data, 48))

# Message 2: version 2, key type 3, descriptor 2; MIC over the frame with
# its MIC field zero.
rsn = bytes([48, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0])
body = (bytes([2]) + struct.pack(">HHQ", 2 | 8 | 0x100, 0, 1) + b"\x22" * 32 + b"\0" * 16 + b"\0" * 8 + b"\0" * 8
        + b"\0" * 16 + struct.pack(">H", len(rsn)) + rsn)
frame = bytes([2, 3]) + struct.pack(">H", len(body)) + body
write("mic.hex", hmac.new(b"\x5a" * 16, frame, hashlib.sha1).digest()[:16])

tk = bytes(range(0xc0, 0xd0))
pn = 0x010203040506
for name, qos in (("ccmp-data.hex", False), ("ccmp-qos.hex", True)):
    hdr = bytearray.fromhex("0801 0000 02005e000106 525400123457 ffffffffffff 3012")
    if qos:
        hdr[0] = 0x88
        hdr[1] |= 0x08
        hdr += bytes([5, 0])
    payload = b"\xaa\xaa\x03\x00\x00\x00\x08\x00hello, wifi"
    nonce = bytes([hdr[24] & 0x0f if qos else 0]) + bytes(hdr[10:16]) + pn.to_bytes(6, "big")
    fc0 = hdr[0] & 0x8f
    fc1 = (hdr[1] & ~0x38 & 0xff) | 0x40
    if qos:
        fc1 &= ~0x80 & 0xff
    aad = bytes([fc0, fc1]) + bytes(hdr[4:22]) + bytes([hdr[22] & 0x0f, 0])
    if qos:
        aad += bytes([hdr[24] & 0x0f, 0])
    ct = AESCCM(tk, tag_length=8).encrypt(nonce, payload, aad)
    p = pn.to_bytes(6, "little")
    out = bytearray(hdr)
    out[1] |= 0x40
    out += bytes([p[0], p[1], 0, 0x20, p[2], p[3], p[4], p[5]]) + ct
    write(name, bytes(out))
