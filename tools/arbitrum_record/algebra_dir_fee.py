#!/usr/bin/env python3
"""Запись ответов своего узла Arbitrum для вида пула algebra_dir_fee (решение 444, задача T433-F5).

Что пишет (всё — чтение, eth_call / eth_getLogs, ничего не отправляется, ключей и подписей нет) в
tests/data/arbitrum_algebra_dir_fee_<дата UTC>/:
  meta.json     — chainId, web3_clientVersion, блок B (номер, хеш, время), аргументы запуска;
  factory.json  — poolDeployer() фабрики и проверка CREATE2: адрес каждого пула против хешей кода пула
                  (POOL_INIT_CODE_HASH версий ветки direction-based-fee — аргументы --init-hash);
  pool_<адрес>.json — на блоке B: token0/token1, poolByPair, globalState, liquidity, tickSpacing,
                  timepoints(timepointIndex), слова tickTable вокруг тика (--words в каждую сторону), ticks() по
                  установленным битам, balanceOf пула, ответы квотеров (Quoter v1 и QuoterV2 ветки по --quoter;
                  виртуальный квотер бота — код contracts/out/VirtualQuoter.bin-runtime подменой по адресу)
                  на суммы --bps (доли баланса входа пула, в 1/10000) в обе стороны; каждый квотер — дважды:
                  на блоке B как есть и с blockOverrides.time = время последней точки оракула пула (тогда обмен
                  не пересчитывает комиссии — AlgebraPool.sol:777-795);
  logs.json     — eth_getLogs пулов за [B − --log-blocks, B] (все события: Swap/Mint/Burn/Fee);
  geckoterminal_pools.json — ответ GeckoTerminal, если пулы взяты оттуда (--gecko-dex);
  README.md     — что записано, блок, SHA-256 файлов.

Селекторы и topic0 считает сам узел (web3_sha3) из текстов сигнатур — числа в скрипт не зашиты.
Сигнатуры — cryptoalgebra/AlgebraV1.9, ветка direction-based-fee, коммит a45417ed
(src/core/contracts/interfaces/pool/IAlgebraPoolState.sol, IAlgebraPoolEvents.sol, IAlgebraFactory.sol,
src/periphery/contracts/interfaces/IQuoter.sol, IQuoterV2.sol); квотер бота — contracts/VirtualQuoter.sol.

Запуск — README рядом (tools/arbitrum_record/README.md).
"""
import argparse
import datetime
import hashlib
import json
import os
import sys

import requests


class Node:
    """JSON-RPC узла по HTTP."""

    def __init__(self, url):
        self.url, self.n = url, 0

    def call(self, method, params):
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method,
                                          "params": params}, timeout=60)
        r.raise_for_status()
        return r.json()

    def ok(self, method, params):
        j = self.call(method, params)
        if "error" in j:
            raise RuntimeError(f"{method}: {j['error']}")
        return j["result"]

    def sha3(self, data_hex):
        return self.ok("web3_sha3", [data_hex])

    def sel(self, sig):
        return self.sha3("0x" + sig.encode().hex())[:10]


def word(v):
    return hex(v)[2:].rjust(64, "0") if isinstance(v, int) else v.lower().replace("0x", "").rjust(64, "0")


def words(raw):
    raw = raw[2:]
    return [raw[i:i + 64] for i in range(0, len(raw), 64)]


def int24(w):
    v = int(w, 16) & 0xFFFFFF
    return v - (1 << 24) if v >= 1 << 23 else v


def addr(w):
    return "0x" + w[-40:]


