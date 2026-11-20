#!/usr/bin/env python3
"""Запись ответов своего узла Arbitrum One для crypto-пулов Curve (решения 442, 453).

Что делает: на ОДНОМ блоке (номер берётся у узла в начале) читает:
  * фабрики Twocrypto-NG и Tricrypto-NG — список пулов (pool_count, pool_list; пулы берутся С КОНЦА списка — новые);
  * у каждого пула — ВСЕ вызовы, которые делает бот (pools/load_crypto.rs calls(); для v1 — pools/load_crypto_v1.rs
    calls()), и проверочные (get_coins фабрики, version(), MATH()); ответ записывается по данным вызова (calldata) —
    тест берёт ответ на тот же calldata, что строит бот;
  * лесенку get_dy(i, j, dx) по каждой паре монет в обе стороны;
  * eth_getCode: реализаций пула (amm, amm_v2_0_0 Twocrypto; amm_native_transfers_enabled/disabled Tricrypto),
    математики (math, math_v2_0_0; math Tricrypto) и каждого пула — для сверки версии исходника по хешу кода;
  * фабрику crypto v1 (pool_count, pool_list, get_coins, get_decimals, код пула — прокси EIP-1167 — и код шаблона) и
    tricrypto 2021 (пул из журнала развёртывания) — решение 453; decimals() монет (точность v1 — по decimals).
Пулы Twocrypto-NG делятся по MATH(): файл twocrypto.json содержит и v2.1.0, и v2.0.0 (--max-pools на каждую
математику, если хватает пулов). Пишет JSON-файлы и README (блок, время блока, SHA-256 файлов) в
tests/data/arbitrum_curve_crypto_<дата>/. Ключей, подписей и транзакций нет — только eth_call, eth_getCode и
eth_getBlockByNumber (чтение).

Запуск (Python 3, requests; адреса — официальные журналы развёртывания curvefi, см. docs/FIRST_RUN.md):
    python3 tools/arbitrum_record/curve_crypto.py --node http://АДРЕС_УЗЛА:8547 \\
        --twocrypto-factory 0x98EE851a00abeE0d95D08cF4CA2BdCE32aeaAF7F \\
        --twocrypto-math 0x1Fd8Af16DC4BEBd950521308D55d0543b6cDF4A1 \\
        --twocrypto-math-v200 0x2005995a71243be9FB995DaB4742327dc76564Df \\
        --twocrypto-impls 0x934791f7F391727db92BFF94cd789c4623d14c52,0x04Fd6beC7D45EFA99a27D29FB94b55c56dD07223 \\
        --tricrypto-factory 0xbC0797015fcFc47d9C1856639CaE50D0e69FbEE8 \\
        --tricrypto-math 0x604388Bb1159AFd21eB5191cE22b4DeCdEE2Ae22 \\
        --tricrypto-impls 0xd7E72f3615aa65b92A4DBdC211E296a35512988B,0x1f7C86AffE5bCF7a1D74a8c8E2ef9E03BF31c1BD \\
        --v1-factory 0xE5De15A9C9bBedb4F5EC13B131E61245f2983A69 \\
        --v1-template 0x5bcA7dDF1bcccB2eE8e46c56bfc9d3CDC77262bC \\
        --tricrypto-v1-pool 0x960ea3e3C7FB317332d990873d354E18d7645590

Источники адресов: curvefi/twocrypto-ng 89c8dfb и curvefi/tricrypto-ng ecaa816 deployments.yaml (arbitrum:mainnet);
curvefi/curve-factory-crypto 7ed1011 deployment-logs/others.log; curvefi/curve-crypto-contract d7d04cd
«deployment-logs/2021-09-13. Arbitrum/log.txt». Сигнатуры вызовов — исходники пулов (src/venues/bsc_dex/pools/
abi_crypto.rs, abi_crypto_v1.rs).
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

# Доли баланса монеты входа для лесенки get_dy (числитель над 10^6).
LADDER_PPM = [1, 100, 1000, 10000, 100000, 500000]

# Вызовы бота: pools/load_crypto.rs calls() (NG) и pools/load_crypto_v1.rs calls() (v1) — тот же порядок и те же данные.
NG_HEAD = ["A()", "gamma()", "D()", "mid_fee()", "out_fee()", "fee_gamma()", "future_A_gamma_time()"]


def bot_calls(n, v1):
    """Сигнатуры и аргументы вызовов бота (calldata). n — число монет, v1 — пул первого поколения."""
    out = [(sig, "") for sig in NG_HEAD]
    if not v1:
        out.append(("precisions()", ""))
    for k in range(n):
        out.append(("coins(uint256)", word(k)))
        out.append(("balances(uint256)", word(k)))
    if n == 2:
        out.append(("price_scale()", ""))
    else:
        for k in range(n - 1):
            out.append(("price_scale(uint256)", word(k)))
    if v1:
        if n == 3:
            out.append(("is_killed()", ""))
    else:
        out.append(("MATH()", ""))
    return out


def record_calls(node, pool, n, v1, tag):
    """Ответы на вызовы бота и проверочные — словарь calldata → ответ."""
    calls = {}
    sigs = bot_calls(n, v1) + ([] if v1 else [("version()", "")])
    for sig, args in sigs:
        data = selector(sig) + args
        calls[data] = node.call(pool, data, tag)
    return calls


def answer(calls, sig, args=""):
    return calls.get(selector(sig) + args, {})


def ladder(node, pool, n, calls, tag):
    rows = []
    for i in range(n):
        bal = as_int(answer(calls, "balances(uint256)", word(i))) or 0
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


def code(node, addr, tag):
    """eth_getCode: hex кода (или ошибка узла)."""
    j = node.rpc("eth_getCode", [addr, tag])
    return {"code": j["result"]} if "result" in j else {"error": j.get("error")}


def holdings(node, pool, calls, n, tag):
    """Что пул держит на самом деле: balanceOf(pool) у каждой монеты и eth_getBalance(pool) — для вопроса 5 решения 442
    (Tricrypto-NG с нативным ETH держит ETH, а не WETH; источник встречного токена пробы берётся по balanceOf)."""
    out = {"eth_balance": node.rpc("eth_getBalance", [pool, tag]).get("result")}
    for k in range(n):
        c = as_addr(answer(calls, "coins(uint256)", word(k)))
        if c:
            out[c] = node.call(c, selector("balanceOf(address)") + addr_word(pool), tag)
    return out


def coin_decimals(node, calls, n, tag):
    out = []
    for k in range(n):
        c = as_addr(answer(calls, "coins(uint256)", word(k)))
        out.append(as_int(node.call(c, selector("decimals()"), tag)) if c else None)
    return out


def factory_pools(node, factory, tag):
    count = as_int(node.call(factory, selector("pool_count()"), tag)) or 0
    return count, lambda k: as_addr(node.call(factory, selector("pool_list(uint256)") + word(k), tag))


def ng_rows(node, factory, n, maths, per_math, tag):
    """Пулы фабрики NG с конца pool_list: до per_math пулов каждой математики из maths (MATH() пула)."""
    count, at = factory_pools(node, factory, tag)
    taken = {m.lower(): 0 for m in maths}
    rows = []
    for k in reversed(range(count)):
        if all(v >= per_math for v in taken.values()):
            break
        p = at(k)
        if not p:
            continue
        calls = record_calls(node, p, n, False, tag)
        m = as_addr(answer(calls, "MATH()"))
        if m is None or m.lower() not in taken or taken[m.lower()] >= per_math:
            continue
        taken[m.lower()] += 1
        rows.append({
            "pool": p,
            "index": k,
            "calls": calls,
            "factory_get_coins": node.call(factory, selector("get_coins(address)") + addr_word(p), tag),
            "code": code(node, p, tag),
            "holdings": holdings(node, p, calls, n, tag),
            "get_dy": ladder(node, p, n, calls, tag),
        })
        print(f"NG({n}) {p}: MATH {m}, {len(rows[-1]['get_dy'])} ответов get_dy", file=sys.stderr)
    return count, rows


def v1_row(node, pool, n, tag, factory=None):
    calls = record_calls(node, pool, n, True, tag)
    row = {
        "pool": pool,
        "calls": calls,
        "decimals": coin_decimals(node, calls, n, tag),
        "code": code(node, pool, tag),
        "holdings": holdings(node, pool, calls, n, tag),
        "get_dy": ladder(node, pool, n, calls, tag),
    }
    if factory:
        row["factory_get_coins"] = node.call(factory, selector("get_coins(address)") + addr_word(pool), tag)
        row["factory_get_decimals"] = node.call(factory, selector("get_decimals(address)") + addr_word(pool), tag)
    print(f"v1({n}) {pool}: {len(row['get_dy'])} ответов get_dy", file=sys.stderr)
    return row


def split(v):
    return [a for a in (v or "").split(",") if a]


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum One")
    ap.add_argument("--twocrypto-factory", required=True)
    ap.add_argument("--twocrypto-math", required=True, help="math v2.1.0 (deployments.yaml math)")
    ap.add_argument("--twocrypto-math-v200", required=True, help="math v2.0.0 (deployments.yaml math_v2_0_0)")
    ap.add_argument("--twocrypto-impls", default="", help="реализации пула через запятую (amm, amm_v2_0_0)")
    ap.add_argument("--tricrypto-factory", required=True)
    ap.add_argument("--tricrypto-math", required=True)
    ap.add_argument("--tricrypto-impls", default="", help="amm_native_transfers_enabled, …_disabled")
    ap.add_argument("--v1-factory", required=True, help="фабрика crypto v1 (others.log)")
    ap.add_argument("--v1-template", required=True, help="шаблон пула фабрики crypto v1 (others.log)")
    ap.add_argument("--tricrypto-v1-pool", required=True, help="tricrypto 2021 (log.txt)")
    ap.add_argument("--max-pools", type=int, default=20, help="пулов на каждую математику / фабрику v1")
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
    base = {"chain_id": chain, "block": head, "block_hash": blk["hash"], "timestamp": int(blk["timestamp"], 16)}

    files = {}
    specs = (
        ("twocrypto", a.twocrypto_factory, 2, [a.twocrypto_math, a.twocrypto_math_v200], split(a.twocrypto_impls)),
        ("tricrypto", a.tricrypto_factory, 3, [a.tricrypto_math], split(a.tricrypto_impls)),
    )
    for kind, factory, n, maths, impls in specs:
        count, rows = ng_rows(node, factory, n, maths, a.max_pools, tag)
        files[f"{kind}.json"] = dict(base, kind=kind, factory=factory, pool_count=count, maths=maths,
                                     code_of={x: code(node, x, tag) for x in maths + impls}, pools=rows)

    count, at = factory_pools(node, a.v1_factory, tag)
    v1 = [v1_row(node, p, 2, tag, a.v1_factory) for p in (at(k) for k in list(reversed(range(count)))[: a.max_pools]) if p]
    files["crypto_v1.json"] = dict(base, kind="crypto_v1", factory=a.v1_factory, pool_count=count,
                                   templates=[a.v1_template], code_of={a.v1_template: code(node, a.v1_template, tag)},
                                   pools=v1)
    files["tricrypto_v1.json"] = dict(base, kind="tricrypto_v1", pools=[v1_row(node, a.tricrypto_v1_pool, 3, tag)])

    for name, doc in files.items():
        with open(os.path.join(out, name), "w") as f:
            json.dump(doc, f, indent=1)

    lines = [
        f"# Curve crypto-пулы Arbitrum One — запись своего узла, {date}",
        "",
        "Скрипт: tools/arbitrum_record/curve_crypto.py (только eth_call / eth_getCode / eth_getBlockByNumber, без ключей).",
        f"Узел: chainId {chain}; блок {head} ({blk['hash']}), время блока {base['timestamp']}.",
        "",
        "Что записано (на одном блоке): ответы на вызовы бота по calldata (pools/load_crypto.rs, load_crypto_v1.rs),",
        "version(), MATH(), get_coins (и get_decimals у v1) фабрики, код (eth_getCode) пулов, реализаций и математики,",
        "decimals() монет v1, balanceOf(pool) монет и eth_getBalance(pool) (holdings),",
        f"лесенка get_dy(i, j, dx) в обе стороны (dx — доли баланса, ppm: {LADDER_PPM}).",
        "Пулы фабрик — с конца pool_list. Откат вызова — поле error.",
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
