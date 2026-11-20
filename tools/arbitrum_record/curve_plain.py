#!/usr/bin/env python3
# Файл: tools/arbitrum_record/curve_plain.py
# Что делает: решение 472 (T450-02) — запись своего узла Arbitrum для вида curve_stableswap_plain (Curve StableSwap
#   2021): 2pool 0x7f90…F353 и plain-пулы фабрики Curve Sidechain/L2 на реализациях Plain2Basic / Plain3Basic /
#   Plain4Basic. Только чтение (eth_call, eth_getCode, eth_getLogs, eth_getBlockByNumber), без ключей и подписей.
#   Пишет tests/data/arbitrum_curve_plain_<дата>/ (meta.json, pools.json, code.json, logs.json, README.md). Тест —
#   tests/arbitrum_curve_plain_live.rs (#[ignore] до записи).
# Кто вызывает: Тигран (record_all.py → record_steps.py) или вручную.
# От чего зависит: Python 3.8+ (стандартная библиотека), curve_crypto_rpc.py (keccak256, selector, word, addr_word),
#   stdhttp.py. Узел — аргумент --node.
# Настройки: нет. Адреса — официальные (аргументы по умолчанию):
#   2pool 0x7f90122BF0700F9E7e1F688fe926940E8839F353 — curvefi/curve-js ab3f1c2 src/constants/pools/arbitrum.ts L15-L20;
#   фабрика 0xb17b674D9c5CB2e441F8e196a2f048A81355d031 — там же network_constants.ts L157 (ALIASES_ARBITRUM.factory);
#   реализации Plain2Basic 0x54e8…7A62, Plain3Basic 0xe381…7A62, Plain4Basic 0x8d53…fa86 — factory/stable.ts L168-L180.
#
# Порядок (узел хранит состояние ~128 блоков — состояние читается одним пакетом на свежем блоке, без долгих операций
#   перед ним): 1) список пулов фабрики (pool_count / pool_list / get_implementation_address — на «latest», список
#   только растёт); 2) голова B; 3) ОДНИМ пакетом JSON-RPC на B — ответы на вызовы бота по calldata (load_curve_plain.rs:
#   coins(k) / balances(k) k < 8, A_precise(), fee(); и A(), admin_fee(), initial_A, future_A, initial_A_time,
#   future_A_time — для сверки рампы), decimals монет; 4) вторым пакетом на B — get_dy(i, j, dx) на долях баланса
#   входа (--fractions); 5) eth_getCode пулов-неклонов и реализаций (есть ли селектор exchange(int128,int128,uint256,
#   uint256) 3df02124 — его зовёт шаг CURVE исполнителя); 6) логи пулов узким окном [B − --log-blocks, B].
# Потолок времени — --deadline (секунды на весь скрипт): истёк — пишет, что успел, с отметкой в meta.json.
#
# Запуск: python3 tools/arbitrum_record/curve_plain.py --node http://УЗЕЛ:8547

import argparse
import datetime
import hashlib
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import stdhttp as requests  # noqa: E402  решение 459
from curve_crypto_rpc import addr_word, selector, word  # noqa: E402

POOL_2 = "0x7f90122BF0700F9E7e1F688fe926940E8839F353"
FACTORY = "0xb17b674D9c5CB2e441F8e196a2f048A81355d031"
IMPLS = ["0x54e8A25d0Ac0E4945b697C80b8372445FEA17A62", "0xe381C25de995d62b453aF8B931aAc84fcCaa7A62",
         "0x8d53E5De033078367Ad91527c53abfd1Eb6bfa86"]
MAX_COINS = 8  # abi_curve.rs MAX_COINS (curve-pool-registry Registry.vy)
EXCHANGE4 = selector("exchange(int128,int128,uint256,uint256)")[2:]


class Batch:
    """JSON-RPC пакетом: список (метод, параметры) → список {"result"} / {"error"} по порядку."""

    def __init__(self, url):
        self.url = url

    def run(self, reqs, step=500):
        out = []
        for s in range(0, len(reqs), step):
            part = reqs[s:s + step]
            body = [{"jsonrpc": "2.0", "id": s + k, "method": m, "params": p} for k, (m, p) in enumerate(part)]
            r = requests.post(self.url, json=body, timeout=120)
            r.raise_for_status()
            got = {x["id"]: x for x in r.json()}
            for k in range(len(part)):
                x = got.get(s + k, {"error": "нет ответа"})
                out.append({"result": x["result"]} if "result" in x else {"error": x.get("error")})
        return out

    def one(self, method, params):
        return self.run([(method, params)])[0]


def call(to, data, tag):
    return ("eth_call", [{"to": to, "data": data}, tag])


def bot_calls(pool):
    """Calldata вызовов бота (load_curve_plain.rs calls) и сверочных геттеров."""
    out = []
    for k in range(MAX_COINS):
        out.append(selector("coins(uint256)") + word(k))
        out.append(selector("balances(uint256)") + word(k))
    for sig in ["A_precise()", "fee()", "A()", "admin_fee()", "initial_A()", "future_A()", "initial_A_time()",
                "future_A_time()"]:
        out.append(selector(sig))
    return out


