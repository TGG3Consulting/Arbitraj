#!/usr/bin/env python3
# Файл: tools/arbitrum_record/balancer_v3.py
# Что делает: решение 441 (T433-F2) — запись ответов своего узла Arbitrum для пулов Balancer V3 на ОДНОМ блоке (только
#   чтение, eth_call): неизменяемое пула (крючки, токены, наименьшая сумма хранилища, isPoolFromFactory каждой фабрики,
#   веса, нижние балансы), состояние (живые балансы, курсы, конфигурация, паузы, A) — теми же вызовами и в том же
#   порядке, что бот (pools/load_balancer_v3*.rs), и ответы квотера Router V3 querySwapSingleTokenExactIn и
#   querySwapSingleTokenExactOut (ревью 441, п. 3) на несколько сумм по каждой паре токенов пула (eth_call БЕЗ from — Vault.quote требует tx.origin = 0). Ключей и подписей нет.
# Кто вызывает: Тигран вручную (README ниже); тест — tests/arbitrum_balancer_v3.rs (сверяет расчёт бота с роутером).
# От чего зависит: Python 3, requests. Адрес узла и все адреса контрактов — аргументы (официальные значения —
#   balancer/balancer-deployments@b55dc2b8 output/arbitrum.json, пример команды — README в папке записи).
#
# Пример:
#   python3 tools/arbitrum_record/balancer_v3.py --rpc http://127.0.0.1:8547 \
#     --vault 0xbA1333333333a1BA1108E8412f11850A5C319bA9 --router 0xEAedc32a51c510d35ebC11088fD5fF2b47aACF2E \
#     --weighted 0xD961E30156C2E0D0d925A0De45f931CB7815e970 --weighted-min 0x83bf399FA3DC49Af8fb5c34031a50c7C93F56129 \
#     --stable 0xEB1eeaBF0126d813589C3D2CfeFFE410D9aE3863,0x44d33798dddCdAbc93Fe6a40C80588033Dc502d3 \
#     --stable-guard 0x1FC7F1F84CFE61a04224AC8D3F87f56214FeC08c --gt-pages 2
#   (пулы — с GeckoTerminal dex balancer-v3-arbitrum, страниц --gt-pages; или явно --pools a,b,c)

import argparse
import datetime
import hashlib
import json
import os
import sys

import requests

# --- keccak256 (Keccak-f[1600], padding 0x01 — как в Ethereum; нужен только для селекторов функций) ---
_RC = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
    0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
    0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
    0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
    0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
    0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
]
_ROT = [[0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61], [28, 55, 25, 21, 56], [27, 20, 39, 8, 14]]
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
        x, y = i % 5, i // 5
        out += a[x][y].to_bytes(8, "little")
    return out


def sel(sig: str) -> str:
    return keccak256(sig.encode()).hex()[:8]


def word_addr(a: str) -> str:
    return a.lower().replace("0x", "").rjust(64, "0")


def word_int(n: int) -> str:
    return format(n, "064x")


# --- JSON-RPC ---
class Node:
    def __init__(self, url):
        self.url, self.n = url, 0

    def rpc(self, method, params):
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}, timeout=60)
        r.raise_for_status()
        return r.json()

    def call(self, to, data, block):
        """eth_call без from (tx.origin = 0). Возвращает запись: to, data, result или error (ответ узла как есть)."""
        j = self.rpc("eth_call", [{"to": to, "input": "0x" + data}, block])
        rec = {"to": to, "data": "0x" + data}
        if "error" in j:
            rec["error"] = j["error"]
        else:
            rec["result"] = j["result"]
        return rec


def words(hexstr):
    h = hexstr[2:] if hexstr.startswith("0x") else hexstr
    return [int(h[i:i + 64], 16) for i in range(0, len(h), 64)]


def dyn_array(ws, head_index):
    """Разбор uint256[] / address[] по смещению в голове ответа (ABI)."""
    off = ws[head_index] // 32
    n = ws[off]
    return ws[off + 1:off + 1 + n]


