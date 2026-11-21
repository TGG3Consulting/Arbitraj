#!/usr/bin/env python3
# Файл: tools/arbitrum_record/kyber_elastic_v1.py
# Что делает: запись фактов своего узла Arbitrum для KyberSwap Elastic (решение 451, задача T448-06 / T433-006;
#   решение 462 — разбор прогона 20.11.2026) — только чтение (eth_call, eth_getLogs, eth_getBlockByNumber), без ключей.
#   Пишет JSON-файлы в tests/data/arbitrum_kyber_elastic_v1_<дата>/ и README.md. По pools.json (пулы v1) идут тесты
#   tests/arbitrum_kyber_elastic_v1_live.rs (сейчас #[ignore]).
# Кто вызывает: record_all.py (record_steps.py: --rpc --gt) или Тигран вручную.
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py), recnode.py, kyber_elastic_ticks.py. GeckoTerminal — --gt.
# Настройки: нет. Адреса — официальные (решение 462):
#   v1 («Elastic Legacy»): Factory 0x5F1d…D50a, QuoterV2 0x0D12…8C1f, TicksFeesReader 0x165c…E75D — KyberNetwork/ks-elastic-sc
#     4ab08c0a README.md L6-L17; docs.kyberswap.com reference/legacy/elastic-legacy/elastic-contract-addresses (Arbitrum);
#   v2 («KyberSwap Elastic»): Factory 0xC7a5…8C4A, QuoterV2 0x4d47…7856, TicksFeesReader 0x8Fd8…DbB3 — docs.kyberswap.com
#     reference/legacy/kyberswap-elastic/contracts/elastic-contract-addresses (13 сетей, в т.ч. Arbitrum).
#
# Решение 462. Прогон 20.11: все 20 пулов GeckoTerminal kyberswap_elastic_arbitrum — factory() = 0xC7a5…8C4A (v2), а
#   квотер был v1: QuoterV2 v1 считает адрес пула от СВОЕЙ фабрики (QuoterV2.sol L40-L45 _getPool → PoolAddress.computeAddress)
#   — пула v1 с такой парой/комиссией нет (getPool v1 = 0 у 14) → вызов в адрес без кода, откат «0x»; у 6 пар пул v1 есть —
#   квотер котировал ДРУГОЙ пул (v1), а не пул записи. Теперь: версия пула — по factory(), квотер и читатель — своей версии;
#   пулы v1 — factory() = v1 у пулов GeckoTerminal / --pool и getPool фабрики v1 для пар GeckoTerminal (ненулевой ответ).
#   getTicksInRange(pool, MIN_TICK, 0) — как вызывает бот (load_elastic.rs; ответ или ошибка пишется как есть); 20.11 у
#   4 пулов (tickDistance 8 и 10) — «out of gas»: массив maxNumTicks = (MAX_TICK−1)/tickDistance·2+3 слов (TicksFeesReader.sol
#   L46-L61) — 221 820 / 177 457 слов, стоимость памяти 3a + a²/512 (Yellow Paper) ≈ 96 / 62 млн газа. Поэтому тики ещё и
#   частями getTicksInRange(pool, start, --tick-chunk) по связному списку (ticks_chunks) — тики есть у каждого пула.
#   Логи — после фазы состояния за --log-blocks (20.11: 2000 блоков — 0 событий; GeckoTerminal в тот момент: 0 обменов
#   за m30 по всем 20 пулам) — окно больше, таймаут узла — пополам (recnode.logs_split).
#
# Что записывается (состояние — на одном блоке B):
#   meta.json        — chainId, блок B и его время, адреса v1/v2, аргументы;
#   gt_pools.json    — ответы GeckoTerminal /networks/arbitrum/dexes/kyberswap_elastic_arbitrum/pools?page=N (как есть);
#   pools.json       — пулы v1 (factory() = v1): token0/token1/factory/swapFeeUnits/tickDistance/getPoolState/
#                      getLiquidityState (сырой hex), getPool своей фабрики, getTicksInRange(pool, MIN_TICK, 0),
#                      ticks_chunks, ticks(t), decimals, QuoterV2.quoteExactInputSingle на --sizes в обе стороны;
#   pools_v2.json    — то же для пулов v2 (factory() = v2) с квотером и читателем v2;
#   pools_other.json — кандидаты с другой фабрикой или без ответа factory();
#   logs.json        — eth_getLogs пулов v1 и v2 за [B − --log-blocks, B] (+ время границ окна).
# Селекторы — keccak256 сигнатур (шапка kyber_elastic_ticks.py).
#
# Запуск: python3 tools/arbitrum_record/kyber_elastic_v1.py --rpc http://127.0.0.1:8547 --gt [--pool 0x…]

