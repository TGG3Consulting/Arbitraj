#!/usr/bin/env python3
"""
Запись ответов своего узла Arbitrum One для семейства Liquidity Book V2.2 — LFJ V2.2 (решение 446, T433-F7 / T433-067).
Только чтение (eth_chainId, eth_getBlockByNumber, eth_call, eth_getStorageAt, eth_getCode, eth_getLogs) — без ключей,
подписей и отправки транзакций.

Что пишет в tests/data/arbitrum_lb_v22_<дата UTC>/:
  probe.json — блок B (номер, хеш, время, chainId), фабрика (getLBPairImplementation, getNumberOfLBPairs), пулы — всё
               на одном блоке B, каждый вызов: calldata и сырой ответ (hex) или текст отката:
               getFactory, getTokenX, getTokenY, getBinStep, getActiveId, getStaticFeeParameters,
               getVariableFeeParameters, getReserves, implementation, getLBHooksParameters; реестр фабрики
               getLBPairInformation(X, Y, binStep); слова хранилища eth_getStorageAt 0..8 (раскладка: _parameters — 3,
               _reserves — 4); код клона (eth_getCode); все непустые бины (обход getNextNonEmptyBin от активного в обе
               стороны до края или --max-bins, getBin каждого); getSwapOut на суммах в обе стороны (резерв входа
               пула / d для d из --divs и uint128 max — она больше ликвидности: остаток входа > 0).
  logs.json  — реальные логи выбранных пулов за последние --log-blocks блоков до B (Swap, DepositedToBins, ... — все
               topic0) и логи HooksParametersSet всех пулов фабрики (поиск назад от B частями по --chunk блоков, пока не
               наберётся --rare штук или не пройдено --rare-blocks блоков).
  README.md  — что записано, блок, sha256 файлов.

Как выбираются пулы: getNumberOfLBPairs фабрики, getLBPairAtIndex(i) для i от последнего назад (--scan штук), из них
пулы с ненулевыми резервами обеих сторон (getReserves) — первые --pools штук; --pool АДРЕС (можно несколько) —
добавить свои.

Источник ABI — lfj-gg/joe-v2@v2.2.0 (1297c38): src/interfaces/ILBPair.sol, src/interfaces/ILBFactory.sol; адрес
фабрики — документация LFJ (lfj-gg/joe-docs 61c1666, versioned_docs/version-V2.2/deployment-addresses/arbitrum.md:
LBFactory 0xb43120c4745967fa9b93E79C149E66B0f2D6Fe0c). Селекторы и topic0 — keccak256 подписей (посчитаны при
написании; тест tests/arb_dex_lb_v22_live.rs сверяет записанную calldata с ABI кода бота).

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
    rec["storage"] = {str(i): node.rpc("eth_getStorageAt", [pool, hex(i), tag]) for i in range(9)}
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
    chosen = [p.lower() for p in a.pool]
    for i in range(total - 1, max(total - 1 - a.scan, -1), -1):
        if len(chosen) >= len(a.pool) + a.pools:
            break
        r = node.call(factory, "0x" + SEL["getLBPairAtIndex"] + word(i), tag)
        if "out" not in r:
            continue
        p = "0x" + r["out"][-40:]
        res = node.call(p, "0x" + SEL["getReserves"], tag)
        if "out" in res and u(res["out"], 0) > 0 and u(res["out"], 1) > 0 and p not in chosen:
            chosen.append(p)
    probe["pools"] = []
    for p in chosen:
        print(f"пул {p}", file=sys.stderr)
        probe["pools"].append(pool_record(node, p, factory, tag, divs, a.max_bins))

    logs = {"block": b, "pool_logs": [], "hooks_set": []}
    frm = max(b - a.log_blocks, 0)
    if chosen:
        logs["pool_logs"] = node.rpc("eth_getLogs", [{"address": chosen, "fromBlock": hex(frm), "toBlock": tag}])
    logs["pool_logs_from"] = frm
    hi, searched = b, 0
    while len(logs["hooks_set"]) < a.rare and hi > 0 and searched < a.rare_blocks:
        lo = max(hi - a.chunk + 1, 0)
        got = node.rpc("eth_getLogs", [{"topics": [HOOKS_SET_TOPIC], "fromBlock": hex(lo), "toBlock": hex(hi)}])
        logs["hooks_set"].extend(got)
        searched += hi - lo + 1
        hi = lo - 1
    logs["hooks_set_searched_from"] = hi + 1

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
