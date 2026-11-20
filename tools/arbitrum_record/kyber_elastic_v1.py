#!/usr/bin/env python3
# Файл: tools/arbitrum_record/kyber_elastic_v1.py
# Что делает: запись фактов своего узла Arbitrum для KyberSwap Elastic v1 (решение 451, задача T448-06 / T433-006) —
#   только чтение (eth_call, eth_getLogs, eth_getBlockByNumber), без ключей и подписей. Пишет JSON-файлы в
#   tests/data/arbitrum_kyber_elastic_v1_<дата>/ и README.md (что записано, блок, хеши файлов). По ним идут тесты
#   tests/arbitrum_kyber_elastic_v1_live.rs (сейчас #[ignore] — записи нет).
# Кто вызывает: Тигран на своём сервере (узел Arbitrum), вручную.
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py, решение 459). Узел — аргумент --rpc (HTTP JSON-RPC). GeckoTerminal — по желанию (--gt).
# Настройки: нет (адреса — аргументы; по умолчанию — официальные: KyberNetwork/ks-elastic-sc 4ab08c0a README.md L6-L17,
#   «Deployed contracts for KyberSwap Elastic v1», Arbitrum; Sourcify 42161 — full match).
#
# Что записывается (на одном блоке B = «latest» на момент запуска, номер — в meta.json):
#   meta.json     — eth_chainId, номер и время блока B, адреса, аргументы запуска;
#   gt_pools.json — ответы GeckoTerminal /networks/arbitrum/dexes/kyberswap_elastic_arbitrum/pools?page=N (как есть);
#   pools.json    — по каждому пулу: token0/token1/factory/swapFeeUnits/tickDistance/getPoolState/getLiquidityState
#                   (сырой hex), фабрика getPool(token0, token1, swapFeeUnits), TicksFeesReader.getTicksInRange(pool,
#                   MIN_TICK, 0) — как вызывает бот (pools/load_elastic.rs; откат/ошибка узла — текст), ticks(t) каждого
#                   тика из ответа, decimals токенов, QuoterV2.quoteExactInputSingle на суммы --sizes в обе стороны
#                   (limitSqrtP = 0; сырой hex; откат — текст);
#   logs.json     — eth_getLogs пулов за блоки [B − --log-blocks, B] (Swap/Mint/Burn/BurnRTokens и прочие).
# Селекторы (keccak256 сигнатур, посчитаны при написании): getPoolState() 217ac237, getLiquidityState() ab612f2b,
#   swapFeeUnits() c79a590e, tickDistance() 48626a8c, token0() 0dfe1681, token1() d21220a7, factory() c45a0155,
#   ticks(int24) f30dba93, getTicksInRange(address,int24,uint32) 6bea4865, getPool(address,address,uint24) 1698ee82,
#   quoteExactInputSingle((address,address,uint256,uint24,uint160)) c6a5026a, decimals() 313ce567.
#
# Запуск: python3 tools/arbitrum_record/kyber_elastic_v1.py --rpc http://127.0.0.1:8547 --gt [--pool 0x…]

import argparse
import datetime
import hashlib
import json
import os
import sys

import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests

FACTORY = "0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a"
QUOTER = "0x0D125c15D54cA1F8a813C74A81aEe34ebB508C1f"
READER = "0x165c68077ac06c83800d19200e6E2B08D02dE75D"
GT = "https://api.geckoterminal.com/api/v2/networks/arbitrum/dexes/kyberswap_elastic_arbitrum/pools"
MIN_TICK = -887272
MOD = 1 << 256


class Rpc:
    """JSON-RPC узла: один запрос — один вызов; ошибка узла — исключение с текстом."""

    def __init__(self, url):
        self.url = url
        self.n = 0

    def call(self, method, params):
        self.n += 1
        r = requests.post(self.url, json={"jsonrpc": "2.0", "id": self.n, "method": method, "params": params},
                          timeout=120)
        r.raise_for_status()
        body = r.json()
        if "error" in body:
            raise RuntimeError(json.dumps(body["error"]))
        return body["result"]

    def eth_call(self, to, data, tag):
        return self.call("eth_call", [{"to": to, "input": "0x" + data}, tag])


def word(v):
    """Число (в т.ч. отрицательное — дополнение до 2^256) → 32 байта hex без 0x."""
    return format(v % MOD, "064x")


def addr_word(a):
    return word(int(a, 16))


def try_call(rpc, to, data, tag):
    """eth_call → {"ok": hex} или {"error": текст} (откат пишется, а не роняет запись)."""
    try:
        return {"ok": rpc.eth_call(to, data, tag)}
    except Exception as e:
        return {"error": str(e)}


def gt_pages(pages):
    out = []
    for page in range(1, pages + 1):
        r = requests.get(GT, params={"page": page}, headers={"Accept": "application/json"}, timeout=60)
        out.append({"page": page, "status": r.status_code, "body": r.text})
        if r.status_code != 200 or not json.loads(r.text).get("data"):
            break
    return out


def int24_list(hexdata):
    """Разбор int24[] из ответа ABI: смещение, длина, слова."""
    raw = hexdata[2:]
    n = int(raw[64:128], 16)
    out = []
    for i in range(n):
        v = int(raw[128 + 64 * i:192 + 64 * i], 16)
        out.append(v - MOD if v >> 255 else v)
    return out


