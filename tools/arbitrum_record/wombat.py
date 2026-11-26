#!/usr/bin/env python3
# Файл: tools/arbitrum_record/wombat.py
# Что делает: решение 484 — запись своего узла Arbitrum для пулов Wombat Exchange (17 пулов официальной страницы
#   docs.wombat.exchange/developers/contracts/arbitrum; Cross Chain — тоже: его реализация CrossChainPool на CoreV3 в
#   код не взята, запись — для разбора). Только чтение (eth_call, eth_getStorageAt, eth_getBlockByNumber, eth_chainId),
#   без ключей, подписей и транзакций. Пишет tests/data/arbitrum_wombat_<дата>/ (meta.json, pools.json, README.md).
#   Тест — tests/arbitrum_wombat_live.rs (#[ignore] до записи).
# Кто вызывает: Тигран (record_all.py → record_steps.py) или вручную.
# От чего зависит: Python 3.8+ (стандартная библиотека), wombat_calls.py, curve_plain_pools.py (Batch, call),
#   curve_sim.py (SENDER), recnode.py (fresh), stdhttp.py. Узел — аргумент --node.
# Настройки: нет.
#
# Порядок (узел хранит состояние ~128 блоков — решение 460): 1) на «latest» — getTokens пулов и слоты баланса /
#   разрешения монет для симуляции (curve_sim.Sim prefill); 2) фаза состояния на свежем блоке B (recnode.fresh: состояние
#   удалено — фаза целиком заново): ПАКЕТОМ — вызовы бота ступени 1 (load_wombat_calls.rs head_calls) и слоты хранилища
#   (реализация EIP-1967, _pausedAssets[монета]); ПАКЕТОМ — addressOfAsset(монета) и isPaused(монета) (у реализаций без
#   геттера — откат, пишется как есть: сверка слота 201); ПАКЕТОМ — ступень 3 у каждого актива (underlyingToken,
#   underlyingTokenDecimals, pool, cash, liability, getRelativePrice); ПАКЕТОМ — quotePotentialSwap(i → j, dx) на долях
#   cash монеты входа (--fractions); ОДНИМ ПАКЕТОМ — симуляции swap(i → j, dx, 1, W, 2^256 − 1) на тех же суммах (минимум 1 — как шаг
#   WOMBAT исполнителя).
#
# Запуск: python3 tools/arbitrum_record/wombat.py --node http://УЗЕЛ:8547

import argparse
import datetime
import hashlib
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import recnode  # noqa: E402
from curve_crypto_rpc import addr_word, selector  # noqa: E402
from curve_plain_pools import Batch, call  # noqa: E402
from curve_sim import SENDER  # noqa: E402
import wombat_calls as wc  # noqa: E402


def say(text):
    print(text, file=sys.stderr, flush=True)


def ans(rec, to, data):
    """Ответ записи на вызов (или {})."""
    return rec["answers"].get(wc.key(to, data), {})


