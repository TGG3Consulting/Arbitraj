#!/usr/bin/env python3
"""Запись ответов своего узла Arbitrum One для crypto-пулов Curve (решение 442: Twocrypto-NG, Tricrypto-NG).

Что делает: на ОДНОМ блоке (номер берётся у узла в начале) читает у фабрик Twocrypto-NG и Tricrypto-NG список пулов
(pool_count, pool_list), у каждого пула — те же геттеры, что бот (pools/load_crypto.rs: A, gamma, D, mid_fee, out_fee,
fee_gamma, future_A_gamma_time, precisions, coins(k), balances(k), price_scale), проверку (get_coins фабрики, version(),
MATH()) и ответы квотера get_dy(i, j, dx) лесенкой сумм по каждой паре монет в обе стороны. Пишет JSON-файлы и README
(блок, время блока, SHA-256 файлов) в tests/data/arbitrum_curve_crypto_<дата>/. Ключей, подписей и транзакций нет —
только eth_call и eth_getBlockByNumber (чтение).

Запуск (Python 3, requests):
    python3 tools/arbitrum_record/curve_crypto.py --node http://АДРЕС_УЗЛА:8547 \
        --twocrypto-factory 0x98EE851a00abeE0d95D08cF4CA2BdCE32aeaAF7F \
        --tricrypto-factory 0xbC0797015fcFc47d9C1856639CaE50D0e69FbEE8

Адреса фабрик — официальные deployments.yaml (curvefi/twocrypto-ng 89c8dfb, curvefi/tricrypto-ng ecaa816, раздел
arbitrum:mainnet). Сигнатуры вызовов — исходники пулов (ссылки — src/venues/bsc_dex/pools/abi_crypto.rs).
"""

import argparse
import datetime
import hashlib
import json
import os
import sys

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


# --- запись -------------------------------------------------------------------------------------------------------------

# Геттеры пула в порядке pools/load_crypto.rs calls(): HEAD (8), затем coins(k)/balances(k), затем price_scale.
HEAD = ["A()", "gamma()", "D()", "mid_fee()", "out_fee()", "fee_gamma()", "future_A_gamma_time()", "precisions()"]
# Доли баланса монеты входа для лесенки get_dy (числитель над 10^6).
LADDER_PPM = [1, 100, 1000, 10000, 100000, 500000]


def pool_calls(node, pool, n, tag):
    out = {}
    for sig in HEAD:
        out[sig] = node.call(pool, selector(sig), tag)
    for k in range(n):
        out[f"coins({k})"] = node.call(pool, selector("coins(uint256)") + word(k), tag)
        out[f"balances({k})"] = node.call(pool, selector("balances(uint256)") + word(k), tag)
    if n == 2:
        out["price_scale()"] = node.call(pool, selector("price_scale()"), tag)
    else:
        for k in range(n - 1):
            out[f"price_scale({k})"] = node.call(pool, selector("price_scale(uint256)") + word(k), tag)
    out["version()"] = node.call(pool, selector("version()"), tag)
    out["MATH()"] = node.call(pool, selector("MATH()"), tag)
    return out


def ladder(node, pool, n, calls, tag):
    rows = []
    for i in range(n):
        bal = as_int(calls[f"balances({i})"]) or 0
        for j in range(n):
            if i == j:
                continue
            for ppm in LADDER_PPM:
                dx = bal * ppm // 1_000_000
                if dx == 0:
                    continue
                data = selector("get_dy(uint256,uint256,uint256)") + word(i) + word(j) + word(dx)
                rows.append({"i": i, "j": j, "dx": str(dx), "get_dy": node.call(pool, data, tag)})
    return rows


def factory_pools(node, factory, tag, limit):
    count = as_int(node.call(factory, selector("pool_count()"), tag)) or 0
    pools = []
    for k in range(min(count, limit)):
        a = as_addr(node.call(factory, selector("pool_list(uint256)") + word(k), tag))
        if a:
            pools.append(a)
    return count, pools


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum One")
    ap.add_argument("--twocrypto-factory", required=True, help="фабрика Twocrypto-NG (deployments.yaml)")
    ap.add_argument("--tricrypto-factory", required=True, help="фабрика Tricrypto-NG (deployments.yaml)")
    ap.add_argument("--max-pools", type=int, default=40, help="сколько пулов каждой фабрики читать (с начала pool_list)")
    ap.add_argument("--out", default=None, help="папка (по умолчанию tests/data/arbitrum_curve_crypto_<дата>)")
    a = ap.parse_args()

    node = Node(a.node)
    chain = node.rpc("eth_chainId", [])["result"]
    head = int(node.rpc("eth_blockNumber", [])["result"], 16)
    tag = hex(head)
    blk = node.rpc("eth_getBlockByNumber", [tag, False])["result"]
    date = datetime.date.today().isoformat()
    out = a.out or os.path.join(os.path.dirname(__file__), "..", "..", "tests", "data", f"arbitrum_curve_crypto_{date}")
    os.makedirs(out, exist_ok=True)

    files = {}
    for kind, factory, n in (("twocrypto", a.twocrypto_factory, 2), ("tricrypto", a.tricrypto_factory, 3)):
        count, pools = factory_pools(node, factory, tag, a.max_pools)
        rows = []
        for p in pools:
            calls = pool_calls(node, p, n, tag)
            get_coins = node.call(factory, selector("get_coins(address)") + addr_word(p), tag)
            rows.append({
                "pool": p,
                "calls": calls,
                "factory_get_coins": get_coins,
                "get_dy": ladder(node, p, n, calls, tag),
            })
            print(f"{kind} {p}: {len(rows[-1]['get_dy'])} ответов get_dy", file=sys.stderr)
        doc = {
            "chain_id": chain,
            "block": head,
            "block_hash": blk["hash"],
            "timestamp": int(blk["timestamp"], 16),
            "kind": kind,
            "factory": factory,
            "pool_count": count,
            "head_order": HEAD,
            "pools": rows,
        }
        name = f"{kind}.json"
        with open(os.path.join(out, name), "w") as f:
            json.dump(doc, f, indent=1)
        files[name] = doc

    lines = [
        f"# Curve crypto-пулы Arbitrum One — запись своего узла, {date}",
        "",
        "Скрипт: tools/arbitrum_record/curve_crypto.py (только eth_call / eth_getBlockByNumber, без ключей).",
        f"Узел: chainId {chain}; блок {head} ({blk['hash']}), время блока {int(blk['timestamp'], 16)}.",
        f"Фабрика Twocrypto-NG {a.twocrypto_factory}: pool_count {files['twocrypto.json']['pool_count']}; "
        f"Tricrypto-NG {a.tricrypto_factory}: pool_count {files['tricrypto.json']['pool_count']}.",
        "",
        "Что записано (на одном блоке): у каждого пула — A, gamma, D, mid_fee, out_fee, fee_gamma, future_A_gamma_time,",
        "precisions, coins(k), balances(k), price_scale, version(), MATH(), get_coins(pool) фабрики и лесенка get_dy(i, j, dx)",
        f"по каждой паре монет в обе стороны (dx — доли баланса монеты входа, ppm: {LADDER_PPM}). Откат вызова — поле error.",
        "Читает тест tests/arb_curve_crypto_live.rs (снять #[ignore] после записи).",
        "",
        "## SHA-256 файлов",
    ]
    for name in sorted(files):
        with open(os.path.join(out, name), "rb") as f:
            lines.append(f"{hashlib.sha256(f.read()).hexdigest()}  {name}")
    with open(os.path.join(out, "README.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print(f"записано в {out}", file=sys.stderr)


if __name__ == "__main__":
    main()
