#!/usr/bin/env python3
# Файл: tools/arbitrum_record/kyber_elastic_legacy.py
# Что делает: решение 475 (T462-01) — запись фактов своего узла Arbitrum для KyberSwap Elastic Legacy (v1, фабрика
#   0x5F1d…D50a): пулы — НЕ из GeckoTerminal (там только v2), а реестром фабрики getPool(a, b, swapFeeUnits) по парам
#   токенов-кандидатов и уровням комиссии (перечисления пулов у фабрики нет: в Factory.sol только mapping getPool —
#   Sourcify 42161 0x5F1d…D50a, ks-elastic-sc 4ab08c0a contracts/Factory.sol L44); по найденным пулам — то, что читает
#   бот (как kyber_elastic_v1.py: состояние, getPool, тики частями getTicksInRange(pool, start, --tick-chunk) и одним
#   вызовом (length = 0), ticks(t), QuoterV2 v1 в обе стороны); события PoolCreated / SwapFeeEnabled фабрики — узким
#   окном --log-blocks до блока B (фильтр по адресу фабрики). Только чтение, без ключей.
# Кто вызывает: record_all.py (record_steps.py: --rpc) или Тигран вручную.
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py, темп GeckoTerminal — gt.py), recnode.py,
#   kyber_elastic_ticks.py. GeckoTerminal — только список токенов-кандидатов (пары v2 kyberswap_elastic_arbitrum и
#   верхние пулы сети arbitrum); пул берётся только по ответу фабрики v1.
# Адреса v1 (официальные): Factory 0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a,
#   QuoterV2 0x0D125c15D54cA1F8a813C74A81aEe34ebB508C1f, TicksFeesReader 0x165c68077ac06c83800d19200e6E2B08D02dE75D —
#   ks-elastic-sc 4ab08c0a README.md L6-L17;
#   docs.kyberswap.com reference/legacy/elastic-legacy/elastic-contract-addresses (Arbitrum).
# Уровни комиссии по умолчанию 8, 10, 40, 300, 1000 — конструктор фабрики v1 (Sourcify 42161:
#   feeAmountTickDistance[8] = 1, [10] = 1, [40] = 8, [300] = 60, [1000] = 200); feeAmountTickDistance(fee) пишется
#   с цепи. Уровни, включённые позже enableSwapFee, видны только по событиям SwapFeeEnabled — в окне
#   --log-blocks (вся история — нельзя, решение 460).
# Селекторы (keccak256): getPool(address,address,uint24) 1698ee82, feeAmountTickDistance(uint24) 6cc85293; события:
#   PoolCreated(address,address,uint24,int24,address) 783cca1c…7118, SwapFeeEnabled(uint24,int24) 6f406634…f422.
#
# Что записывается (tests/data/arbitrum_kyber_elastic_legacy_<дата>/):
#   meta.json      — chainId, блок B и время, адреса, аргументы, число токенов / пар / вызовов;
#   gt_tokens.json — ответы GeckoTerminal как есть и итоговый список токенов;
#   fees.json      — feeAmountTickDistance(fee) фабрики v1 по уровням;
#   discover.json  — ненулевые ответы getPool (token_a, token_b, fee, pool) и ошибки пачек;
#   pools.json     — найденные пулы на блоке B (формат kyber_elastic_ticks.record_pool + found_by);
#   factory_logs.json — eth_getLogs фабрики за [B − --log-blocks, B] (+ время границ).
# Тест: tests/arbitrum_kyber_elastic_legacy_live.rs (#[ignore] до записи).
# Запуск: python3 tools/arbitrum_record/kyber_elastic_legacy.py --rpc http://127.0.0.1:8547

import argparse
import datetime
import hashlib
import itertools
import json
import os
import sys

import recnode
import stdhttp as requests
from kyber_elastic_ticks import Rpc, addr_word, record_pool, try_call, word

V1 = {"factory": "0x5F1dddbf348aC2fbe22a163e30F99F9ECE3DD50a", "quoter": "0x0D125c15D54cA1F8a813C74A81aEe34ebB508C1f",
      "reader": "0x165c68077ac06c83800d19200e6E2B08D02dE75D"}
FEES = "8,10,40,300,1000"
TOPICS = ["0x783cca1c0412dd0d695e784568c96da2e9c22ff989357a2e8b1d9b2b4e6b7118",
          "0x6f406634e7dd70954c5839918b5b301612c89d7c15e6b52548fa5b4c0f2cf422"]
