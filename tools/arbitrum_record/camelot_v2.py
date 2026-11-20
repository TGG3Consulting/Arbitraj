#!/usr/bin/env python3
"""
Запись ответов своего узла Arbitrum One для семейства Camelot V2 (решение 443, T433-F4).
Только чтение (eth_call, eth_getLogs, eth_getCode, eth_getBlockByNumber) — без ключей, подписей и отправки.

Что пишет в tests/data/arbitrum_camelot_v2_<дата>/ (дата — UTC в момент запуска):
  probe.json — блок B (номер, хеш, время), фабрика, пары: factory/token0/token1, getReserves (сырой ответ), stableSwap,
               precisionMultiplier0/1, decimals токенов, getPair фабрики, sha256 и длина кода пары, getAmountOut пары
               на суммах в обе стороны — всё на одном блоке B.
  logs.json  — реальные логи: Sync выбранных пар (последние --log-blocks блоков до B), FeePercentUpdated и
               SetStableSwap пар этой фабрики (поиск назад от B частями по --chunk блоков, пока не наберётся --rare
               штук каждого или не кончится история), PairCreated фабрики (последние найденные).
  README.md  — что записано, блок, хеши файлов (sha256).

Как выбираются пары: allPairsLength фабрики, равномерная выборка --scan индексов allPairs(i); из пар с ненулевыми
резервами — все стабильные (stableSwap = true) и волатильные с разными парами комиссий (token0/1FeePercent), всего
не больше --pairs. Суммы getAmountOut: резерв входа / d для d из --divs.

Источник ABI — verified-исходник фабрики Camelot V2 0x6EcCab422D763aC031210895C81787E87B43A652 (Arbiscan; адрес —
docs.camelot.exchange/contracts/arbitrum/one-mainnet), тот же текст CamelotPair, что в коде бота
(src/venues/bsc_dex/pools/abi_camelot.rs). Селекторы и topic0 считаются здесь keccak256 от подписей.

Установка:  pip install requests
Запуск:     python3 tools/arbitrum_record/camelot_v2.py http://АДРЕС_УЗЛА:8547 \
                --factory 0x6EcCab422D763aC031210895C81787E87B43A652
            (из корня репозитория; --out — другая папка tests/data)
"""
import argparse
import datetime
import hashlib
import json
import os
import sys

import requests

