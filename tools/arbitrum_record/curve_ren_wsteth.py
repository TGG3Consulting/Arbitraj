#!/usr/bin/env python3
# Файл: tools/arbitrum_record/curve_ren_wsteth.py
# Что делает: решение 480 (T472-10) — запись своего узла Arbitrum для двух пулов Curve без фабрики, исходники которых
#   найдены в Sourcify (docs/sources/curve_arbitrum/): WBTC/renBTC 0x3E01…F0Fb (вид curve_stableswap_plain — та же
#   математика, что у 2pool) и ETH/wstETH 0x6eB2…7d80 (вид curve_stableswap_oracle — курс wstETH от оракула пула).
#   Только чтение (eth_call, eth_getCode, eth_getBlockByNumber), без ключей, подписей и транзакций. Пишет
#   tests/data/arbitrum_curve_ren_wsteth_<дата>/ (meta.json, pools.json, code.json, README.md). Тест —
#   tests/arbitrum_curve_ren_wsteth_live.rs (#[ignore] до записи).
# Кто вызывает: Тигран (record_all.py → record_steps.py) или вручную.
# От чего зависит: Python 3.8+ (стандартная библиотека), curve_plain_pools.py (Batch, bot_calls, call, key),
#   curve_sim.py (Sim), curve_crypto_rpc.py (selector, word), recnode.py (fresh), stdhttp.py. Узел — аргумент --node.
# Настройки: нет. Адреса — официальные: curvefi/curve-js ab3f1c2 src/constants/pools/arbitrum.ts L98 (ren), L152
#   (wsteth).
#
# Порядок (узел хранит состояние ~128 блоков — решение 460): 1) на «latest» — oracle() пула wstETH (immutable — от
#   блока не зависит), слоты баланса / разрешения монет для симуляции (curve_sim.py); 2) фаза состояния на свежем блоке
#   B (recnode.fresh: узел уже удалил состояние — фаза целиком заново на новом блоке): ОДНИМ пакетом — ответы на вызовы
#   бота по (адрес, calldata) (load_curve_plain.rs calls; у wstETH ещё latestAnswer() оракула — load_curve_oracle.rs
#   calls) и сверочные геттеры рампы, oracle(); пакетом — decimals монет (кроме метки ETH) и get_dy(i, j, dx) на долях
#   баланса входа (--fractions); ОДНИМ пакетом — симуляции exchange(int128,int128,uint256,uint256) на тех же суммах
#   (вход ETH — value, выход ETH — raw_call пустому отправителю); eth_getCode обоих пулов и оракула.
#
# Запуск: python3 tools/arbitrum_record/curve_ren_wsteth.py --node http://УЗЕЛ:8547

import argparse
import datetime
import hashlib
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import recnode  # noqa: E402
from curve_crypto_rpc import selector, word  # noqa: E402
from curve_plain_pools import MAX_COINS, NATIVE, Batch, bot_calls, call, key  # noqa: E402
from curve_sim import SENDER, Sim  # noqa: E402

REN = "0x3E01dD8a5E1fb3481F0F589056b428Fc308AF0Fb"
WSTETH = "0x6eB2dc694eB516B16Dc9FBc678C60052BbdD7d80"
EXCHANGE4 = "exchange(int128,int128,uint256,uint256)"


def say(text):
    print(text, file=sys.stderr, flush=True)


def coins_of(rec):
    """Монеты пула по ответам coins(k) записи (до первого отката)."""
    out = []
    for k in range(MAX_COINS):
        x = rec["answers"].get(key(rec["pool"], selector("coins(uint256)") + word(k)), {})
        if not x.get("result"):
            break
        out.append("0x" + x["result"][-40:])
    return out


