#!/usr/bin/env python3
# Файл: tools/arbitrum_record/woofi.py
# Что делает: запись фактов своего узла Arbitrum для WOOFi WooPPV2.2 (решение 451, задача T448-08 / T433-095) — только
#   чтение (eth_call, eth_getStorageAt, eth_getCode, eth_getBlockByNumber), без ключей и подписей. Пишет JSON-файлы в
#   tests/data/arbitrum_woofi_<дата>/ и README.md (блок, хеши). По ним идут тесты tests/arbitrum_woofi_live.rs
#   (сейчас #[ignore] — записи нет).
# Кто вызывает: Тигран на своём сервере (узел Arbitrum), вручную.
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py, решение 459). Узел — аргумент --rpc.
# Настройки: нет (адреса — аргументы; по умолчанию — woonetwork/docs 9bcc162d references/readme/arbitrum-one.md L5:
#   WooPPV2.2 0x5520…9FA4, WooracleV2_2_1 0xCf4E…34Ec; монеты — --base, адреса вводит запускающий).
#
# Что записывается (на блоке B = «latest»):
#   meta.json  — chainId, блок, адреса;
#   proxy.json — eth_getStorageAt(WooPP, слот EIP-1967 реализации), eth_getCode реализации (hex) и наличие в нём
#                селекторов query f58a435f, tryQuery ce824f19, swap(address,address,uint256,uint256,address,address)
#                7dc20382, quoteToken() 217a4b70, wooracle() bc8530f9, paused() 5c975abb; eth_getCode оракула и
#                наличие state(address) 31e658a5;
#   pool.json  — quoteToken(), wooracle(), paused() у WooPP; по каждой --base: state(base) оракула (сырой hex — 4 слова:
#                price, spread, coeff, woFeasible), query(base, quote, суммы --sizes) и query(quote, base, …) (откат — текст).
#
# Запуск: python3 tools/arbitrum_record/woofi.py --rpc http://127.0.0.1:8547 --base 0x82aF…bAb1 [--base 0x…]

import argparse
import datetime
import hashlib
import json
import os
import sys

import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests

WOOPP = "0x5520385bFcf07Ec87C4c53A7d8d65595Dff69FA4"
ORACLE = "0xCf4EA1688bc23DD93D933edA535F8B72FC8934Ec"
# EIP-1967: bytes32(uint256(keccak256('eip1967.proxy.implementation')) - 1).
IMPL_SLOT = "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc"
POOL_SELECTORS = {"query": "f58a435f", "tryQuery": "ce824f19", "swap": "7dc20382", "quoteToken": "217a4b70",
                  "wooracle": "bc8530f9", "paused": "5c975abb"}


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
    try:
        return {"ok": rpc.call("eth_call", [{"to": to, "input": "0x" + data}, tag])}
    except Exception as e:
        return {"error": str(e)}


def main():
    ap = argparse.ArgumentParser(description="Запись фактов узла Arbitrum для WOOFi (решение 451)")
    ap.add_argument("--rpc", required=True)
    ap.add_argument("--woopp", default=WOOPP)
    ap.add_argument("--oracle", default=ORACLE)
    ap.add_argument("--base", action="append", default=[], help="адрес базовой монеты (0x…); можно несколько")
    ap.add_argument("--sizes", default="0.001,0.1,10,1000", help="суммы входа в единицах монеты")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    rpc = Rpc(args.rpc)
    chain = rpc.call("eth_chainId", [])
    head = rpc.call("eth_getBlockByNumber", ["latest", False])
    b, tag = int(head["number"], 16), head["number"]
    day = datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = args.out or os.path.join(root, "tests", "data", "arbitrum_woofi_" + day)
    os.makedirs(out, exist_ok=True)
    files = {"meta.json": {"chainId": chain, "block": b, "block_hex": tag, "timestamp": int(head["timestamp"], 16),
                           "woopp": args.woopp, "oracle": args.oracle, "bases": args.base, "sizes": args.sizes}}

    slot = rpc.call("eth_getStorageAt", [args.woopp, IMPL_SLOT, tag])
    impl = "0x" + slot[-40:]
    code = rpc.call("eth_getCode", [impl, tag])
    ocode = rpc.call("eth_getCode", [args.oracle, tag])
    files["proxy.json"] = {"slot": IMPL_SLOT, "slot_value": slot, "implementation": impl, "code": code,
                           "selectors": {k: ("63" + v) in code[2:] for k, v in POOL_SELECTORS.items()},
                           "oracle_code_has_state": "6331e658a5" in ocode[2:], "oracle_code_len": len(ocode) // 2 - 1}

    pool = {k: try_call(rpc, args.woopp, sel, tag)
            for k, sel in [("quoteToken", "217a4b70"), ("wooracle", "bc8530f9"), ("paused", "5c975abb")]}
    quote = "0x" + pool["quoteToken"]["ok"][-40:] if "ok" in pool["quoteToken"] else None
    bases = []
    for base in args.base:
        rec = {"base": base, "state": try_call(rpc, args.oracle, "31e658a5" + aw(base), tag),
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
        bases.append(rec)
    pool["bases"] = bases
    files["pool.json"] = pool

    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# WOOFi WooPPV2.2 (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/woofi.py (решение 451): только чтение, без ключей и подписей.\n")
        f.write("Блок B = %d (%s), chainId %s. WooPP %s, оракул %s, реализация (слот EIP-1967) %s. Узел: %s.\n\n" % (
            b, tag, chain, args.woopp, args.oracle, impl, args.rpc.split("@")[-1]))
        f.write("Файлы — шапка скрипта.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    print("записано в", out, file=sys.stderr)


if __name__ == "__main__":
    main()
