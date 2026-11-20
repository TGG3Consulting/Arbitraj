"""Общее для записи crypto-пулов Curve (решения 442, 453; вынесено из curve_crypto.py — ревью 453, файлы ≤ ~200 строк):
keccak256 (чистый Python), селекторы и слова ABI, JSON-RPC узла (eth_call — ответ или откат как есть), разбор ответов.

Кто вызывает: tools/arbitrum_record/curve_crypto_pools.py, curve_crypto.py.
"""

import requests

# --- keccak256 (чистый Python: в stdlib только sha3_256 с другим дополнением) ---------------------------------------

_RC = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
    0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
    0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
    0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
    0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
    0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
]
_ROT = [
    [0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14],
]
_M = (1 << 64) - 1


def _rol(x, n):
    return ((x << n) | (x >> (64 - n))) & _M if n else x


def _f(a):
    for rc in _RC:
        c = [a[x][0] ^ a[x][1] ^ a[x][2] ^ a[x][3] ^ a[x][4] for x in range(5)]
        d = [c[(x - 1) % 5] ^ _rol(c[(x + 1) % 5], 1) for x in range(5)]
        a = [[a[x][y] ^ d[x] for y in range(5)] for x in range(5)]
        b = [[0] * 5 for _ in range(5)]
        for x in range(5):
            for y in range(5):
                b[y][(2 * x + 3 * y) % 5] = _rol(a[x][y], _ROT[x][y])
        a = [[b[x][y] ^ ((~b[(x + 1) % 5][y]) & b[(x + 2) % 5][y]) for y in range(5)] for x in range(5)]
        a[0][0] ^= rc
    return a


def keccak256(data: bytes) -> bytes:
    rate = 136
    msg = bytearray(data) + b"\x01"
    while len(msg) % rate:
        msg += b"\x00"
    msg[-1] |= 0x80
    a = [[0] * 5 for _ in range(5)]
    for off in range(0, len(msg), rate):
        block = msg[off:off + rate]
        for i in range(rate // 8):
            x, y = i % 5, i // 5
            a[x][y] ^= int.from_bytes(block[8 * i:8 * i + 8], "little")
        a = _f(a)
    out = b""
    for i in range(4):
        out += a[i % 5][i // 5].to_bytes(8, "little")
    return out


assert keccak256(b"").hex() == "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"


def selector(sig: str) -> str:
    return "0x" + keccak256(sig.encode()).hex()[:8]


def word(v: int) -> str:
    return v.to_bytes(32, "big").hex()


def addr_word(a: str) -> str:
    return a.lower().replace("0x", "").rjust(64, "0")


# --- JSON-RPC ---------------------------------------------------------------------------------------------------------


class Node:
    def __init__(self, url: str):
        self.url = url
        self.id = 0

    def rpc(self, method, params):
        self.id += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.id, "method": method, "params": params},
                          timeout=60)
        r.raise_for_status()
        return r.json()

    def call(self, to: str, data: str, tag: str):
        """Возвращает {"result": hex} или {"error": {...}} — откат записывается как есть."""
        j = self.rpc("eth_call", [{"to": to, "data": data}, tag])
        return {"result": j["result"]} if "result" in j else {"error": j.get("error")}


def as_int(res):
    return int(res["result"], 16) if res.get("result") not in (None, "0x") else None


def as_addr(res):
    v = res.get("result")
    return "0x" + v[-40:] if v and len(v) >= 42 else None
