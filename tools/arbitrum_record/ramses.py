#!/usr/bin/env python3
"""Файл: tools/arbitrum_record/ramses.py
Что делает: решение 483 — запись ответов своего узла Arbitrum One для трёх DEX Ramses (варианты видов бота):
  Ramses CL (v3_fork_ramses, пулы RamsesV2Pool фабрики 0xAA2c…edf8), Ramses CL V2 (slipstream_ramses, RamsesV3Pool
  фабрики RamsesV3Factory 0xd001…d28b, QuoterV2 0x00d4…E124) и Ramses Legacy V2 (solidly_v1_ramses, PairFactory
  0xADd3…Dc3B). Только чтение: eth_call (в том числе с переопределением кода — проба обмена, ramses_calls.py),
  eth_getLogs, eth_getBlockByNumber; ключей и подписей нет.
  Порядок (решение 460): сначала долгое (списки пулов GeckoTerminal, логи окном --log-blocks), затем свежий блок B и всё
  чтение состояния на нём (recnode.fresh: состояние B удалено узлом — заново на новом блоке).
  Пишет tests/data/arbitrum_ramses_<дата>/: record.json (блок B, пулы, ответы, пробы обмена, квотер, логи) и README.md
  (блок, хеш блока, sha256 файла).
  По пулу CL: factory, token0, token1, fee(), currentFee() (только старая линия), tickSpacing, slot0, liquidity,
  реестр фабрики (getPool(t0, t1, fee) / getPool(t0, t1, int24 tickSpacing)), слова tickBitmap ±--words и ticks()
  установленных тиков, балансы токенов пула, проба обмена swap в обе стороны на суммах «баланс входа пула / d» (d из
  --divs) и (линия V3) QuoterV2.quoteExactInputSingle на тех же суммах. По паре легаси: metadata, getReserves, fee(),
  stable, isPair и getPair(t0, t1, stable) фабрики, getAmountOut на суммах «резерв / d» в обе стороны.
  Логи (окно --log-blocks до головы): FeeAdjustment(uint24,uint24) и Mint пулов обеих CL-линий, Sync(uint112,uint112)
  пар легаси, PairCreated фабрики легаси — для тестов разбора событий.
Кто вызывает: Тигран вручную или record_all.py (record_steps.py).
От чего зависит: Python 3.8+, stdhttp.py, recnode.py, gt.py (через stdhttp), camelot_v2.py (keccak256), ramses_calls.py.
Источники адресов: PairFactory, RamsesV3Factory, QuoterV2 — www.ramses.xyz/docs/contract-addresses (вкладка Arbitrum,
  20.11.2026); фабрика старой CL-линии 0xAA2cd7477c451E703f3B9Ba5663334914763edf8 — на официальной странице её нет:
  факт — адаптер DefiLlama «ramses-cl» и Sourcify 42161 exact_match (RamsesTransparentUpgradeableProxy → RamsesV2Factory
  0xF896d16fa56A625802b6013F9f9202790ec69908); запись сверяет factory() каждого пула. Пулы — GeckoTerminal ramses-v2,
  ramses-v3-arbitrum (решение 483: таблица 1, строки 43, 81); пары легаси — allPairs(i) фабрики.
Запуск: python3 tools/arbitrum_record/ramses.py --rpc http://УЗЕЛ:8547 [--max-pools 10] [--out tests/data]
"""

import argparse
import datetime
import hashlib
import json
import os
import sys

import recnode
import stdhttp as requests
from camelot_v2 import topic
from ramses_calls import a_of, addr, balance, bitmap_and_ticks, call, probe, w, word

CL_FACTORY = "0xAA2cd7477c451E703f3B9Ba5663334914763edf8"
V3_FACTORY = "0xd0019e86edB35E1fedaaB03aED5c3c60f115d28b"
V3_QUOTER = "0x00d4FeA3Dd90C4480992f9c7Ea13b8a6A8F7E124"
PAIR_FACTORY = "0xADd32480630A16dfAcEe6eeFcB3ab2181449Dc3B"
GT = "https://api.geckoterminal.com/api/v2/networks/arbitrum/dexes/{}/pools"
T_FEE = topic("FeeAdjustment(uint24,uint24)")
T_MINT_V2 = topic("Mint(address,address,int24,int24,uint128,uint256,uint256)")
T_MINT_V3 = topic("Mint(address,address,uint256,int24,int24,uint128,uint256,uint256)")
T_SYNC = topic("Sync(uint112,uint112)")
T_PAIR = topic("PairCreated(address,address,address,uint256)")