def record_pool(rpc, pool, args, tag):
    rec = {"pool": pool}
    for name, sel in [("token0", "0dfe1681"), ("token1", "d21220a7"), ("factory", "c45a0155"),
                      ("swapFeeUnits", "c79a590e"), ("tickDistance", "48626a8c"), ("getPoolState", "217ac237"),
                      ("getLiquidityState", "ab612f2b")]:
        rec[name] = try_call(rpc, pool, sel, tag)
    t0 = "0x" + rec["token0"]["ok"][-40:]
    t1 = "0x" + rec["token1"]["ok"][-40:]
    fee = int(rec["swapFeeUnits"]["ok"], 16)
    rec["factory_getPool"] = try_call(rpc, args.factory, "1698ee82" + addr_word(t0) + addr_word(t1) + word(fee), tag)
    rec["decimals"] = {t: try_call(rpc, t, "313ce567", tag) for t in (t0, t1)}
    rng = try_call(rpc, args.reader, "6bea4865" + addr_word(pool) + word(MIN_TICK) + word(0), tag)
    rec["getTicksInRange"] = rng
    ticks = {}
    if "ok" in rng:
        for t in int24_list(rng["ok"]):
            ticks[str(t)] = try_call(rpc, pool, "f30dba93" + word(t), tag)
            if t >= -MIN_TICK:
                break
    rec["ticks"] = ticks
    quotes = []
    for a, b in ((t0, t1), (t1, t0)):
        dec = int(rec["decimals"][a]["ok"], 16)
        for s in args.sizes.split(","):
            amount = int(float(s) * 10 ** dec)
            data = "c6a5026a" + addr_word(a) + addr_word(b) + word(amount) + word(fee) + word(0)
            q = {"tokenIn": a, "tokenOut": b, "amountIn": str(amount)}
            q.update(try_call(rpc, args.quoter, data, tag))
            quotes.append(q)
    rec["quotes"] = quotes
    return rec


def main():
    ap = argparse.ArgumentParser(description="Запись фактов узла Arbitrum для KyberSwap Elastic v1 (решение 451)")
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--gt", action="store_true", help="записать пулы GeckoTerminal kyberswap_elastic_arbitrum")
    ap.add_argument("--gt-pages", type=int, default=10)
    ap.add_argument("--pool", action="append", default=[], help="адрес пула (0x…); можно несколько")
    ap.add_argument("--max-pools", type=int, default=30, help="не больше стольких пулов")
    ap.add_argument("--sizes", default="0.001,0.01,0.1,1,10,100,1000", help="суммы входа в единицах токена")
    ap.add_argument("--log-blocks", type=int, default=2000, help="сколько блоков логов пулов до B")
    ap.add_argument("--factory", default=FACTORY)
    ap.add_argument("--quoter", default=QUOTER)
    ap.add_argument("--reader", default=READER)
    ap.add_argument("--out", default=None, help="папка вывода (по умолчанию tests/data/arbitrum_kyber_elastic_v1_<дата>)")
    args = ap.parse_args()

    rpc = Rpc(args.rpc)
    chain = rpc.call("eth_chainId", [])
    head = rpc.call("eth_getBlockByNumber", ["latest", False])
    b, tag = int(head["number"], 16), head["number"]
    day = datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = args.out or os.path.join(root, "tests", "data", "arbitrum_kyber_elastic_v1_" + day)
    os.makedirs(out, exist_ok=True)
    files = {}
    meta = {"chainId": chain, "block": b, "block_hex": tag, "timestamp": int(head["timestamp"], 16),
            "factory": args.factory, "quoter": args.quoter, "reader": args.reader, "sizes": args.sizes,
            "log_blocks": args.log_blocks}
    files["meta.json"] = meta

    ids = list(args.pool)
    if args.gt:
        pages = gt_pages(args.gt_pages)
        files["gt_pools.json"] = pages
        for p in pages:
            if p["status"] == 200:
                for item in json.loads(p["body"]).get("data", []):
                    a = item.get("attributes", {}).get("address", "")
                    if a.startswith("0x") and len(a) == 42:
                        ids.append(a)
    pools = []
    for pool in list(dict.fromkeys(i.lower() for i in ids))[:args.max_pools]:
        try:
            pools.append(record_pool(rpc, pool, args, tag))
        except Exception as e:
            pools.append({"pool": pool, "error": str(e)})
    files["pools.json"] = pools
    frm = hex(max(0, b - args.log_blocks))
    addrs = [p["pool"] for p in pools if "error" not in p]
    files["logs.json"] = {"from": frm, "to": tag,
                          "logs": rpc.call("eth_getLogs", [{"address": addrs, "fromBlock": frm, "toBlock": tag}])
                          if addrs else []}

    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# KyberSwap Elastic v1 (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/kyber_elastic_v1.py (решение 451): только чтение, без ключей и подписей.\n")
        f.write("Блок B = %d (%s), время %d, chainId %s. Пулов: %d. Узел: %s.\n\n" % (
            b, tag, meta["timestamp"], chain, len(pools), args.rpc.split("@")[-1]))
        f.write("Файлы — шапка скрипта. Фабрика %s, QuoterV2 %s, TicksFeesReader %s.\n\n" % (
            args.factory, args.quoter, args.reader))
        f.write("## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    print("записано в", out, file=sys.stderr)


if __name__ == "__main__":
    main()
