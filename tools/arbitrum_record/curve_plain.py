#!/usr/bin/env python3
# Файл: tools/arbitrum_record/curve_plain.py
# Что делает: решение 472 (T450-02, T472-01, T472-02) — запись своего узла Arbitrum для видов curve_stableswap_plain и
#   curve_stableswap_meta (Curve StableSwap 2021): 2pool 0x7f90…F353 и пулы фабрики Curve Sidechain/L2 на сверенных
#   реализациях (plain: Basic, Balances, ETH, Optimized, BasicEMA, ETHEMA; мета: MetaUSD*, MetaFraxUSD* —
#   curve_plain_pools.py). Только чтение (eth_call, eth_getCode, eth_getLogs, eth_getBlockByNumber), без ключей,
#   подписей и транзакций. Пишет tests/data/arbitrum_curve_plain_<дата>/ (meta.json, pools.json, code.json, logs.json,
#   README.md). Тест — tests/arbitrum_curve_plain_live.rs (#[ignore] до записи).
# Кто вызывает: Тигран (record_all.py → record_steps.py) или вручную.
# От чего зависит: Python 3.8+ (стандартная библиотека), curve_plain_pools.py, curve_sim.py, curve_crypto_rpc.py,
#   v4live_rpc.py / v4live_slots.py, stdhttp.py. Узел — аргумент --node.
# Настройки: нет. Адреса — официальные (curve_plain_pools.py).
#
# Порядок (узел хранит состояние ~128 блоков — всё чтение состояния на свежем блоке сразу, без долгих операций перед
#   ним): 1) список пулов фабрики, реализации и базовые пулы метапулов (на «latest», список только растёт); 2) слоты
#   баланса / разрешения монет для симуляции (curve_sim.py, на «latest»: место слота от блока не зависит); 3) голова B;
#   4) ОДНИМ пакетом на B — ответы на вызовы бота по (адрес, calldata) и сверочные геттеры рампы; 5) пакетом на B —
#   decimals монет (кроме метки ETH) и get_dy(i, j, dx) на долях баланса входа (--fractions); 6) ОДНИМ пакетом на B —
#   симуляции exchange(int128,int128,uint256,uint256) на тех же суммах (curve_sim.py: подмена баланса и разрешения у
#   пустого отправителя; выход или откат пула — «модель говорит откат ↔ пул откатывает»); 7) eth_getCode 2pool и
#   реализаций (селектор exchange 4 аргументов — шаг CURVE исполнителя); 8) логи пулов узким окном [B − --log-blocks, B].
# Потолок времени — --deadline: истёк — пишет, что успел, с отметкой в meta.json.
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
from curve_crypto_rpc import selector, word  # noqa: E402
from curve_plain_pools import (FACTORY, MAX_COINS, META, NATIVE, PLAIN, POOL_2, Batch, bot_calls, call,  # noqa: E402
                               factory_pools, key)
from curve_sim import SENDER, Sim  # noqa: E402

EXCHANGE4 = "exchange(int128,int128,uint256,uint256)"


def coins_of(rec):
    out = []
    for k in range(MAX_COINS):
        x = rec["answers"].get(key(rec["pool"], selector("coins(uint256)") + word(k)), {})
        if not x.get("result"):
            break
        out.append("0x" + x["result"][-40:])
    return out