class Rpc:
    """JSON-RPC узла. raw — тело ответа; call — {"result"} или {"error"}. Ошибки: сеть, StaleState (stdhttp)."""

    def __init__(self, url, deadline):
        self.url, self.n, self.deadline = url, 0, deadline

    def raw(self, method, params):
        self.deadline.check(method)
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params},
                          timeout=self.deadline.http(120))
        r.raise_for_status()
        return r.json()

    def call(self, to, data, tag):
        body = self.raw("eth_call", [{"to": to, "data": "0x" + data}, tag])
        return {"error": body["error"]} if "error" in body else {"result": body["result"]}


def gt_pools(dex, n):
    """Адреса пулов DEX у GeckoTerminal (страница 1, не больше n). Возвращает: (адреса, ответ). Ошибок нет."""
    r = requests.get(GT.format(dex), params={"page": 1}, headers={"Accept": "application/json"}, timeout=60)
    try:
        data = json.loads(r.text).get("data") or []
    except ValueError:
        data = []
    return [x["attributes"]["address"] for x in data][:n], {"status": r.status_code, "count": len(data)}


def logs(rpc, head, addrs, topics, span):
    """Логи адресов с topic0 из списка за span блоков до головы (окно делится при таймауте узла). Ошибок нет: сбой —
    {"error"}."""
    if not addrs:
        return []
    fetch = lambda a, b: rpc.raw("eth_getLogs", [{"fromBlock": hex(a), "toBlock": hex(b), "address": addrs,
                                                  "topics": [topics]}]).get("result") or []
    try:
        return recnode.logs_split(fetch, max(0, head - span), head, 2000, deadline=rpc.deadline)
    except Exception as e:  # noqa: BLE001 — причина в запись
        return {"error": str(e)}


def cl_pool(rpc, p, v2line, a, tag):
    """Пул CL одной линии на блоке tag (шапка). Ошибок нет: откаты — в записи."""
    x = {"pool": p}
    names = ["factory()", "token0()", "token1()", "fee()", "tickSpacing()", "slot0()", "liquidity()"]
    if v2line:
        names.append("currentFee()")
    for n in names:
        x[n] = call(rpc, p, n, "", tag)
    t0, t1 = a_of(x["token0()"]), a_of(x["token1()"])
    ts, tick = w(x["tickSpacing()"], 0, True), w(x["slot0()"], 1, True)
    if None in (t0, t1, ts, tick):
        return x
    key = word(w(x["fee()"])) if v2line else word(ts)
    reg = "getPool(address,address,uint24)" if v2line else "getPool(address,address,int24)"
    x["getPool"] = call(rpc, CL_FACTORY if v2line else V3_FACTORY, reg, addr(t0) + addr(t1) + key, tag)
    if not v2line:
        x["tickSpacingInitialFee"] = call(rpc, V3_FACTORY, "tickSpacingInitialFee(int24)", word(ts), tag)
    x["map"] = bitmap_and_ticks(rpc, p, tick, ts, a.words, "ticks(int24)", tag)
    x["balances"] = [balance(rpc, t0, p, tag), balance(rpc, t1, p, tag)]
    x["swaps"] = []
    for zfo, tin, tout, bal in ((True, t0, t1, x["balances"][0]), (False, t1, t0, x["balances"][1])):
        for d in a.divs:
            amt = (bal or 0) // d
            if amt <= 0:
                continue
            q = {"zeroForOne": zfo, "amountIn": str(amt), "probe": probe(rpc, p, zfo, amt, tag)}
            if not v2line:
                q["quoter"] = call(rpc, V3_QUOTER, "quoteExactInputSingle((address,address,uint256,int24,uint160))",
                                   addr(tin) + addr(tout) + word(amt) + word(ts) + word(0), tag)
            x["swaps"].append(q)
    return x