# --- keccak256 (Keccak-f[1600], padding 0x01 — как в Ethereum), без внешних библиотек ---
_RC = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000, 0x000000000000808B,
    0x0000000080000001, 0x8000000080008081, 0x8000000000008009, 0x000000000000008A, 0x0000000000000088,
    0x0000000080008009, 0x000000008000000A, 0x000000008000808B, 0x800000000000008B, 0x8000000000008089,
    0x8000000000008003, 0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
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
    p = bytearray(data) + b"\x01" + b"\x00" * ((rate - (len(data) + 1) % rate) % rate)
    p[-1] |= 0x80
    a = [[0] * 5 for _ in range(5)]
    for off in range(0, len(p), rate):
        blk = p[off:off + rate]
        for i in range(rate // 8):
            a[i % 5][i // 5] ^= int.from_bytes(blk[8 * i:8 * i + 8], "little")
        a = _f(a)
    out = b"".join(a[i % 5][i // 5].to_bytes(8, "little") for i in range(4))
    return out


def sel(sig):
    return "0x" + keccak256(sig.encode()).hex()[:8]


def topic(sig):
    return "0x" + keccak256(sig.encode()).hex()


assert sel("transfer(address,uint256)") == "0xa9059cbb", "keccak256 неверен"

# --- JSON-RPC ---
class Node:
    def __init__(self, url):
        self.url, self.s, self.n = url, requests.Session(), 0

    def batch(self, reqs):
        """reqs: [(method, params)] → [result | {"error": …}] в том же порядке."""
        body = []
        for m, p in reqs:
            self.n += 1
            body.append({"jsonrpc": "2.0", "id": self.n, "method": m, "params": p})
        r = self.s.post(self.url, json=body, timeout=120)
        r.raise_for_status()
        by = {x["id"]: x for x in r.json()}
        return [by[b["id"]].get("result", {"error": by[b["id"]].get("error")}) for b in body]

    def one(self, m, p):
        res = self.batch([(m, p)])[0]
        if isinstance(res, dict) and "error" in res:
            raise RuntimeError(f"{m}: {res['error']}")
        return res

    def calls(self, items, tag, size=200):
        """items: [(to, data)] → [hex | None (откат)]. Откат — ошибка узла с кодом 3 или текстом «execution reverted»
        (geth / Nitro); любая другая ошибка узла (лимит, нет блока, сбой) — исключение: запись не продолжается."""
        out = []
        for i in range(0, len(items), size):
            part = items[i:i + size]
            res = self.batch([("eth_call", [{"to": to, "data": d}, tag]) for to, d in part])
            for (to, d), x in zip(part, res):
                if not isinstance(x, dict):
                    out.append(x)
                    continue
                err = x.get("error") or {}
                if err.get("code") == 3 or "execution reverted" in str(err.get("message", "")).lower():
                    out.append(None)
                else:
                    raise RuntimeError(f"eth_call {to} {d[:10]}: ошибка узла (не откат): {err}")
        return out


def word(h, i=0):
    """Слово i ответа. Пустой или короткий ответ («0x» — у адреса нет кода или вызов ничего не вернул) — понятная
    причина, а не ValueError разбора."""
    h = h[2:] if h.startswith("0x") else h
    if len(h) < 64 * (i + 1):
        raise RuntimeError(f"ответ 0x{h} короче {i + 1} слов — у адреса нет кода или вызов не вернул данных")
    return int(h[64 * i:64 * (i + 1)], 16)


def full(h, n):
    """Ответ не короче n слов (пары без кода / не той ABI отсеиваются при выборке)."""
    return h is not None and len(h[2:] if h.startswith("0x") else h) >= 64 * n


def addr_word(h):
    return "0x" + (h[2:] if h.startswith("0x") else h)[24:64]


def enc_addr(a):
    return a[2:].lower().rjust(64, "0")


def enc_uint(v):
    return hex(v)[2:].rjust(64, "0")


S = {k: sel(k) for k in [
    "allPairsLength()", "allPairs(uint256)", "getPair(address,address)", "factory()", "token0()", "token1()",
    "getReserves()", "stableSwap()", "precisionMultiplier0()", "precisionMultiplier1()", "decimals()",
    "getAmountOut(uint256,address)",
]}
T = {
    "sync": topic("Sync(uint112,uint112)"),
    "fee": topic("FeePercentUpdated(uint16,uint16)"),
    "stable": topic("SetStableSwap(bool,bool)"),
    "created": topic("PairCreated(address,address,address,uint256)"),
}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("node", help="HTTP JSON-RPC своего узла Arbitrum One")
    ap.add_argument("--factory", required=True, help="фабрика Camelot V2 (официальная документация)")
    ap.add_argument("--scan", type=int, default=3000, help="сколько индексов allPairs просмотреть")
    ap.add_argument("--pairs", type=int, default=24, help="сколько пар записать")
    ap.add_argument("--divs", default="1000000,10000,1000,100,10,3", help="суммы: резерв входа / d")
    ap.add_argument("--log-blocks", type=int, default=20000, help="окно Sync выбранных пар, блоков")
    ap.add_argument("--chunk", type=int, default=2000000, help="часть поиска редких событий, блоков")
    ap.add_argument("--from-block", type=int, default=0, help="нижняя граница поиска редких событий")
    ap.add_argument("--rare", type=int, default=20, help="сколько FeePercentUpdated / SetStableSwap / PairCreated")
    ap.add_argument("--out", default="tests/data", help="папка tests/data")
    a = ap.parse_args()
    node, fac = Node(a.node), a.factory
    chain = int(node.one("eth_chainId", []), 16)
    b = int(node.one("eth_blockNumber", []), 16)
    blk = node.one("eth_getBlockByNumber", [hex(b), False])
    tag = hex(b)
    print(f"chainId {chain}, блок {b} {blk['hash']}", file=sys.stderr)

    n = word(node.calls([(fac, S["allPairsLength()"])], tag)[0])
    step = max(1, n // max(1, a.scan))
    idx = list(range(0, n, step))
    addrs = [addr_word(x) for x in node.calls([(fac, S["allPairs(uint256)"] + enc_uint(i)) for i in idx], tag) if full(x, 1)]
    rare = {"fee": [], "stable": [], "created": []}
    hi = b
    while hi > a.from_block and any(len(v) < a.rare for v in rare.values()):
        lo2 = max(a.from_block, hi - a.chunk)
        got = node.one("eth_getLogs", [{"fromBlock": hex(lo2), "toBlock": hex(hi),
                                        "topics": [[T["fee"], T["stable"]]]}])
        own = sorted({g["address"] for g in got})
        facs = node.calls([(x, S["factory()"]) for x in own], tag) if own else []
        ours = {x for x, f in zip(own, facs) if full(f, 1) and addr_word(f).lower() == fac.lower()}
        for g in reversed(got):
            k = "fee" if g["topics"][0] == T["fee"] else "stable"
            if g["address"] in ours and len(rare[k]) < a.rare:
                rare[k].append(g)
        if len(rare["created"]) < a.rare:
            cr = node.one("eth_getLogs", [{"fromBlock": hex(lo2), "toBlock": hex(hi), "address": fac,
                                           "topics": [T["created"]]}])
            rare["created"] += list(reversed(cr))[: a.rare - len(rare["created"])]
        print(f"поиск событий {lo2}..{hi}: fee {len(rare['fee'])}, stable {len(rare['stable'])}, "
              f"created {len(rare['created'])}", file=sys.stderr)
        hi = lo2 - 1

    # Пары из SetStableSwap — кандидаты в стабильные (выборка по индексам их может не задеть).
    extra = sorted({g["address"] for g in rare["stable"]} - {x.lower() for x in addrs})
    addrs += extra
    fields = ["factory()", "token0()", "token1()", "getReserves()", "stableSwap()", "precisionMultiplier0()",
              "precisionMultiplier1()"]
    raw = node.calls([(p, S[f]) for p in addrs for f in fields], tag)
    cands = []
    for k, p in enumerate(addrs):
        r = raw[k * len(fields):(k + 1) * len(fields)]
        if not all(full(x, 4 if j == 3 else 1) for j, x in enumerate(r)):
            continue
        res = r[3]
        if word(res, 0) == 0 or word(res, 1) == 0 or addr_word(r[0]).lower() != fac.lower():
            continue
        cands.append({
            "pair": p, "factory": addr_word(r[0]), "token0": addr_word(r[1]), "token1": addr_word(r[2]),
            "getReserves": res, "stableSwap": word(r[4]) == 1,
            "precisionMultiplier0": str(word(r[5])), "precisionMultiplier1": str(word(r[6])),
            "fees": [word(res, 2), word(res, 3)],
        })
    stable = [c for c in cands if c["stableSwap"]]
    if not stable:
        # Выборка и SetStableSwap не дали стабильных пар — полный просмотр allPairs: stableSwap() каждой пары.
        sampled = set(idx)
        rest = [i for i in range(n) if i not in sampled]
        print(f"стабильных в выборке нет — полный просмотр {len(rest)} пар", file=sys.stderr)
        more = [addr_word(x) for x in node.calls([(fac, S["allPairs(uint256)"] + enc_uint(i)) for i in rest], tag)
                if full(x, 1)]
        flags = node.calls([(p, S["stableSwap()"]) for p in more], tag)
        extra2 = [p for p, f in zip(more, flags) if full(f, 1) and word(f) == 1]
        raw2 = node.calls([(p, S[f]) for p in extra2 for f in fields], tag)
        for k, p in enumerate(extra2):
            r = raw2[k * len(fields):(k + 1) * len(fields)]
            if not all(full(x, 4 if j == 3 else 1) for j, x in enumerate(r)):
                continue
            res = r[3]
            if word(res, 0) == 0 or word(res, 1) == 0 or addr_word(r[0]).lower() != fac.lower():
                continue
            stable.append({
                "pair": p, "factory": addr_word(r[0]), "token0": addr_word(r[1]), "token1": addr_word(r[2]),
                "getReserves": res, "stableSwap": True,
                "precisionMultiplier0": str(word(r[5])), "precisionMultiplier1": str(word(r[6])),
                "fees": [word(res, 2), word(res, 3)],
            })
        print(f"стабильных с резервами после полного просмотра: {len(stable)}", file=sys.stderr)
    chosen, seen = stable[: a.pairs // 2], set()
    for c in cands:
        if len(chosen) >= a.pairs:
            break
        key = tuple(c["fees"])
        if not c["stableSwap"] and key not in seen:
            seen.add(key)
            chosen.append(c)
    for c in cands:
        if len(chosen) >= a.pairs:
            break
        if not c["stableSwap"] and c not in chosen:
            chosen.append(c)
    print(f"пар {n}, просмотрено {len(addrs)}, с резервами {len(cands)}, стабильных {len(stable)}, "
          f"записано {len(chosen)}", file=sys.stderr)

    divs = [int(x) for x in a.divs.split(",")]
    for c in chosen:
        dec = node.calls([(c["token0"], S["decimals()"]), (c["token1"], S["decimals()"])], tag)
        c["decimals"] = [word(x) if full(x, 1) else None for x in dec]
        gp = node.calls([(fac, S["getPair(address,address)"] + enc_addr(c["token0"]) + enc_addr(c["token1"]))], tag)
        c["getPair"] = addr_word(gp[0]) if full(gp[0], 1) else None
        code = node.one("eth_getCode", [c["pair"], tag])
        if code in ("0x", ""):
            raise RuntimeError(f"у пары {c['pair']} нет кода на блоке {b}")
        cb = bytes.fromhex(code[2:])
        c["code_sha256"], c["code_len"] = hashlib.sha256(cb).hexdigest(), len(cb)
        items, meta = [], []
        for side, (tin, rin) in enumerate([(c["token0"], word(c["getReserves"], 0)),
                                           (c["token1"], word(c["getReserves"], 1))]):
            for d in divs:
                amt = rin // d
                if amt == 0:
                    continue
                items.append((c["pair"], S["getAmountOut(uint256,address)"] + enc_uint(amt) + enc_addr(tin)))
                meta.append((side, amt))
        outs = node.calls(items, tag)
        c["getAmountOut"] = [[s, str(amt), (str(word(o)) if o is not None else None)] for (s, amt), o in zip(meta, outs)]
        del c["fees"]

    lo = max(0, b - a.log_blocks)
    sync = node.one("eth_getLogs", [{"fromBlock": hex(lo), "toBlock": tag,
                                     "address": [c["pair"] for c in chosen], "topics": [T["sync"]]}])
    day = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    out = os.path.join(a.out, f"arbitrum_camelot_v2_{day}")
    os.makedirs(out, exist_ok=True)
    probe = {
        "chain_id": chain, "block": b, "block_hash": blk["hash"], "block_timestamp": int(blk["timestamp"], 16),
        "factory": fac, "all_pairs_length": n, "scanned": len(addrs), "with_reserves": len(cands),
        "stable_found": len(stable), "pairs": chosen,
    }
    logs = {"sync": sync, "fee_percent_updated": rare["fee"], "set_stable_swap": rare["stable"],
            "pair_created": rare["created"], "topics": T, "sync_from_block": lo}
    files = {"probe.json": probe, "logs.json": logs}
    sums = []
    for name, obj in files.items():
        data = json.dumps(obj, indent=1).encode()
        with open(os.path.join(out, name), "wb") as fh:
            fh.write(data)
        sums.append(f"{hashlib.sha256(data).hexdigest()}  {name}")
    readme = f"""# Camelot V2 — запись своего узла Arbitrum One ({day}, решение 443)

Записано скриптом tools/arbitrum_record/camelot_v2.py (только чтение, без ключей и подписей).
Узел: chainId {chain}. Блок B = {b}, хеш {blk['hash']}, время {int(blk['timestamp'], 16)}.
Фабрика: {fac}; allPairsLength = {n}; просмотрено {len(addrs)} пар, с резервами {len(cands)}, стабильных
{len(stable)}; записано {len(chosen)} пар.

## Файлы
- probe.json — пары на блоке B: factory/token0/token1, getReserves (сырой ответ), stableSwap, precisionMultiplier0/1,
  decimals, getPair фабрики, sha256 и длина кода пары, getAmountOut [сторона (0 — вход token0), вход, выход или
  null — откат] на суммах «резерв входа / d», d = {a.divs}.
- logs.json — Sync выбранных пар (блоки {lo}..{b}), FeePercentUpdated и SetStableSwap пар этой фабрики, PairCreated
  фабрики (поиск назад от B частями по {a.chunk} блоков, до {a.rare} штук каждого).

## SHA-256 файлов
""" + "\n".join(sums) + "\n"
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as fh:
        fh.write(readme)
    print(out)


if __name__ == "__main__":
    main()
