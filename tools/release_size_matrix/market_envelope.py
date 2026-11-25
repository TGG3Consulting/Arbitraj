"""Single-use runner key for transferring an anonymous ledger projection to a hosted test."""
import argparse
import gzip
import os
from pathlib import Path

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa
from cryptography.hazmat.primitives.ciphers.aead import AESGCM


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("keygen", "seal", "open"))
    parser.add_argument("key", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("input", type=Path, nargs="?")
    args = parser.parse_args()
    if args.mode == "keygen":
        key = rsa.generate_private_key(public_exponent=65537, key_size=3072)
        args.key.write_bytes(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                                              serialization.NoEncryption()))
        args.output.write_bytes(key.public_key().public_bytes(serialization.Encoding.PEM,
                                                             serialization.PublicFormat.SubjectPublicKeyInfo))
        return
    rsa_pad = padding.OAEP(mgf=padding.MGF1(hashes.SHA256()), algorithm=hashes.SHA256(), label=None)
    if args.mode == "seal":
        key = serialization.load_pem_public_key(args.key.read_bytes())
        secret, nonce = AESGCM.generate_key(bit_length=256), os.urandom(12)
        args.output.write_bytes(key.encrypt(secret, rsa_pad) + nonce +
                                AESGCM(secret).encrypt(nonce, args.input.read_bytes(), None))
    else:
        key = serialization.load_pem_private_key(args.key.read_bytes(), None)
        blob = args.input.read_bytes()
        cut = key.key_size // 8
        secret = key.decrypt(blob[:cut], rsa_pad)
        args.output.write_bytes(gzip.decompress(AESGCM(secret).decrypt(blob[cut:cut+12], blob[cut+12:], None)))


if __name__ == "__main__":
    main()