GT = "https://api.geckoterminal.com/api/v2/networks/arbitrum"


def say(text):
    print(text, file=sys.stderr, flush=True)


def gt_tokens(pages, limit):
    """Токены-кандидаты из GeckoTerminal: пары пулов v2 kyberswap_elastic_arbitrum и верхние пулы сети. Возвращает:
    (ответы как есть, токены в нижнем регистре, не больше limit). Ошибки HTTP — в ответах, не исключение."""
    raw, tokens = [], []
    for path in ("/dexes/kyberswap_elastic_arbitrum/pools", "/pools"):
        for page in range(1, pages + 1):
            r = requests.get(GT + path, params={"page": page}, headers={"Accept": "application/json"}, timeout=60)
            raw.append({"path": path, "page": page, "status": r.status_code, "body": r.text})
            if r.status_code != 200:
                break
            data = json.loads(r.text).get("data") or []
            for item in data:
                rel = item.get("relationships", {})
                for side in ("base_token", "quote_token"):
                    tid = (rel.get(side, {}).get("data") or {}).get("id", "")
                    a = tid.split("_", 1)[-1].lower()
                    if a.startswith("0x") and len(a) == 42 and a not in tokens:
                        tokens.append(a)
            if not data:
                break
    return raw, tokens[:limit]