def gt_pools(pages):
    out = []
    for p in range(1, pages + 1):
        url = f"https://api.geckoterminal.com/api/v2/networks/arbitrum/dexes/balancer-v3-arbitrum/pools?page={p}"
        r = requests.get(url, headers={"Accept": "application/json"}, timeout=60)
        r.raise_for_status()
        out.append(r.json())
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--vault", required=True)
    ap.add_argument("--router", required=True)
    ap.add_argument("--weighted", default="", help="фабрики 20241205-v3-weighted-pool через запятую")
    ap.add_argument("--weighted-min", default="", help="фабрики 20260115-v3-weighted-pool-v2")
    ap.add_argument("--stable", default="", help="фабрики 20241205-v3-stable-pool и 20250324-v3-stable-pool-v2")
    ap.add_argument("--stable-guard", default="", help="фабрики 20260116-v3-stable-pool-v3")
    ap.add_argument("--pools", default="", help="адреса пулов через запятую")
    ap.add_argument("--gt-pages", type=int, default=0, help="сколько страниц пулов взять с GeckoTerminal")
    ap.add_argument("--block", default="", help="номер блока (по умолчанию — текущая голова)")
    ap.add_argument("--out", default="", help="папка записи (по умолчанию tests/data/arbitrum_balancer_v3_<дата>)")
    a = ap.parse_args()

    node = Node(a.rpc)
    chain = node.rpc("eth_chainId", [])["result"]
    head = int(a.block) if a.block else int(node.rpc("eth_blockNumber", [])["result"], 16)
    tag = hex(head)
    blk = node.rpc("eth_getBlockByNumber", [tag, False])["result"]
    date = datetime.datetime.utcfromtimestamp(int(blk["timestamp"], 16)).strftime("%Y-%m-%d")
    out = a.out or os.path.join("tests", "data", f"arbitrum_balancer_v3_{date}")
    os.makedirs(out, exist_ok=True)

    factories = []
    for name, arg in [("weighted", a.weighted), ("weighted_min", a.weighted_min), ("stable", a.stable),
                      ("stable_guard", a.stable_guard)]:
        for f in [x.strip() for x in arg.split(",") if x.strip()]:
            factories.append({"factory": f, "release": name})

    pools = [x.strip() for x in a.pools.split(",") if x.strip()]
    if a.gt_pages:
        gt = gt_pools(a.gt_pages)
        with open(os.path.join(out, "geckoterminal_pools.json"), "w") as fh:
            json.dump(gt, fh, indent=1)
        for page in gt:
            for p in page.get("data", []):
                addr = p.get("attributes", {}).get("address")
                if addr and addr not in pools:
                    pools.append(addr)

    meta = {"chain_id": chain, "block": head, "block_hash": blk["hash"], "timestamp": int(blk["timestamp"], 16),
            "vault": a.vault, "router": a.router, "factories": factories, "pools": pools}
    with open(os.path.join(out, "meta.json"), "w") as fh:
        json.dump(meta, fh, indent=1)

    v = a.vault
    for pool in pools:
        pw = word_addr(pool)
        rec = {"pool": pool, "fixed": {}, "state": {}, "quotes": []}
        fx = rec["fixed"]
        fx["getHooksConfig"] = node.call(v, sel("getHooksConfig(address)") + pw, tag)
        fx["getPoolTokenInfo"] = node.call(v, sel("getPoolTokenInfo(address)") + pw, tag)
        fx["getMinimumTradeAmount"] = node.call(v, sel("getMinimumTradeAmount()"), tag)
        fx["isPoolFromFactory"] = [dict(f, **node.call(f["factory"], sel("isPoolFromFactory(address)") + pw, tag))
                                   for f in factories]
        fx["getNormalizedWeights"] = node.call(pool, sel("getNormalizedWeights()"), tag)
        fx["getMinTokenBalances"] = node.call(pool, sel("getMinTokenBalances()"), tag)
        st = rec["state"]
        # Порядок — как load_balancer_v3::calls (0…5, у стейблсвапа ещё 6).
        st["getPoolTokenInfo"] = fx["getPoolTokenInfo"]
        st["getCurrentLiveBalances"] = node.call(v, sel("getCurrentLiveBalances(address)") + pw, tag)
        st["getPoolTokenRates"] = node.call(v, sel("getPoolTokenRates(address)") + pw, tag)
        st["getPoolConfig"] = node.call(v, sel("getPoolConfig(address)") + pw, tag)
        st["isPoolPaused"] = node.call(v, sel("isPoolPaused(address)") + pw, tag)
        st["isVaultPaused"] = node.call(v, sel("isVaultPaused()"), tag)
        st["getAmplificationParameter"] = node.call(pool, sel("getAmplificationParameter()"), tag)
        info = fx["getPoolTokenInfo"]
        if "result" in info:
            ws = words(info["result"])
            tokens = ["0x" + format(t, "040x") for t in dyn_array(ws, 0)]
            raw = dyn_array(ws, 2)
            q = sel("querySwapSingleTokenExactIn(address,address,address,uint256,address,bytes)")
            # Ревью 441, п. 3: и точный выход (querySwapSingleTokenExactOut) — сверка обратного расчёта бота (amount_in).
            qo = sel("querySwapSingleTokenExactOut(address,address,address,uint256,address,bytes)")
            rec["quotes_out"] = []
            for i, ti in enumerate(tokens):
                for j, tj in enumerate(tokens):
                    if i == j:
                        continue
                    for div in (1_000_000, 10_000, 1_000, 100, 10):
                        amt = raw[i] // div
                        if amt == 0:
                            continue
                        data = (q + pw + word_addr(ti) + word_addr(tj) + word_int(amt) + word_addr("0x0")
                                + word_int(0xC0) + word_int(0))
                        r = node.call(a.router, data, tag)
                        rec["quotes"].append(dict(r, i=i, j=j, token_in=ti, token_out=tj, amount=str(amt)))
                    for div in (1_000_000, 10_000, 1_000, 100, 10):
                        amt = raw[j] // div
                        if amt == 0:
                            continue
                        data = (qo + pw + word_addr(ti) + word_addr(tj) + word_int(amt) + word_addr("0x0")
                                + word_int(0xC0) + word_int(0))
                        r = node.call(a.router, data, tag)
                        rec["quotes_out"].append(dict(r, i=i, j=j, token_in=ti, token_out=tj, amount=str(amt)))
        with open(os.path.join(out, f"pool_{pool.lower()}.json"), "w") as fh:
            json.dump(rec, fh, indent=1)
        print(pool, "квот:", len(rec["quotes"]), file=sys.stderr)

    lines = [f"# Balancer V3 на Arbitrum — запись своего узла (решение 441, T433-F2)", "",
             f"Блок {head}, хеш {blk['hash']}, время {blk['timestamp']} ({date}), chainId {chain}.",
             "Только eth_call (чтение), без from; ключей и подписей нет. Скрипт — tools/arbitrum_record/balancer_v3.py.",
             f"Хранилище {a.vault}, роутер {a.router}; фабрики — meta.json. Пулов: {len(pools)}.", "",
             "Файлы: meta.json — блок, адреса, список пулов; pool_<адрес>.json — fixed (неизменяемое), state (вызовы "
             "в порядке load_balancer_v3::calls), quotes (Router V3 querySwapSingleTokenExactIn: i, j, сумма, ответ), quotes_out "
             "(querySwapSingleTokenExactOut: i, j, точный выход, ответ — вход); "
             "geckoterminal_pools.json — ответ GeckoTerminal (если --gt-pages).", "", "## SHA-256 файлов"]
    for name in sorted(os.listdir(out)):
        if name == "README.md":
            continue
        with open(os.path.join(out, name), "rb") as fh:
            lines.append(f"{hashlib.sha256(fh.read()).hexdigest()}  {name}")
    with open(os.path.join(out, "README.md"), "w") as fh:
        fh.write("\n".join(lines) + "\n")
    print("записано:", out, file=sys.stderr)


if __name__ == "__main__":
    main()
