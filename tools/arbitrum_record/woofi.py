#!/usr/bin/env python3
# Файл: tools/arbitrum_record/woofi.py
# Что делает: запись фактов своего узла Arbitrum для WOOFi WooPPV2.2 (решение 451, задача T448-08 / T433-095) — только
#   чтение (eth_call, eth_getStorageAt, eth_getCode, eth_getLogs, eth_getBlockByNumber), без ключей и подписей. Пишет
#   JSON-файлы в tests/data/arbitrum_woofi_<дата>/ и README.md (блок, хеши). По ним идут тесты
#   tests/arbitrum_woofi_live.rs (сейчас #[ignore] — записи нет).
# Кто вызывает: record_all.py (record_steps.py: только --rpc) или Тигран вручную.
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py, решение 459), recnode.py (решение 460),
#   curve_crypto_rpc.py (keccak256 — topic0 WooSwap). Узел — аргумент --rpc.
# Настройки: нет. WooPP по умолчанию — woonetwork/docs 9bcc162d references/readme/arbitrum-one.md L5 (WooPPV2.2 0x5520…9FA4).
#
# Решение 462 (разбор прогона 20.11.2026): запись полная без ручных параметров.
#   * Оракул — С ЦЕПИ: WooPP.wooracle() на блоке B (именно его сверяет бот: verify_woo.rs, wooracle() = dex.oracle).
#     Прогон 20.11: wooracle() = 0xf9b0c8ee…8081, а адрес из документации (WooracleV2_2_1 0xCf4E…34Ec, arbitrum-one.md L5;
#     resources/on-chain-price-feeds.md) — другой. Оракул документации пишется рядом (state по каждой базе) — для сравнения;
#     --oracle — только переопределение.
#   * Базовые монеты — по факту обменов: WooSwap(fromToken, toToken …) пула (IWooPPV2.sol L51-L61, f73a0f64) за
#     --log-blocks блоков до B; база — каждый токен обменов, кроме quoteToken() пула (USDC 0xaf88…5831 в прогоне 20.11);
#     --base — добавить вручную. Прогон 20.11: --base не задан → bases пуст, котировок не было.
#
# Что записывается:
#   meta.json   — chainId, блок B, WooPP, оракул с цепи (oracle) и из документации (oracle_docs), базы и откуда они;
#   swaps.json  — окно поиска WooSwap, число событий, токены и сколько раз встретились, первые --keep-logs логов (сырые);
#   proxy.json  — eth_getStorageAt(WooPP, слот EIP-1967 реализации), eth_getCode реализации и наличие селекторов
#                 query f58a435f, tryQuery ce824f19, swap 7dc20382, quoteToken() 217a4b70, wooracle() bc8530f9,
#                 paused() 5c975abb; у оракула с цепи и оракула документации — длина кода и наличие state(address) 31e658a5;
#   pool.json   — quoteToken(), wooracle(), paused() пула; по каждой базе: state(base) оракула с цепи (4 слова: price,
#                 spread, coeff, woFeasible), state(base) оракула документации (state_docs_oracle), decimals,
#                 query(base, quote, суммы --sizes) и query(quote, base, …) (откат — текст).
#
# Запуск: python3 tools/arbitrum_record/woofi.py --rpc http://127.0.0.1:8547 [--base 0x… …] [--log-blocks N]

import argparse
import datetime
import hashlib
import json
import os
import sys

import recnode  # решение 460: логи до блока B, состояние — на свежем B
import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests
from curve_crypto_rpc import keccak256

WOOPP = "0x5520385bFcf07Ec87C4c53A7d8d65595Dff69FA4"
ORACLE_DOCS = "0xCf4EA1688bc23DD93D933edA535F8B72FC8934Ec"
# EIP-1967: bytes32(uint256(keccak256('eip1967.proxy.implementation')) - 1).
IMPL_SLOT = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
POOL_SELECTORS = {"query": "f58a435f", "tryQuery": "ce824f19", "swap": "7dc20382", "quoteToken": "217a4b70",
                  "wooracle": "bc8530f9", "paused": "5c975abb"}