def main():
    a = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    a.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    a.add_argument("--factory", required=True, help="AlgebraFactory DEX (официальный адрес)")
    a.add_argument("--quoter", help="квотер DEX (официальный адрес), необязательно")
    a.add_argument("--pool", action="append", default=[], help="адрес пула (можно несколько)")
    a.add_argument("--gecko-dex", help="id DEX GeckoTerminal: пулы — первая страница его списка пулов")
    a.add_argument("--gecko-pools", type=int, help="сколько пулов взять из GeckoTerminal")
    a.add_argument("--init-hash", action="append", required=True, help="POOL_INIT_CODE_HASH версии (несколько)")
    a.add_argument("--words", type=int, required=True, help="слов tickTable в каждую сторону от слова тика")
    a.add_argument("--bps", type=int, action="append", required=True, help="сумма = баланс входа × bps / 10000")
    a.add_argument("--log-blocks", type=int, required=True, help="глубина eth_getLogs в блоках")
    a.add_argument("--vq-runtime", required=True, help="contracts/out/VirtualQuoter.bin-runtime")
    a.add_argument("--out", required=True, help="папка tests/data")
    o = a.parse_args()
    n = Node(o.rpc)
    head = n.ok("eth_getBlockByNumber", ["latest", False])
    tag = head["number"]
    utc = datetime.timezone.utc
    day = datetime.datetime.fromtimestamp(int(head["timestamp"], 16), utc).strftime("%Y-%m-%d")
    out = os.path.join(o.out, f"arbitrum_algebra_dir_fee_{day}")
    os.makedirs(out, exist_ok=True)
    files = {}

    def save(name, obj):
        files[name] = obj
        with open(os.path.join(out, name), "w") as f:
            json.dump(obj, f, indent=1)

    def eth_call(to, data, extra=None):
        p = [{"to": to, "data": data}, tag] + (extra or [])
        return n.call("eth_call", p)

    def must(j):
        if "error" in j:
            raise RuntimeError(f"eth_call: {j['error']}")
        return j

    pools = [p.lower() for p in o.pool]
    if o.gecko_dex:
        u = f"https://api.geckoterminal.com/api/v2/networks/arbitrum/dexes/{o.gecko_dex}/pools?page=1"
        g = requests.get(u, timeout=60, headers={"Accept": "application/json"}).json()
        save("geckoterminal_pools.json", {"url": u, "response": g})
        pools += [d["attributes"]["address"].lower() for d in g["data"][:o.gecko_pools]]
    if not pools:
        sys.exit("нет пулов: --pool или --gecko-dex/--gecko-pools")
    save("meta.json", {"chainId": n.ok("eth_chainId", []), "client": n.call("web3_clientVersion", []),
                       "block": {k: head[k] for k in ("number", "hash", "timestamp")},
                       "args": {k: v for k, v in vars(o).items() if k != "rpc"}})
    s = {k: n.sel(k) for k in [
        "poolDeployer()", "poolByPair(address,address)", "token0()", "token1()", "globalState()", "liquidity()",
        "tickSpacing()", "timepoints(uint256)", "tickTable(int16)", "ticks(int24)", "balanceOf(address)",
        "quoteExactInputSingle(address,address,uint256,uint160)",
        "quoteExactInputSingle((address,address,uint256,uint160))", "quoteExactIn(address,bool,uint256)"]}
    deployer = addr(words(must(eth_call(o.factory, s["poolDeployer()"]))["result"])[0])
    vq_addr = "0x" + n.sha3("0x" + b"arb.virtual_quoter".hex())[-40:]
    vq_code = "0x" + open(o.vq_runtime).read().strip().replace("0x", "")
    fac = {"factory": o.factory, "poolDeployer": deployer, "init_hashes": o.init_hash, "pools": {}}
    for p in pools:
        rec = {"pool": p}
        r = lambda sig, args="": must(eth_call(p, s[sig] + args))  # noqa: E731
        t0 = addr(words(r("token0()")["result"])[0])
        t1 = addr(words(r("token1()")["result"])[0])
        rec.update(token0=t0, token1=t1)
        rec["poolByPair"] = eth_call(o.factory, s["poolByPair(address,address)"] + word(t0) + word(t1))
        salt = n.sha3("0x" + word(t0) + word(t1))
        fac["pools"][p] = {h: "0x" + n.sha3("0xff" + deployer[2:] + salt[2:] + h.replace("0x", ""))[-40:]
                           for h in o.init_hash}
        for k in ("globalState()", "liquidity()", "tickSpacing()"):
            rec[k] = r(k)
        gs = words(rec["globalState()"]["result"])
        tick, tp_index = int24(gs[1]), int(gs[4], 16)
        rec["timepoints"] = r("timepoints(uint256)", word(tp_index))
        w0 = tick >> 8
        rec["tickTable"], rec["ticks"] = {}, {}
        for w in range(w0 - o.words, w0 + o.words + 1):
            res = r("tickTable(int16)", word(w & ((1 << 256) - 1)))
            rec["tickTable"][str(w)] = res
            bits = int(res["result"], 16)
            for b in range(256):
                if bits >> b & 1:
                    t = w * 256 + b
                    rec["ticks"][str(t)] = r("ticks(int24)", word(t & ((1 << 256) - 1)))
        bal = {t: int(must(eth_call(t, s["balanceOf(address)"] + word(p)))["result"], 16) for t in (t0, t1)}
        rec["balances"] = {t: hex(v) for t, v in bal.items()}
        last_ts = int(words(rec["timepoints"]["result"])[1], 16)
        rec["quotes"] = []
        for zfo in (True, False):
            tin, tout = (t0, t1) if zfo else (t1, t0)
            for bps in o.bps:
                amt = bal[tin] * bps // 10000
                if amt == 0:
                    continue
                q = {"zero_for_one": zfo, "bps": bps, "amount_in": hex(amt)}
                for name, over in (("at_block", []), ("at_last_timepoint", [{}, {"time": hex(last_ts)}])):
                    vq = [{vq_addr: {"code": vq_code}}] + ([over[1]] if over else [])
                    q[name] = {"virtual": eth_call(vq_addr, s["quoteExactIn(address,bool,uint256)"] + word(p)
                                                   + word(int(zfo)) + word(amt), vq)}
                    if o.quoter:
                        args = word(tin) + word(tout) + word(amt) + word(0)
                        q[name]["quoter_v1"] = eth_call(
                            o.quoter, s["quoteExactInputSingle(address,address,uint256,uint160)"] + args, over)
                        q[name]["quoter_v2"] = eth_call(
                            o.quoter, s["quoteExactInputSingle((address,address,uint256,uint160))"] + args, over)
                rec["quotes"].append(q)
        rec["last_timepoint_ts"] = last_ts
        save(f"pool_{p}.json", rec)
    save("factory.json", fac)
    lo = hex(max(0, int(tag, 16) - o.log_blocks))
    save("logs.json", {"fromBlock": lo, "toBlock": tag, "address": pools,
                       "response": n.call("eth_getLogs", [{"fromBlock": lo, "toBlock": tag, "address": pools}])})
    lines = [f"# Algebra V1.9 с направленными комиссиями — запись своего узла Arbitrum ({day})", "",
             f"Блок B = {int(tag, 16)} ({head['hash']}), время {int(head['timestamp'], 16)}.",
             "Записано скриптом tools/arbitrum_record/algebra_dir_fee.py (только чтение; ключей и подписей нет).",
             f"Фабрика {o.factory}, poolDeployer {deployer}, пулы: {', '.join(pools)}.",
             "Состав файлов — шапка скрипта.", "", "## SHA-256 файлов"]
    for name in sorted(files):
        lines.append(hashlib.sha256(open(os.path.join(out, name), "rb").read()).hexdigest() + "  " + name)
    open(os.path.join(out, "README.md"), "w").write("\n".join(lines) + "\n")
    print(out)


if __name__ == "__main__":
    main()