def state_phase(b, sim, oracle, fractions, head):
    """Фаза состояния на блоке head (объект блока узла). Возвращает: (head, recs, code). StaleState — наружу
    (recnode.fresh начнёт заново на новом блоке)."""
    tag = head["number"]
    sim.tag, sim.reqs, sim.recs = tag, [], []
    pools = [(REN, "plain", None), (WSTETH, "oracle", oracle)]
    reqs, keys = [], []
    for p, _, o in pools:
        calls = bot_calls(p) + [(p, selector("oracle()"))]
        if o:
            calls.append((o, selector("latestAnswer()")))
        for to, d in calls:
            reqs.append(call(to, d, tag))
            keys.append((p, key(to, d)))
    got = b.run(reqs)
    recs = {p: {"pool": p, "kind": k, "oracle": o, "answers": {}} for p, k, o in pools}
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
                if i == j:
                    continue
                for f in fractions.split(","):
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
    code = {}
    for addr in [REN, WSTETH] + ([oracle] if oracle else []):
        c = b.one("eth_getCode", [addr, tag])
        h = c.get("result") or ""
        code[addr] = {"len": len(h) // 2 - 1, "sha256": hashlib.sha256(h.encode()).hexdigest(),
                      "has_exchange4": selector(EXCHANGE4)[2:] in h,
                      "has_oracle": selector("oracle()")[2:] in h, "error": c.get("error")}
    return head, recs, code


def main():
    ap = argparse.ArgumentParser(description="Запись узла Arbitrum: Curve ren и wstETH (решение 480)")
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--fractions", default="0.0001,0.01,0.2", help="доли баланса входа для get_dy и симуляции")
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    t0 = time.time()
    b = Batch(a.node)
    chain = b.one("eth_chainId", [])["result"]
    o = b.one(*call(WSTETH, selector("oracle()"), "latest"))
    oracle = "0x" + o["result"][-40:] if o.get("result") else None
    if not oracle:
        say("ОШИБКА: oracle() пула wstETH не ответил: %s" % o)
    sim = Sim(a.node, "latest")
    if not sim.empty():
        say("ОШИБКА: отправитель симуляции %s не пуст (код или баланс)" % SENDER)
        sys.exit(2)
    pre = b.run([call(p, selector("coins(uint256)") + word(k), "latest") for p in (REN, WSTETH) for k in range(2)])
    for n, p in enumerate([REN, REN, WSTETH, WSTETH]):
        c = "0x" + pre[n]["result"][-40:] if pre[n].get("result") else None
        if c and c.lower() != NATIVE:
            sim.prefill(c, p)
    head, recs, code = recnode.fresh(lambda: b.one("eth_getBlockByNumber", ["latest", False])["result"],
                                     lambda h: state_phase(b, sim, oracle, a.fractions, h), say=say)
    write(a, chain, head, oracle, recs, code, t0)


def write(a, chain, head, oracle, recs, code, t0):
    """Файлы записи и README с SHA-256."""
    tag = head["number"]
    files = {
        "meta.json": {"chainId": chain, "block": int(tag, 16), "block_hex": tag,
                      "timestamp": int(head["timestamp"], 16), "pools": [REN, WSTETH], "oracle": oracle,
                      "sim_sender": SENDER, "fractions": a.fractions, "seconds": round(time.time() - t0, 1)},
        "pools.json": [recs[REN], recs[WSTETH]],
        "code.json": code,
    }
    day = datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = a.out or os.path.join(root, "tests", "data", "arbitrum_curve_ren_wsteth_" + day)
    os.makedirs(out, exist_ok=True)
    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# Curve WBTC/renBTC и ETH/wstETH (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/curve_ren_wsteth.py (решение 480): только чтение, без ключей и "
                "транзакций.\nБлок B = %d, chainId %s, оракул wstETH %s; симуляции exchange — eth_call с подменой "
                "состояния от пустого адреса %s.\n" % (int(tag, 16), chain, oracle, SENDER))
        f.write("Файлы и порядок записи — шапка скрипта.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    say("записано в " + out)


if __name__ == "__main__":
    main()