WOOSWAP = "0x" + keccak256(
    b"WooSwap(address,address,uint256,uint256,address,address,address,uint256,uint256)").hex()


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


def word(v):
    return format(v, "064x")


def aw(a):
    return word(int(a, 16))


def try_call(rpc, to, data, tag):
    """eth_call → {"ok": hex} или {"error": текст}. StaleState (состояние блока удалено) — наружу (recnode.fresh)."""
    try:
        return {"ok": rpc.call("eth_call", [{"to": to, "input": "0x" + data}, tag])}
    except Exception as e:
        return {"error": str(e)}


def swaps(rpc, woopp, blocks, keep):
    """WooSwap пула за [голова − blocks, голова] (таймаут узла — окно пополам). Возвращает: запись swaps.json и токены
    обменов по убыванию числа событий. Ошибки: ошибка узла (кроме таймаута) — RuntimeError."""
    hi = int(rpc.call("eth_blockNumber", []), 16)
    lo = max(0, hi - blocks)
    flt = {"address": woopp, "topics": [WOOSWAP]}
    logs = recnode.logs_split(lambda a, b: rpc.call("eth_getLogs", [dict(flt, fromBlock=hex(a), toBlock=hex(b))]),
                              lo, hi, 1000, lambda t: print(t, file=sys.stderr, flush=True))
    seen = {}
    for g in logs:
        for t in g["topics"][1:3]:  # topic1 = fromToken, topic2 = toToken (indexed)
            a = "0x" + t[-40:]
            seen[a] = seen.get(a, 0) + 1
    order = sorted(seen, key=lambda a: -seen[a])
    return {"from": lo, "to": hi, "topic0": WOOSWAP, "events": len(logs), "tokens": {a: seen[a] for a in order},
            "logs_first": logs[:keep]}, order


def state_phase(rpc, args, bases_in, head):
    """Состояние на свежем блоке head. Возвращает: (head, файлы meta/proxy/pool). Ошибки: StaleState — recnode.fresh."""
    b, tag = int(head["number"], 16), head["number"]
    pool = {k: try_call(rpc, args.woopp, sel, tag)
            for k, sel in [("quoteToken", "217a4b70"), ("wooracle", "bc8530f9"), ("paused", "5c975abb")]}
    quote = "0x" + pool["quoteToken"]["ok"][-40:] if "ok" in pool["quoteToken"] else None
    chain_oracle = "0x" + pool["wooracle"]["ok"][-40:] if "ok" in pool["wooracle"] else None
    oracle = args.oracle or chain_oracle or ORACLE_DOCS
    slot = rpc.call("eth_getStorageAt", [args.woopp, IMPL_SLOT, tag])
    impl = "0x" + slot[-40:]
    code = rpc.call("eth_getCode", [impl, tag])
    ocode = {o: rpc.call("eth_getCode", [o, tag]) for o in dict.fromkeys([oracle, ORACLE_DOCS])}
    proxy = {"slot": IMPL_SLOT, "slot_value": slot, "implementation": impl, "code": code,
             "selectors": {k: ("63" + v) in code[2:] for k, v in POOL_SELECTORS.items()},
             "oracle_code_has_state": "6331e658a5" in ocode[oracle][2:], "oracle_code_len": len(ocode[oracle]) // 2 - 1,
             "oracle_docs": {"address": ORACLE_DOCS, "code_len": len(ocode[ORACLE_DOCS]) // 2 - 1,
                             "code_has_state": "6331e658a5" in ocode[ORACLE_DOCS][2:]}}
    bases = [x for x in dict.fromkeys(a.lower() for a in bases_in) if not quote or x != quote.lower()]
    recs = []
    for base in bases:
        rec = {"base": base, "state": try_call(rpc, oracle, "31e658a5" + aw(base), tag),
               "state_docs_oracle": try_call(rpc, ORACLE_DOCS, "31e658a5" + aw(base), tag),
               "decimals": try_call(rpc, base, "313ce567", tag), "queries": []}
        if quote and "ok" in rec["decimals"]:
            qdec = try_call(rpc, quote, "313ce567", tag)
            rec["quote_decimals"] = qdec
            for frm, to, dec in ((base, quote, rec["decimals"]), (quote, base, qdec)):
                if "ok" not in dec:
                    continue
                for s in args.sizes.split(","):
                    amount = int(float(s) * 10 ** int(dec["ok"], 16))
                    q = {"from": frm, "to": to, "amount": str(amount)}
                    q.update(try_call(rpc, args.woopp, "f58a435f" + aw(frm) + aw(to) + word(amount), tag))
                    rec["queries"].append(q)
        recs.append(rec)
    pool["bases"] = recs
    meta = {"block": b, "block_hex": tag, "timestamp": int(head["timestamp"], 16), "woopp": args.woopp,
            "oracle": oracle, "oracle_source": "аргумент --oracle" if args.oracle else "WooPP.wooracle() на блоке B",
            "oracle_docs": ORACLE_DOCS, "quote_token": quote, "bases": bases, "sizes": args.sizes}
    return head, meta, proxy, pool


