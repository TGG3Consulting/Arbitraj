#!/usr/bin/env python3
"""
Запись ответов своего узла Arbitrum One для семейства Liquidity Book V2.2 — LFJ V2.2 (решение 446, T433-F7 / T433-067).
Только чтение (eth_chainId, eth_getBlockByNumber, eth_call, eth_getStorageAt, eth_getCode, eth_getLogs) — без ключей,
подписей и отправки транзакций.

Что пишет в tests/data/arbitrum_lb_v22_<дата UTC>/:
  probe.json — блок B (номер, хеш, время, chainId), фабрика (getLBPairImplementation, getNumberOfLBPairs),
               implementations — implementation() ВСЕХ пулов фабрики (уникальный набор, число пулов, примеры), hooks —
               крючки из getLBHooksParameters всех пулов и из логов HooksParametersSet (адрес, флаги, keccak256 кода),
               подробные пулы — всё
               на одном блоке B, каждый вызов: calldata и сырой ответ (hex) или текст отката:
               getFactory, getTokenX, getTokenY, getBinStep, getActiveId, getStaticFeeParameters,
               getVariableFeeParameters, getReserves, implementation, getLBHooksParameters; реестр фабрики
               getLBPairInformation(X, Y, binStep); слова хранилища eth_getStorageAt 0..8 и 65545 (раскладка:
               _parameters — 3, _reserves — 4, _hooksParameters — 65545); код клона (eth_getCode); все непустые бины (обход getNextNonEmptyBin от активного в обе
               стороны до края или --max-bins, getBin каждого); getSwapOut на суммах в обе стороны (резерв входа
               пула / d для d из --divs и uint128 max — она больше ликвидности: остаток входа > 0).
  logs.json  — реальные логи выбранных пулов за последние --log-blocks блоков до B (Swap, DepositedToBins, ... — все
               topic0) и логи HooksParametersSet всех пулов фабрики (поиск назад от B частями по --chunk блоков, пока не
               наберётся --rare штук или не пройдено --rare-blocks блоков).
  README.md  — что записано, блок, sha256 файлов.

Как выбираются подробные пулы: --pool АДРЕС (можно несколько), все пулы с крючком (слово крючков не 0) и пулы из логов
HooksParametersSet, затем пулы с ненулевыми резервами обеих сторон с конца фабрики (из последних --scan) — --pools штук.

Источник ABI — lfj-gg/joe-v2@v2.2.0 (1297c38): src/interfaces/ILBPair.sol, src/interfaces/ILBFactory.sol; адрес
фабрики — документация LFJ (lfj-gg/joe-docs 61c1666, versioned_docs/version-V2.2/deployment-addresses/arbitrum.md:
LBFactory 0xb43120c4745967fa9b93E79C149E66B0f2D6Fe0c). Селекторы и topic0 — keccak256 подписей; скрипт сверяет их
при запуске (SIGS), тест tests/arb_dex_lb_v22_live.rs сверяет записанную calldata с ABI кода бота.

Установка:  pip install requests
Запуск:     python3 tools/arbitrum_record/lb_v22.py http://АДРЕС_УЗЛА:8547 \
                --factory 0xb43120c4745967fa9b93E79C149E66B0f2D6Fe0c
            (из корня репозитория; --out — другая папка)
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


# Селекторы функций и topic0 (keccak256 подписей ILBPair / ILBFactory v2.2.0).
SEL = {
    "getFactory": "88cc58e4",
    "getTokenX": "05e8746d",
    "getTokenY": "da10610c",
    "getBinStep": "17f11ecc",
    "getActiveId": "dbe65edc",
    "getBin": "0abe9688",  # getBin(uint24)
    "getNextNonEmptyBin": "a41a01fb",  # getNextNonEmptyBin(bool,uint24)
    "getStaticFeeParameters": "7ca0de30",
    "getVariableFeeParameters": "8d7024e5",
    "getReserves": "0902f1ac",
    "getSwapOut": "e77366f8",  # getSwapOut(uint128,bool)
    "implementation": "5c60da1b",
    "getLBHooksParameters": "781a8915",
    "getLBPairInformation": "704037bd",  # getLBPairInformation(address,address,uint256)
    "getLBPairImplementation": "af371065",
    "getNumberOfLBPairs": "4e937c3a",
    "getLBPairAtIndex": "7daf5d66",  # getLBPairAtIndex(uint256)
}
HOOKS_SET_TOPIC = "0x4d24d81a1a6e09c37497747b4c9d67d858ce80438b54df0fa361a24dc742d934"
SIGS = {
    "getFactory": "getFactory()", "getTokenX": "getTokenX()", "getTokenY": "getTokenY()",
    "getBinStep": "getBinStep()", "getActiveId": "getActiveId()", "getBin": "getBin(uint24)",
    "getNextNonEmptyBin": "getNextNonEmptyBin(bool,uint24)", "getStaticFeeParameters": "getStaticFeeParameters()",
    "getVariableFeeParameters": "getVariableFeeParameters()", "getReserves": "getReserves()",
    "getSwapOut": "getSwapOut(uint128,bool)", "implementation": "implementation()",
    "getLBHooksParameters": "getLBHooksParameters()",
    "getLBPairInformation": "getLBPairInformation(address,address,uint256)",
    "getLBPairImplementation": "getLBPairImplementation()", "getNumberOfLBPairs": "getNumberOfLBPairs()",
    "getLBPairAtIndex": "getLBPairAtIndex(uint256)",
}
for _k, _sig in SIGS.items():
    assert keccak256(_sig.encode()).hex()[:8] == SEL[_k], _k
assert "0x" + keccak256(b"HooksParametersSet(address,bytes32)").hex() == HOOKS_SET_TOPIC

STATE = [
    "getFactory", "getTokenX", "getTokenY", "getBinStep", "getActiveId", "getStaticFeeParameters",
    "getVariableFeeParameters", "getReserves", "implementation", "getLBHooksParameters",
]


def word(n: int) -> str:
    return format(n, "064x")


def addr_word(a: str) -> str:
    return a.lower().replace("0x", "").rjust(64, "0")


class Node:
    def __init__(self, url: str):
        self.url = url
        self.s = requests.Session()
        self.n = 0

    def rpc(self, method, params):
        self.n += 1
        r = self.s.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params},
                        timeout=60)
        r.raise_for_status()
        j = r.json()
        if "error" in j:
            raise RuntimeError(f"{method}: {j['error']}")
        return j["result"]

    def call(self, to: str, data: str, block: str):
        """Возвращает {'data': calldata, 'out': hex} или {'data': ..., 'revert': текст}."""
        self.n += 1
        r = self.s.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": "eth_call",
                                        "params": [{"to": to, "input": data}, block]}, timeout=60)
        r.raise_for_status()
        j = r.json()
        if "error" in j:
            return {"data": data, "revert": json.dumps(j["error"], ensure_ascii=False)}
        return {"data": data, "out": j["result"]}


    def calls(self, items, block: str, size: int = 100):
        """Пачка eth_call (JSON-RPC batch) частями по size: [(to, data)] → ответы как у call, по порядку."""
        out = []
        for off in range(0, len(items), size):
            part = items[off:off + size]
            body = []
            for to, data in part:
                self.n += 1
                body.append({"jsonrpc": "2.0", "id": self.n, "method": "eth_call",
                             "params": [{"to": to, "input": data}, block]})
            r = self.s.post(self.url, json=body, timeout=120)
            r.raise_for_status()
            got = {j["id"]: j for j in r.json()}
            for (to, data), req in zip(part, body):
                j = got[req["id"]]
                out.append({"data": data, "revert": json.dumps(j["error"], ensure_ascii=False)} if "error" in j
                           else {"data": data, "out": j["result"]})
        return out


def u(hexs: str, i: int = 0) -> int:
    h = hexs[2:]
    return int(h[64 * i:64 * i + 64], 16)


def pool_record(node: Node, pool: str, factory: str, tag: str, divs, max_bins: int):
    rec = {"pool": pool, "calls": {}}
    for name in STATE:
        rec["calls"][name] = node.call(pool, "0x" + SEL[name], tag)
    c = rec["calls"]
    if any("revert" in c[k] for k in ("getTokenX", "getTokenY", "getBinStep", "getActiveId", "getReserves")):
        return rec
    x = "0x" + c["getTokenX"]["out"][-40:]
    y = "0x" + c["getTokenY"]["out"][-40:]
    step = u(c["getBinStep"]["out"])
    active = u(c["getActiveId"]["out"])
    rec["registry"] = node.call(factory, "0x" + SEL["getLBPairInformation"] + addr_word(x) + addr_word(y)
                                + word(step), tag)
    rec["storage"] = {str(i): node.rpc("eth_getStorageAt", [pool, hex(i), tag])
                      for i in list(range(9)) + [65545]}
    code = node.rpc("eth_getCode", [pool, tag])
    rec["code"] = code
    rec["code_sha256"] = hashlib.sha256(bytes.fromhex(code[2:])).hexdigest()
    bins, complete = {}, True
    for for_y in (True, False):
        bid = active
        if for_y:
            b = node.call(pool, "0x" + SEL["getBin"] + word(bid), tag)
            bins[str(bid)] = [str(u(b["out"], 0)), str(u(b["out"], 1))]
        for _ in range(max_bins):
            n = node.call(pool, "0x" + SEL["getNextNonEmptyBin"] + word(1 if for_y else 0) + word(bid), tag)
            nxt = u(n["out"])
            if nxt == 0 or nxt == 0xFFFFFF:
                break
            b = node.call(pool, "0x" + SEL["getBin"] + word(nxt), tag)
            bins[str(nxt)] = [str(u(b["out"], 0)), str(u(b["out"], 1))]
            bid = nxt
        else:
            complete = False
    rec["bins"] = bins
    rec["bins_complete"] = complete
    rx, ry = u(c["getReserves"]["out"], 0), u(c["getReserves"]["out"], 1)
    quotes = []
    for for_y in (True, False):
        # Суммы — резерв пула на стороне входа / d (вход X — от rx, вход Y — от ry) и ещё uint128 max: она больше
        # ликвидности стороны выхода всегда (остаток входа > 0 — как откат swap LBPair__OutOfLiquidity).
        base = rx if for_y else ry
        amounts = [min(max(int(base / d), 1), (1 << 128) - 1) for d in divs] + [(1 << 128) - 1]
        for amt in amounts:
            q = node.call(pool, "0x" + SEL["getSwapOut"] + word(amt) + word(1 if for_y else 0), tag)
            quotes.append({"amountIn": str(amt), "swapForY": for_y, **q})
    rec["quotes"] = quotes
    return rec


def main():
    ap = argparse.ArgumentParser(description="Запись LB V2.2 (LFJ) со своего узла Arbitrum One")
    ap.add_argument("node", help="адрес JSON-RPC своего узла, например http://127.0.0.1:8547")
    ap.add_argument("--factory", required=True, help="LBFactory V2.2 (документация LFJ)")
    ap.add_argument("--pool", action="append", default=[], help="свой пул (можно несколько раз)")
    ap.add_argument("--pools", type=int, default=5, help="сколько пулов с резервами взять из фабрики")
    ap.add_argument("--scan", type=int, default=200, help="сколько последних индексов фабрики просмотреть")
    ap.add_argument("--divs", default="10000,1000,100,10,1", help="делители суммы getSwapOut")
    ap.add_argument("--max-bins", type=int, default=2000, help="предел обхода бинов в одну сторону")
    ap.add_argument("--log-blocks", type=int, default=20000, help="логи пулов за последние N блоков")
    ap.add_argument("--chunk", type=int, default=50000, help="размер части eth_getLogs при поиске назад")
    ap.add_argument("--rare", type=int, default=5, help="сколько логов HooksParametersSet искать")
    ap.add_argument("--rare-blocks", type=int, default=50_000_000, help="предел поиска назад, блоков")
    ap.add_argument("--out", default=None, help="папка записи (по умолчанию tests/data/arbitrum_lb_v22_<дата>)")
    a = ap.parse_args()

    node = Node(a.node)
    factory = a.factory.lower()
    divs = [float(x) for x in a.divs.split(",")]
    chain = node.rpc("eth_chainId", [])
    blk = node.rpc("eth_getBlockByNumber", ["latest", False])
    tag = blk["number"]
    b = int(tag, 16)
    probe = {"chain_id": chain, "block": b, "block_hash": blk["hash"], "timestamp": int(blk["timestamp"], 16),
             "factory": factory, "factory_calls": {
                 "getLBPairImplementation": node.call(factory, "0x" + SEL["getLBPairImplementation"], tag),
                 "getNumberOfLBPairs": node.call(factory, "0x" + SEL["getNumberOfLBPairs"], tag)}}
    total = u(probe["factory_calls"]["getNumberOfLBPairs"]["out"])

    # Все пулы фабрики (ревью 446, С4): адрес, implementation(), getLBHooksParameters(), getReserves — уникальный набор
    # реализаций и крючков всей фабрики (для dex.lb_implementations и dex.lb_hooks_*).
    idx = node.calls([(factory, "0x" + SEL["getLBPairAtIndex"] + word(i)) for i in range(total)], tag)
    every = ["0x" + r["out"][-40:] for r in idx if "out" in r]
    per = node.calls([(p, "0x" + SEL[n]) for p in every for n in ("implementation", "getLBHooksParameters",
                                                                   "getReserves")], tag)
    implementations, hooked, reserves = {}, {}, {}
    for k, p in enumerate(every):
        imp, hp, res = per[3 * k], per[3 * k + 1], per[3 * k + 2]
        key = ("0x" + imp["out"][-40:]) if "out" in imp else "откат: " + imp.get("revert", "")
        implementations.setdefault(key, []).append(p)
        if "out" in hp and int(hp["out"], 16) != 0:
            hooked[p] = hp["out"]
        if "out" in res:
            reserves[p] = (u(res["out"], 0), u(res["out"], 1))
    probe["implementations"] = {k: {"pools": len(v), "sample": v[:5]} for k, v in implementations.items()}

    logs = {"block": b, "pool_logs": [], "hooks_set": []}
    hi, searched = b, 0
    while len(logs["hooks_set"]) < a.rare and hi > 0 and searched < a.rare_blocks:
        lo = max(hi - a.chunk + 1, 0)
        got = node.rpc("eth_getLogs", [{"topics": [HOOKS_SET_TOPIC], "fromBlock": hex(lo), "toBlock": hex(hi)}])
        logs["hooks_set"].extend(got)
        searched += hi - lo + 1
        hi = lo - 1
    logs["hooks_set_searched_from"] = hi + 1

    # Крючки: из слов пулов и из логов HooksParametersSet — адрес, флаги, keccak256 кода на блоке B.
    words = dict(hooked)
    for l in logs["hooks_set"]:
        words.setdefault(l["address"].lower() + "@log" + l["blockNumber"], l["data"][:66])
    hooks = {}
    for src, w in words.items():
        v = int(w, 16)
        h = "0x" + format(v & ((1 << 160) - 1), "040x")
        e = hooks.setdefault(h, {"flags_bits_160_169": [], "pools": []})
        e["flags_bits_160_169"].append(format((v >> 160) & 0x3FF, "010b"))
        e["pools"].append(src)
    for h, e in hooks.items():
        if int(h, 16) == 0:
            continue
        code = node.rpc("eth_getCode", [h, tag])
        e["code_keccak256"] = "0x" + keccak256(bytes.fromhex(code[2:])).hex()
        e["code_len"] = (len(code) - 2) // 2
    probe["hooks"] = hooks

    # Выборка для подробной записи: свои (--pool), пулы с крючком и пулы из логов HooksParametersSet, затем пулы с
    # резервами обеих сторон с конца фабрики (--pools штук).
    chosen = [p.lower() for p in a.pool]
    extra = list(hooked) + [l["address"].lower() for l in logs["hooks_set"]]
    for p in extra:
        if p not in chosen:
            chosen.append(p)
    want = len(chosen) + a.pools
    for p in reversed(every[-a.scan:] if a.scan else every):
        if len(chosen) >= want:
            break
        rx, ry = reserves.get(p, (0, 0))
        if rx > 0 and ry > 0 and p not in chosen:
            chosen.append(p)
    probe["pools"] = []
    for p in chosen:
        print(f"пул {p}", file=sys.stderr)
        probe["pools"].append(pool_record(node, p, factory, tag, divs, a.max_bins))

    frm = max(b - a.log_blocks, 0)
    if chosen:
        logs["pool_logs"] = node.rpc("eth_getLogs", [{"address": chosen, "fromBlock": hex(frm), "toBlock": tag}])
    logs["pool_logs_from"] = frm

    day = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    out = a.out or os.path.join("tests", "data", f"arbitrum_lb_v22_{day}")
    os.makedirs(out, exist_ok=True)
    files = {"probe.json": probe, "logs.json": logs}
    sums = {}
    for name, obj in files.items():
        text = json.dumps(obj, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        sums[name] = hashlib.sha256(text.encode("utf-8")).hexdigest()
    readme = [
        f"# Liquidity Book V2.2 (LFJ V2.2, Arbitrum One) — ответы своего узла, {day}",
        "",
        "Записано tools/arbitrum_record/lb_v22.py (только чтение; без ключей и подписей).",
        f"- chainId {chain}, блок {b} (хеш {blk['hash']}, время {probe['timestamp']});",
        f"- фабрика {factory}: getNumberOfLBPairs = {total}, getLBPairImplementation = "
        f"{probe['factory_calls']['getLBPairImplementation'].get('out', 'откат')};",
        f"- пулов: {len(probe['pools'])} — " + ", ".join(p["pool"] for p in probe["pools"]) + ";",
        f"- реализации пулов фабрики (implementation(), все {len(every)} пулов): "
        + "; ".join(f"{k} — {v['pools']} пулов" for k, v in probe["implementations"].items()) + ";",
        f"- крючки (слова пулов и логи HooksParametersSet): " + (", ".join(
            f"{h} (keccak256 кода {e.get('code_keccak256', '—')})" for h, e in probe["hooks"].items()) or "нет") + ";",
        f"- logs.json: логи пулов с блока {frm} до {b} ({len(logs['pool_logs'])} шт.), HooksParametersSet фабрики — "
        f"{len(logs['hooks_set'])} шт. (поиск с блока {logs['hooks_set_searched_from']});",
        "",
        "sha256 файлов (UTF-8 текста):",
    ] + [f"- {k}: {v}" for k, v in sums.items()] + [
        "",
        "Используется: tests/arb_dex_lb_v22_live.rs (решение 446). Реализации пулов (implementation()) — сверить код",
        "с lfj-gg/joe-v2@v2.2.0 (Sourcify / Arbiscan) и внести в dex.lb_implementations.",
    ]
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("\n".join(readme) + "\n")
    print(f"записано в {out}; запросов к узлу: {node.n}", file=sys.stderr)


if __name__ == "__main__":
    main()