def legacy(rpc, a, tag):
    """Пары легаси на блоке tag: allPairs(i) с конца, не больше --max-pools. Ошибок нет: откаты — в записи."""
    rec = {"allPairsLength": call(rpc, PAIR_FACTORY, "allPairsLength()", "", tag), "pairs": []}
    n = w(rec["allPairsLength"]) or 0
    for i in range(max(0, n - a.max_pools), n):
        p = a_of(call(rpc, PAIR_FACTORY, "allPairs(uint256)", word(i), tag))
        if not p:
            continue
        x = {"pair": p, "index": i}
        for k in ("metadata()", "getReserves()", "fee()", "stable()", "token0()", "token1()", "factory()"):
            x[k] = call(rpc, p, k, "", tag)
        t0, t1, st = a_of(x["token0()"]), a_of(x["token1()"]), w(x["stable()"])
        x["isPair"] = call(rpc, PAIR_FACTORY, "isPair(address)", addr(p), tag)
        if None not in (t0, t1, st):
            x["getPair"] = call(rpc, PAIR_FACTORY, "getPair(address,address,bool)", addr(t0) + addr(t1) + word(st), tag)
        r0, r1 = w(x["getReserves()"], 0) or 0, w(x["getReserves()"], 1) or 0
        x["quotes"] = []
        for tok, r in ((t0, r0), (t1, r1)):
            for d in a.divs:
                amt = r // d
                if tok and amt > 0:
                    q = call(rpc, p, "getAmountOut(uint256,address)", word(amt) + addr(tok), tag)
                    x["quotes"].append({"tokenIn": tok, "amountIn": str(amt), "out": q})
        rec["pairs"].append(x)
    return rec


def main():
    ap = argparse.ArgumentParser(description="Запись пулов Ramses своим узлом Arbitrum (решение 483)")
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--max-pools", type=int, default=10)
    ap.add_argument("--words", type=int, default=4)
    ap.add_argument("--divs", type=int, nargs="+", default=[100000, 10000, 1000, 100])
    ap.add_argument("--log-blocks", type=int, default=2000000)
    ap.add_argument("--time-limit", type=int, default=900)
    ap.add_argument("--out", default="tests/data")
    a = ap.parse_args()
    rpc = Rpc(a.rpc, recnode.Deadline(a.time_limit))
    cl, cl_gt = gt_pools("ramses-v2", a.max_pools)
    v3, v3_gt = gt_pools("ramses-v3-arbitrum", a.max_pools)
    head0 = int(rpc.raw("eth_blockNumber", [])["result"], 16)
    lg = {"fromHead": head0, "span": a.log_blocks,
          "cl": logs(rpc, head0, cl, [T_FEE, T_MINT_V2], a.log_blocks),
          "v3": logs(rpc, head0, v3, [T_FEE, T_MINT_V3], a.log_blocks),
          "pairCreated": logs(rpc, head0, [PAIR_FACTORY], [T_PAIR], a.log_blocks)}

    def body(head):
        tag = head["number"]
        leg = legacy(rpc, a, tag)
        return {"block": int(tag, 16), "blockHash": head["hash"], "timestamp": int(head["timestamp"], 16),
                "gt": {"ramses-v2": cl_gt, "ramses-v3-arbitrum": v3_gt},
                "cl": [cl_pool(rpc, p, True, a, tag) for p in cl],
                "v3": [cl_pool(rpc, p, False, a, tag) for p in v3], "legacy": leg}

    rec = recnode.fresh(lambda: rpc.raw("eth_getBlockByNumber", ["latest", False])["result"], body)
    pairs = [x["pair"] for x in rec["legacy"]["pairs"]]
    lg["sync"] = logs(rpc, rec["block"], pairs, [T_SYNC], a.log_blocks)
    rec["logs"] = lg
    day = os.environ.get("ARB_RECORD_DATE") or datetime.datetime.utcnow().date().isoformat()
    d = os.path.join(a.out, "arbitrum_ramses_" + day)
    os.makedirs(d, exist_ok=True)
    raw = json.dumps(rec, indent=1, sort_keys=True).encode()
    with open(os.path.join(d, "record.json"), "wb") as f:
        f.write(raw)
    with open(os.path.join(d, "README.md"), "w", encoding="utf-8") as f:
        f.write("# Запись пулов Ramses (решение 483)\n\nСкрипт: tools/arbitrum_record/ramses.py. Узел Arbitrum One, "
                "состояние — на одном блоке B; логи — окно до головы.\n\n"
                f"- блок: {rec['block']}\n- хеш блока: {rec['blockHash']}\n"
                f"- record.json sha256: {hashlib.sha256(raw).hexdigest()}\n"
                f"- пулов: CL {len(rec['cl'])}, CL V2 {len(rec['v3'])}, пар легаси {len(pairs)}\n")
    print("записано:", d)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as e:  # узел недоступен, потолок времени — причина и код 1
        print("ошибка:", e)
        sys.exit(1)