def factory_pools(b, max_pools):
    """Пулы фабрики на сверенных реализациях: (пул, реализация); перечисление pool_list с конца."""
    n = b.one(*call(FACTORY, selector("pool_count()"), "latest"))
    count = int(n["result"], 16) if n.get("result") else 0
    idx = list(range(count - 1, -1, -1))
    lst = b.run([call(FACTORY, selector("pool_list(uint256)") + word(i), "latest") for i in idx])
    pools = ["0x" + x["result"][-40:] for x in lst if x.get("result")]
    imps = b.run([call(FACTORY, selector("get_implementation_address(address)") + addr_word(p), "latest")
                  for p in pools])
    out = []
    for p, x in zip(pools, imps):
        imp = "0x" + x["result"][-40:] if x.get("result") else None
        if imp and imp.lower() in [a.lower() for a in IMPLS]:
            out.append((p, imp))
        if len(out) >= max_pools:
            break
    return count, out


def main():
    ap = argparse.ArgumentParser(description="Запись узла Arbitrum: Curve StableSwap 2021 (решение 472)")
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--max-pools", type=int, default=40, help="пулов фабрики (с конца списка)")
    ap.add_argument("--fractions", default="0.000001,0.0001,0.01,0.2", help="доли баланса входа для get_dy")
    ap.add_argument("--log-blocks", type=int, default=20000, help="окно логов пулов до B")
    ap.add_argument("--deadline", type=int, default=600, help="потолок времени скрипта, с")
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    t0 = time.time()
    b = Batch(a.node)
    chain = b.one("eth_chainId", [])["result"]
    count, fpools = factory_pools(b, a.max_pools)
    pools = [(POOL_2, None)] + fpools
    head = b.one("eth_getBlockByNumber", ["latest", False])["result"]
    tag = head["number"]
    reqs, keys = [], []
    for p, _ in pools:
        for d in bot_calls(p):
            reqs.append(call(p, d, tag))
            keys.append((p, d))
    got = b.run(reqs)
    recs = {p: {"pool": p, "implementation": imp, "answers": {}} for p, imp in pools}
    for (p, d), x in zip(keys, got):
        recs[p]["answers"][d] = x
    dec_reqs, dec_keys, dy_reqs, dy_keys = [], [], [], []
    for p, _ in pools:
        r = recs[p]
        coins = []
        for k in range(MAX_COINS):
            x = r["answers"][selector("coins(uint256)") + word(k)]
            if not x.get("result"):
                break
            coins.append("0x" + x["result"][-40:])
        r["coins"] = coins
        for c in coins:
            dec_reqs.append(call(c, selector("decimals()"), tag))
            dec_keys.append((p, c))
        bal = [int(r["answers"][selector("balances(uint256)") + word(k)].get("result") or "0x0", 16)
               for k in range(len(coins))]
        for i in range(len(coins)):
            for j in range(len(coins)):
                if i == j:
                    continue
                for f in a.fractions.split(","):
                    dx = max(1, int(bal[i] * float(f)))
                    d = selector("get_dy(int128,int128,uint256)") + word(i) + word(j) + word(dx)
                    dy_reqs.append(call(p, d, tag))
                    dy_keys.append((p, i, j, dx))
    for (p, c), x in zip(dec_keys, b.run(dec_reqs)):
        recs[p].setdefault("decimals", {})[c] = x
    for (p, i, j, dx), x in zip(dy_keys, b.run(dy_reqs)):
        recs[p].setdefault("get_dy", []).append({"i": i, "j": j, "dx": str(dx), **x})
    late = time.time() - t0 > a.deadline
    code = {}
    for addr in [POOL_2] + IMPLS:
        c = b.one("eth_getCode", [addr, tag])
        h = c.get("result") or ""
        code[addr] = {"len": len(h) // 2 - 1, "sha256": hashlib.sha256(h.encode()).hexdigest(),
                      "has_exchange4": EXCHANGE4 in h, "error": c.get("error")}
    frm = hex(max(0, int(tag, 16) - a.log_blocks))
    logs = b.one("eth_getLogs", [{"address": [p for p, _ in pools], "fromBlock": frm, "toBlock": tag}])
    files = {
        "meta.json": {"chainId": chain, "block": int(tag, 16), "block_hex": tag,
                      "timestamp": int(head["timestamp"], 16), "factory": FACTORY, "factory_pool_count": count,
                      "implementations": IMPLS, "pools": [p for p, _ in pools], "fractions": a.fractions,
                      "deadline_passed": late, "seconds": round(time.time() - t0, 1)},
        "pools.json": [recs[p] for p, _ in pools],
        "code.json": code,
        "logs.json": {"from": frm, "to": tag, **logs},
    }
    day = datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = a.out or os.path.join(root, "tests", "data", "arbitrum_curve_plain_" + day)
    os.makedirs(out, exist_ok=True)
    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# Curve StableSwap 2021 (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/curve_plain.py (решение 472): только чтение, без ключей.\n")
        f.write("Блок B = %d, chainId %s, пулов %d (2pool + фабрика, всего в фабрике %d).\n" % (
            int(tag, 16), chain, len(pools), count))
        f.write("Файлы и порядок записи — шапка скрипта.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    print("записано в", out, file=sys.stderr)


if __name__ == "__main__":
    main()