import argparse
import datetime
import hashlib
import json
import os
import sys

import recnode  # решение 460: свежий блок состояния, деление окна логов
import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests
from kyber_elastic_ticks import Rpc, addr_word, record_pool, try_call, word

VERSIONS = {
    "v1": {"factory": "0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a", "quoter": "0x0D125c15D54cA1F8a813C74A81aEe34ebB508C1f",
           "reader": "0x165c68077ac06c83800d19200e6E2B08D02dE75D"},
    "v2": {"factory": "0xC7a590291e07B9fe9E64b86c58fD8fC764308C4A", "quoter": "0x4d47fd5a29904Dae0Ef51b1c450C9750F15D7856",
           "reader": "0x8Fd8Cb948965d9305999D767A02bf79833EADbB3"},
}
GT = "https://api.geckoterminal.com/api/v2/networks/arbitrum/dexes/kyberswap_elastic_arbitrum/pools"


def gt_pages(pages):
    out = []
    for page in range(1, pages + 1):
        r = requests.get(GT, params={"page": page}, headers={"Accept": "application/json"}, timeout=60)
        out.append({"page": page, "status": r.status_code, "body": r.text})
        if r.status_code != 200 or not json.loads(r.text).get("data"):
            break
    return out


def addr_of(res):
    """Адрес из ответа {"ok": hex}; None — откат / нулевой адрес. Ошибок нет."""
    v = res.get("ok", "")
    a = "0x" + v[-40:] if len(v) >= 42 else None
    return a if a and int(a, 16) else None


def state_phase(rpc, ids, args, head):
    """Пулы на свежем блоке head: версия по factory(), пулы v1 по getPool фабрики v1 для пар кандидатов. Возвращает:
    (head, {"v1": […], "v2": […], "other": […]}). Ошибки: StaleState — recnode.fresh повторит на новом блоке."""
    tag = head["number"]
    by = {k: v["factory"].lower() for k, v in VERSIONS.items()}
    out, seen = {"v1": [], "v2": [], "other": []}, set()

    def take(pool, why):
        if pool in seen:
            return
        seen.add(pool)
        fac = addr_of(try_call(rpc, pool, "c45a0155", tag))
        ver = next((k for k, f in by.items() if fac and fac.lower() == f), None)
        if ver is None:
            out["other"].append({"pool": pool, "factory": fac, "why": why})
            return
        try:
            rec = record_pool(rpc, pool, VERSIONS[ver], args, tag)
        except Exception as e:  # noqa: BLE001 — ответ не по ABI пишется как есть; StaleState (BaseException) — наружу
            rec = {"pool": pool, "error": str(e)}
        rec["version"], rec["found_by"] = ver, why
        out[ver].append(rec)
        if ver != "v1" and "error" not in rec:
            # Пул v1 той же пары и комиссии (getPool фабрики v1) — тоже в запись.
            t0, t1 = addr_of(rec["token0"]), addr_of(rec["token1"])
            fee = int(rec["swapFeeUnits"]["ok"], 16)
            v1 = addr_of(try_call(rpc, VERSIONS["v1"]["factory"], "1698ee82" + addr_word(t0) + addr_word(t1) + word(fee),
                                  tag))
            if v1:
                take(v1.lower(), "getPool фабрики v1 для пары пула " + pool)

    for pool in list(dict.fromkeys(i.lower() for i in ids))[:args.max_pools]:
        take(pool, "GeckoTerminal / --pool")
    return head, out