def main():
    ap = argparse.ArgumentParser(description="Запись фактов узла Arbitrum для WOOFi (решения 451, 462)")
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--woopp", default=WOOPP)
    ap.add_argument("--oracle", default=None, help="переопределить оракул (по умолчанию — WooPP.wooracle() с цепи)")
    ap.add_argument("--base", action="append", default=[], help="добавить базовую монету (0x…); можно несколько")
    ap.add_argument("--log-blocks", type=int, default=2000000, help="окно поиска WooSwap до головы, блоков")
    ap.add_argument("--max-bases", type=int, default=20, help="не больше стольких баз из обменов (по частоте)")
    ap.add_argument("--keep-logs", type=int, default=50, help="сколько сырых логов WooSwap сохранить")
    ap.add_argument("--sizes", default="0.001,0.1,10,1000", help="суммы входа в единицах монеты")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    rpc = Rpc(args.rpc)
    chain = rpc.call("eth_chainId", [])
    day = os.environ.get("ARB_RECORD_DATE") or datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = args.out or os.path.join(root, "tests", "data", "arbitrum_woofi_" + day)
    os.makedirs(out, exist_ok=True)
    # Решение 460: логи — до выбора блока B (узел хранит только недавнее состояние).
    sw, tokens = swaps(rpc, args.woopp, args.log_blocks, args.keep_logs)
    bases_in = list(args.base) + tokens[:args.max_bases + 1]  # +1: quoteToken отсеется в state_phase
    head, meta, proxy, pool = recnode.fresh(lambda: rpc.call("eth_getBlockByNumber", ["latest", False]),
                                            lambda h: state_phase(rpc, args, bases_in, h))
    meta = dict({"chainId": chain}, **meta, bases_from={"--base": args.base, "WooSwap": tokens[:args.max_bases + 1]})
    files = {"meta.json": meta, "swaps.json": sw, "proxy.json": proxy, "pool.json": pool}

    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# WOOFi WooPPV2.2 (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/woofi.py (решения 451, 462): только чтение, без ключей и подписей.\n")
        f.write("Блок B = %d (%s), chainId %s. WooPP %s, оракул %s (%s; документация — %s), реализация (слот "
                "EIP-1967) %s. Баз: %d (WooSwap за блоки %d…%d: %d событий). Узел: %s.\n\n" % (
                    meta["block"], meta["block_hex"], chain, args.woopp, meta["oracle"], meta["oracle_source"],
                    ORACLE_DOCS, proxy["implementation"], len(meta["bases"]), sw["from"], sw["to"], sw["events"],
                    args.rpc.split("@")[-1]))
        f.write("Файлы — шапка скрипта.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    print("записано в", out, file=sys.stderr)


if __name__ == "__main__":
    main()