def main():
    ap = argparse.ArgumentParser(description="Запись узла Arbitrum: Curve StableSwap 2021 (решение 472)")
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--max-pools", type=int, default=200, help="пулов фабрики (с конца списка)")
    ap.add_argument("--fractions", default="0.0001,0.01,0.2", help="доли баланса входа для get_dy и симуляции")
    ap.add_argument("--log-blocks", type=int, default=20000, help="окно логов пулов до B")
    ap.add_argument("--deadline", type=int, default=600, help="потолок времени скрипта, с")
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    t0 = time.time()
    b = Batch(a.node)
    chain = b.one("eth_chainId", [])["result"]
    count, fpools = factory_pools(b, a.max_pools)
    pools = [(POOL_2, None, "plain", None)] + fpools
    # Слоты монет для симуляции — до блока B (на «latest»): coins(k) пулов на «latest».
    sim = Sim(a.node, "latest")
    if not sim.empty():
        print("ОШИБКА: отправитель симуляции %s не пуст (код или баланс)" % SENDER, file=sys.stderr)
        sys.exit(2)
    pre = b.run([call(p, selector("coins(uint256)") + word(k), "latest") for p, *_ in pools for k in range(MAX_COINS)])
    for n, (p, *_) in enumerate(pools):
        for x in pre[n * MAX_COINS:(n + 1) * MAX_COINS]:
            if not x.get("result"):
                break
            c = "0x" + x["result"][-40:]
            if c.lower() != NATIVE:
                sim.prefill(c, p)
    head = b.one("eth_getBlockByNumber", ["latest", False])["result"]
    tag = head["number"]
    sim.tag = tag
    reqs, keys = [], []
    for p, _, _, base in pools:
        for to, d in bot_calls(p, base):
            reqs.append(call(to, d, tag))
            keys.append((p, key(to, d)))
    got = b.run(reqs)
    recs = {p: {"pool": p, "implementation": imp, "kind": k, "base": base, "answers": {}} for p, imp, k, base in pools}
    for (p, kk), x in zip(keys, got):
        recs[p]["answers"][kk] = x
    dec_reqs, dec_keys, dy_reqs, dy_keys = [], [], [], []
    for p, *_ in pools:
        r = recs[p]
        r["coins"] = coins = coins_of(r)
        for c in coins:
            if c.lower() != NATIVE:
                dec_reqs.append(call(c, selector("decimals()"), tag))
                dec_keys.append((p, c))
        bal = [int(r["answers"][key(p, selector("balances(uint256)") + word(k))].get("result") or "0x0", 16)
               for k in range(len(coins))]
        for i in range(len(coins)):
            for j in range(len(coins)):
                if i != j:
                    for f in a.fractions.split(","):
                        dx = max(1, int(bal[i] * float(f)))
                        d = selector("get_dy(int128,int128,uint256)") + word(i) + word(j) + word(dx)
                        dy_reqs.append(call(p, d, tag))
                        dy_keys.append((p, i, j, dx))
                        sim.add(p, EXCHANGE4, i, j, dx, coins[i])
    for (p, c), x in zip(dec_keys, b.run(dec_reqs)):
        recs[p].setdefault("decimals", {})[c] = x
    for (p, i, j, dx), x in zip(dy_keys, b.run(dy_reqs)):
        recs[p].setdefault("get_dy", []).append({"i": i, "j": j, "dx": str(dx), **x})
    for s in sim.run():
        recs[s["pool"]].setdefault("exchange_sim", []).append(s)
    late = time.time() - t0 > a.deadline
    code = {}
    for addr in [POOL_2] + PLAIN + META:
        c = b.one("eth_getCode", [addr, tag])
        h = c.get("result") or ""
        code[addr] = {"len": len(h) // 2 - 1, "sha256": hashlib.sha256(h.encode()).hexdigest(),
                      "has_exchange4": selector(EXCHANGE4)[2:] in h, "error": c.get("error")}
    frm = hex(max(0, int(tag, 16) - a.log_blocks))
    logs = b.one("eth_getLogs", [{"address": [p for p, *_ in pools], "fromBlock": frm, "toBlock": tag}])
    write(a, chain, tag, head, count, pools, recs, code, frm, logs, late, t0)


def write(a, chain, tag, head, count, pools, recs, code, frm, logs, late, t0):
    """Файлы записи и README с SHA-256."""
    files = {
        "meta.json": {"chainId": chain, "block": int(tag, 16), "block_hex": tag,
                      "timestamp": int(head["timestamp"], 16), "factory": FACTORY, "factory_pool_count": count,
                      "plain_implementations": PLAIN, "meta_implementations": META, "sim_sender": SENDER,
                      "pools": [p for p, *_ in pools], "fractions": a.fractions,
                      "deadline_passed": late, "seconds": round(time.time() - t0, 1)},
        "pools.json": [recs[p] for p, *_ in pools],
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
        f.write("Записано tools/arbitrum_record/curve_plain.py (решение 472): только чтение, без ключей и транзакций.\n")
        f.write("Блок B = %d, chainId %s, пулов %d (2pool + фабрика, всего в фабрике %d); симуляции exchange — eth_call\n"
                "с подменой состояния от пустого адреса %s.\n" % (int(tag, 16), chain, len(pools), count, SENDER))
        f.write("Файлы и порядок записи — шапка скрипта.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    print("записано в", out, file=sys.stderr)


if __name__ == "__main__":
    main()