def state_phase(b, sim, pools, tokens, fractions, head, *, block_selector=None):
    """Фаза состояния на блоке head. Возвращает: (head, recs). StaleState — наружу (recnode.fresh)."""
    tag = block_selector if block_selector is not None else head["number"]
    sim.tag, sim.reqs, sim.recs = tag, [], []
    recs = {p: {"name": n, "pool": p, "tokens": tokens.get(p, []), "answers": {}, "storage": {}} for n, p in pools}

    def run_calls(items):
        reqs = [call(to, d, tag) for _, to, d in items]
        for (p, to, d), x in zip(items, b.run(reqs)):
            recs[p]["answers"][wc.key(to, d)] = x

    def run_slots(items):
        reqs = [("eth_getStorageAt", [p, s, tag]) for p, s in items]
        for (p, s), x in zip(items, b.run(reqs)):
            recs[p]["storage"][p.lower() + "|" + s.lower()] = x

    run_calls([(p, to, d) for _, p in pools for to, d in wc.head_calls(p)])
    run_slots([(p, wc.IMPL_SLOT) for _, p in pools] + [(p, wc.paused_slot(t)) for _, p in pools for t in tokens.get(p, [])])
    items = []
    for _, p in pools:
        for t in tokens.get(p, []):
            items.append((p,) + wc.asset_call(p, t))
            items.append((p, p, selector("isPaused(address)") + addr_word(t)))
    run_calls(items)
    items = []
    for _, p in pools:
        for t in tokens.get(p, []):
            a = ans(recs[p], *wc.asset_call(p, t)).get("result")
            if a:
                items.extend((p, to, d) for to, d in wc.detail_calls("0x" + a[-40:]))
    run_calls(items)
    quotes = []
    for _, p in pools:
        r, ts = recs[p], tokens.get(p, [])
        for i, ti in enumerate(ts):
            a = ans(r, *wc.asset_call(p, ti)).get("result")
            if not a:
                continue
            asset = "0x" + a[-40:]
            cash = int(ans(r, asset, selector("cash()")).get("result") or "0x0", 16)
            dec = int(ans(r, asset, selector("underlyingTokenDecimals()")).get("result") or "0x12", 16)
            units = cash * 10 ** dec // 10 ** 18
            for j, tj in enumerate(ts):
                if i == j:
                    continue
                for f in fractions.split(","):
                    dx = max(1, int(units * float(f)))
                    quotes.append((p, i, j, ti, tj, dx))
                    sim.add_swap(p, ti, tj, dx)
    got = b.run([call(p, wc.quote_data(ti, tj, dx), tag) for p, _, _, ti, tj, dx in quotes])
    for (p, i, j, _, _, dx), x in zip(quotes, got):
        recs[p].setdefault("quote", []).append({"i": i, "j": j, "dx": str(dx), **x})
    for s in sim.run():
        p = s["pool"]
        ts = [t.lower() for t in tokens.get(p, [])]
        s["i"], s["j"] = ts.index(s["from"].lower()), ts.index(s["to"].lower())
        recs[p].setdefault("swap_sim", []).append(s)
    return head, [recs[p] for _, p in pools]


def main():
    ap = argparse.ArgumentParser(description="Запись узла Arbitrum: пулы Wombat Exchange (решение 484)")
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--fractions", default="0.0001,0.01,0.2", help="доли cash монеты входа для квотера и симуляции")
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    t0 = time.time()
    b = Batch(a.node)
    chain = b.one("eth_chainId", [])["result"]
    pools = list(wc.POOLS.items())
    sim = wc.WSim(a.node, "latest")
    if not sim.empty():
        say("ОШИБКА: отправитель симуляции %s не пуст (код или баланс)" % SENDER)
        sys.exit(2)
    pre = b.run([call(p, selector("getTokens()"), "latest") for _, p in pools])
    tokens = {}
    for (n, p), x in zip(pools, pre):
        if x.get("result"):
            tokens[p] = wc.tokens_of(x["result"])
            for t in tokens[p]:
                sim.prefill(t, p)
        else:
            say("пул %s %s: getTokens() — %s" % (n, p, x.get("error")))
    head, recs = recnode.fresh(lambda: b.one("eth_getBlockByNumber", ["latest", False])["result"],
                               lambda h: state_phase(b, sim, pools, tokens, a.fractions, h), say=say)
    write(a, chain, head, recs, t0)


def write(a, chain, head, recs, t0):
    """Файлы записи и README с SHA-256."""
    tag = head["number"]
    files = {
        "meta.json": {"chainId": chain, "block": int(tag, 16), "block_hex": tag,
                      "timestamp": int(head["timestamp"], 16), "pools": dict(wc.POOLS), "sim_sender": SENDER,
                      "fractions": a.fractions, "seconds": round(time.time() - t0, 1)},
        "pools.json": recs,
    }
    day = datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = a.out or os.path.join(root, "tests", "data", "arbitrum_wombat_" + day)
    os.makedirs(out, exist_ok=True)
    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# Wombat Exchange (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/wombat.py (решение 484): только чтение, без ключей и транзакций.\n"
                "Блок B = %d, chainId %s; симуляции swap — eth_call с подменой состояния от пустого адреса %s.\n"
                % (int(tag, 16), chain, SENDER))
        f.write("Файлы и порядок записи — шапка скрипта.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    say("записано в " + out)


if __name__ == "__main__":
    main()