def batch_calls(rpc_url, calls, size, deadline):
    """eth_call пачками JSON-RPC (latest). Принимает: [(to, data)], размер пачки, потолок. Возвращает: список ответов
    {"ok"} / {"error"} в том же порядке. Ошибки: TimeUp; ошибка HTTP пачки — {"error"} у всех её вызовов."""
    out = []
    for i in range(0, len(calls), size):
        deadline.check("getPool пачкой %d из %d" % (i // size + 1, (len(calls) + size - 1) // size))
        part = calls[i:i + size]
        body = [{"jsonrpc": "2.0", "id": k, "method": "eth_call", "params": [{"to": to, "input": "0x" + d}, "latest"]}
                for k, (to, d) in enumerate(part)]
        try:
            r = requests.post(rpc_url, json=body, timeout=deadline.http(120))
            r.raise_for_status()
            got = {x.get("id"): x for x in r.json()}
            out += [{"ok": got[k]["result"]} if "result" in got.get(k, {}) else
                    {"error": json.dumps(got.get(k, {}).get("error", "нет ответа"))} for k in range(len(part))]
        except Exception as e:  # noqa: BLE001 — ошибка пачки пишется у её вызовов
            out += [{"error": str(e)}] * len(part)
    return out


def discover(args, rpc, tokens, fees, deadline):
    """Реестр фабрики v1 по всем парам кандидатов и уровням. Возвращает: (найденные [{token_a, token_b, fee, pool}],
    число вызовов, ошибки). Порядок токенов любой: реестр в обе стороны (Factory.sol createPool)."""
    asks = [(a, b, f) for a, b in itertools.combinations(tokens, 2) for f in fees]
    calls = [(V1["factory"], "1698ee82" + addr_word(a) + addr_word(b) + word(f)) for a, b, f in asks]
    res = batch_calls(args.rpc, calls, args.batch, deadline)
    found, errors = [], []
    for (a, b, f), r in zip(asks, res):
        if "ok" not in r:
            errors.append({"token_a": a, "token_b": b, "fee": f, "error": r["error"][:300]})
        elif len(r["ok"]) >= 42 and int(r["ok"][-40:], 16):
            found.append({"token_a": a, "token_b": b, "fee": f, "pool": "0x" + r["ok"][-40:]})
    return found, len(calls), errors


def main():
    ap = argparse.ArgumentParser(description="Запись узла Arbitrum: KyberSwap Elastic Legacy (v1) — решение 475")
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--gt-pages", type=int, default=3, help="страниц GeckoTerminal на список (токены-кандидаты)")
    ap.add_argument("--token", action="append", default=[], help="токен-кандидат 0x… (можно несколько)")
    ap.add_argument("--max-tokens", type=int, default=60, help="не больше стольких токенов из GeckoTerminal")
    ap.add_argument("--fees", default=FEES, help="уровни swapFeeUnits через запятую (конструктор фабрики v1)")
    ap.add_argument("--batch", type=int, default=200, help="вызовов getPool в одной пачке JSON-RPC")
    ap.add_argument("--max-pools", type=int, default=30, help="не больше стольких пулов в фазе состояния")
    ap.add_argument("--sizes", default="0.001,0.01,0.1,1,10,100,1000", help="суммы входа квотера в единицах токена")
    ap.add_argument("--tick-chunk", type=int, default=1000, help="длина части getTicksInRange")
    ap.add_argument("--log-blocks", type=int, default=2000000, help="окно событий фабрики до B, блоков")
    ap.add_argument("--time-limit", type=int, default=600, help="потолок времени скрипта, с (0 — без потолка)")
    ap.add_argument("--out", default=None, help="папка (по умолчанию tests/data/arbitrum_kyber_elastic_legacy_<дата>)")
    args = ap.parse_args()
    deadline = recnode.Deadline(args.time_limit)
    rpc = Rpc(args.rpc)
    chain = rpc.call("eth_chainId", [])
    day = os.environ.get("ARB_RECORD_DATE") or datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = args.out or os.path.join(root, "tests", "data", "arbitrum_kyber_elastic_legacy_" + day)
    os.makedirs(out, exist_ok=True)

    # GeckoTerminal и реестр — до блока B (реестр getPool постоянный: пул из него не удаляется — Factory.sol).
    raw, tokens = gt_tokens(args.gt_pages, args.max_tokens)
    tokens = list(dict.fromkeys([t.lower() for t in args.token] + tokens))
    fees = [int(x) for x in args.fees.split(",") if x.strip()]
    found, n_calls, errors = discover(args, rpc, tokens, fees, deadline)
    say("токенов %d, вызовов getPool %d, пулов %d, ошибок %d" % (len(tokens), n_calls, len(found), len(errors)))

    def state(head):
        tag = head["number"]
        fee_rec = {str(f): try_call(rpc, V1["factory"], "6cc85293" + word(f), tag) for f in fees}
        pools = []
        for item in found[:args.max_pools]:
            deadline.check("состояние пула " + item["pool"])
            try:
                rec = record_pool(rpc, item["pool"], V1, args, tag)
            except Exception as e:  # noqa: BLE001 — ответ не по ABI пишется как есть; StaleState — наружу
                rec = {"pool": item["pool"], "error": str(e)}
            rec["found_by"] = item
            pools.append(rec)
        return head, fee_rec, pools

    head, fee_rec, pools = recnode.fresh(lambda: rpc.call("eth_getBlockByNumber", ["latest", False]), state, say=say)
    b, tag = int(head["number"], 16), head["number"]
    lo = max(0, b - args.log_blocks)
    logs = recnode.logs_split(lambda x, y: rpc.call("eth_getLogs", [{"address": V1["factory"], "topics": [TOPICS],
                                                                      "fromBlock": hex(x), "toBlock": hex(y)}]),
                              lo, b, 1000, say, deadline)
    ts = lambda n: int(rpc.call("eth_getBlockByNumber", [hex(n), False])["timestamp"], 16)  # noqa: E731
    files = {
        "meta.json": {"chainId": chain, "block": b, "block_hex": tag, "timestamp": int(head["timestamp"], 16),
                      "v1": V1, "fees": fees, "tokens": len(tokens), "getPool_calls": n_calls, "found": len(found),
                      "sizes": args.sizes, "tick_chunk": args.tick_chunk, "log_blocks": args.log_blocks},
        "gt_tokens.json": {"responses": raw, "tokens": tokens},
        "fees.json": fee_rec,
        "discover.json": {"found": found, "errors": errors},
        "pools.json": pools,
        "factory_logs.json": {"from": hex(lo), "to": tag, "from_timestamp": ts(lo), "to_timestamp": ts(b),
                              "topics": TOPICS, "logs": logs},
    }
    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# KyberSwap Elastic Legacy (v1, Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/kyber_elastic_legacy.py (решение 475): только чтение, без ключей.\n")
        f.write("Блок B = %d (%s), chainId %s. Токенов %d, вызовов getPool %d, пулов %d (в записи %d), "
                "событий %d.\n\n" % (b, tag, chain, len(tokens), n_calls, len(found), len(pools), len(logs)))
        f.write("Файлы и адреса — шапка скрипта.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    say("записано в " + out)


if __name__ == "__main__":
    main()