def logs_phase(rpc, pools, args, b):
    """Логи пулов за [B − --log-blocks, B] частями (таймаут — пополам). Возвращает запись logs.json."""
    lo = max(0, b - args.log_blocks)
    addrs = [p["pool"] for p in pools]
    logs = []
    if addrs:
        logs = recnode.logs_split(
            lambda x, y: rpc.call("eth_getLogs", [{"address": addrs, "fromBlock": hex(x), "toBlock": hex(y)}]),
            lo, b, 1000, lambda t: print(t, file=sys.stderr, flush=True))
    ts = lambda n: int(rpc.call("eth_getBlockByNumber", [hex(n), False])["timestamp"], 16)  # noqa: E731
    return {"from": hex(lo), "to": hex(b), "from_timestamp": ts(lo), "to_timestamp": ts(b), "address": addrs,
            "logs": logs}


def main():
    ap = argparse.ArgumentParser(description="Запись фактов узла Arbitrum для KyberSwap Elastic v1/v2 (решения 451, 462)")
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--gt", action="store_true", help="записать пулы GeckoTerminal kyberswap_elastic_arbitrum")
    ap.add_argument("--gt-pages", type=int, default=10)
    ap.add_argument("--pool", action="append", default=[], help="адрес пула (0x…); можно несколько")
    ap.add_argument("--max-pools", type=int, default=30, help="не больше стольких кандидатов")
    ap.add_argument("--sizes", default="0.001,0.01,0.1,1,10,100,1000", help="суммы входа в единицах токена")
    ap.add_argument("--log-blocks", type=int, default=2000000, help="сколько блоков логов пулов до B")
    ap.add_argument("--tick-chunk", type=int, default=1000, help="длина части getTicksInRange при чтении частями")
    ap.add_argument("--out", default=None, help="папка вывода (по умолчанию tests/data/arbitrum_kyber_elastic_v1_<дата>)")
    args = ap.parse_args()

    rpc = Rpc(args.rpc)
    chain = rpc.call("eth_chainId", [])
    day = os.environ.get("ARB_RECORD_DATE") or datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = args.out or os.path.join(root, "tests", "data", "arbitrum_kyber_elastic_v1_" + day)
    os.makedirs(out, exist_ok=True)
    files = {"meta.json": None}

    # Решение 460: GeckoTerminal (с паузами темпа — gt.py) — ДО выбора блока B: узел хранит только недавнее состояние.
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
    head, pools = recnode.fresh(lambda: rpc.call("eth_getBlockByNumber", ["latest", False]),
                                lambda h: state_phase(rpc, ids, args, h))
    b, tag = int(head["number"], 16), head["number"]
    meta = {"chainId": chain, "block": b, "block_hex": tag, "timestamp": int(head["timestamp"], 16),
            "versions": VERSIONS, "sizes": args.sizes, "log_blocks": args.log_blocks, "tick_chunk": args.tick_chunk,
            "counts": {k: len(v) for k, v in pools.items()}}
    files["meta.json"] = meta
    files["pools.json"] = pools["v1"]
    files["pools_v2.json"] = pools["v2"]
    files["pools_other.json"] = pools["other"]
    files["logs.json"] = logs_phase(rpc, [p for p in pools["v1"] + pools["v2"] if "error" not in p], args, b)

    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# KyberSwap Elastic v1 / v2 (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/kyber_elastic_v1.py (решения 451, 462): только чтение, без ключей.\n")
        f.write("Блок B = %d (%s), время %d, chainId %s. Пулов: v1 %d, v2 %d, другие %d; логов %d. Узел: %s.\n\n" % (
            b, tag, meta["timestamp"], chain, len(pools["v1"]), len(pools["v2"]), len(pools["other"]),
            len(files["logs.json"]["logs"]), args.rpc.split("@")[-1]))
        f.write("Файлы и адреса — шапка скрипта.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    print("записано в", out, file=sys.stderr)


if __name__ == "__main__":
    main()
