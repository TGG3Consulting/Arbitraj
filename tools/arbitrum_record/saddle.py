#!/usr/bin/env python3
# Файл: tools/arbitrum_record/saddle.py
# Что делает: запись фактов своего узла Arbitrum для пулов Saddle (решение 451, задача T448-07 / T433-087) — только
#   чтение (eth_call, eth_getLogs, eth_getBlockByNumber), без ключей и подписей. Пишет JSON-файлы в
#   tests/data/arbitrum_saddle_<дата>/ и README.md (что записано, блок, хеши). По ним идут тесты
#   tests/arbitrum_saddle_live.rs (сейчас #[ignore] — записи нет).
# Кто вызывает: Тигран на своём сервере (узел Arbitrum), вручную.
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py, решение 459). Узел — аргумент --rpc (HTTP JSON-RPC).
# Настройки: нет (пулы — аргументы --pool; по умолчанию — плоские пулы saddle-finance/saddle-contract 2cfdd055
#   deployments/arbitrum_mainnet: SaddleArbUSDPool, SaddleArbUSDPoolV2, SaddleFRAXBPPool, CommunityfUSDCPool).
#
# Что записывается (на одном блоке B = «latest», номер — в meta.json):
#   meta.json  — eth_chainId, номер и время блока B, пулы, аргументы;
#   pools.json — по каждому пулу: getToken(k) и getTokenBalance(k) до первого отката getToken, getAPrecise(),
#                swapStorage() (сырой hex — длина ответа проверяется тестом: 7 слов у Saddle), paused(), decimals()
#                монет, calculateSwap(i, j, dx) на суммы --sizes для всех пар (i, j) (сырой hex; откат — текст);
#   logs.json  — eth_getLogs пулов за блоки [B − --log-blocks, B] (TokenSwap, FlashLoan, ликвидность, RampA …).
# Селекторы (keccak256, посчитаны при написании): getToken(uint8) 82b86600, getTokenBalance(uint8) 91ceb3eb,
#   getAPrecise() 0ba81959, swapStorage() 5fd65f0f, paused() 5c975abb, calculateSwap(uint8,uint8,uint256) a95b089f,
#   decimals() 313ce567. topic0 FlashLoan(address,uint8,uint256,uint256,uint256) 7c186b28…22f0.
#
# Ревью t251: чтение пулов — одной фазой recnode.fresh (узел хранит состояние ~128 блоков, решение 460).
#
# Запуск: python3 tools/arbitrum_record/saddle.py --rpc http://127.0.0.1:8547 [--pool 0x…]

import argparse
import datetime
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import recnode  # noqa: E402 — ревью t251: фаза состояния на свежем блоке (решение 460)
import stdhttp as requests  # noqa: E402 — решение 459: стандартная библиотека вместо пакета requests

POOLS = [
    "0xBea9F78090bDB9e662d8CB301A00ad09A5b756e9",  # SaddleArbUSDPool (SwapFlashLoan)
    "0xfeEa4D1BacB0519E8f952460A70719944fe56Ee0",  # SaddleArbUSDPoolV2 (SwapFlashLoan)
    "0x401AFbc31ad2A3Bc0eD8960d63eFcDEA749b4849",  # SaddleFRAXBPPool (SwapFlashLoan)
    "0x773f9144969FA19CdB4cAE1ef19D7A381f5F6922",  # CommunityfUSDCPool (PermissionlessSwap)
]
MAX_TOKENS = 32  # Swap.sol L123: «_pooledTokens.length > 32»


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


def try_call(rpc, to, data, tag):
    """eth_call → {"ok": hex} или {"error": текст}."""
    try:
        return {"ok": rpc.call("eth_call", [{"to": to, "input": "0x" + data}, tag])}
    except Exception as e:
        return {"error": str(e)}


def record_pool(rpc, pool, sizes, tag):
    rec = {"pool": pool, "getToken": [], "getTokenBalance": []}
    for k in range(MAX_TOKENS):
        t = try_call(rpc, pool, "82b86600" + word(k), tag)
        if "ok" not in t:
            rec["getToken_end"] = {"index": k, "error": t["error"]}
            break
        rec["getToken"].append(t)
        rec["getTokenBalance"].append(try_call(rpc, pool, "91ceb3eb" + word(k), tag))
    for name, sel in [("getAPrecise", "0ba81959"), ("swapStorage", "5fd65f0f"), ("paused", "5c975abb")]:
        rec[name] = try_call(rpc, pool, sel, tag)
    coins = ["0x" + t["ok"][-40:] for t in rec["getToken"]]
    rec["decimals"] = [try_call(rpc, c, "313ce567", tag) for c in coins]
    quotes = []
    for i in range(len(coins)):
        dec = int(rec["decimals"][i]["ok"], 16)
        for j in range(len(coins)):
            if i == j:
                continue
            for s in sizes.split(","):
                dx = int(float(s) * 10 ** dec)
                q = {"i": i, "j": j, "dx": str(dx)}
                q.update(try_call(rpc, pool, "a95b089f" + word(i) + word(j) + word(dx), tag))
                quotes.append(q)
    rec["quotes"] = quotes
    return rec


def main():
    ap = argparse.ArgumentParser(description="Запись фактов узла Arbitrum для пулов Saddle (решение 451)")
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--pool", action="append", default=[], help="адрес пула (0x…); не задан — пулы по умолчанию")
    ap.add_argument("--sizes", default="0.01,1,100,10000,1000000", help="суммы входа в единицах монеты")
    ap.add_argument("--log-blocks", type=int, default=20000, help="сколько блоков логов пулов до B")
    ap.add_argument("--out", default=None, help="папка вывода (по умолчанию tests/data/arbitrum_saddle_<дата>)")
    args = ap.parse_args()

    rpc = Rpc(args.rpc)
    chain = rpc.call("eth_chainId", [])
    pools = args.pool or POOLS

    def body(head):
        """Все пулы на одном блоке head; состояние удалено узлом (StaleState) — recnode.fresh начнёт заново."""
        recs = []
        for p in pools:
            try:
                recs.append(record_pool(rpc, p, args.sizes, head["number"]))
            except Exception as e:  # StaleState — BaseException, сюда не попадает
                recs.append({"pool": p, "error": str(e)})
        return head, recs

    head, recs = recnode.fresh(lambda: rpc.call("eth_getBlockByNumber", ["latest", False]), body)
    b, tag = int(head["number"], 16), head["number"]
    day = datetime.date.today().isoformat()
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    out = args.out or os.path.join(root, "tests", "data", "arbitrum_saddle_" + day)
    os.makedirs(out, exist_ok=True)
    files = {"meta.json": {"chainId": chain, "block": b, "block_hex": tag, "timestamp": int(head["timestamp"], 16),
                           "pools": pools, "sizes": args.sizes, "log_blocks": args.log_blocks}}
    files["pools.json"] = recs
    frm = hex(max(0, b - args.log_blocks))
    files["logs.json"] = {"from": frm, "to": tag,
                          "logs": rpc.call("eth_getLogs", [{"address": pools, "fromBlock": frm, "toBlock": tag}])}

    hashes = []
    for name, body in files.items():
        text = json.dumps(body, ensure_ascii=False, indent=1)
        with open(os.path.join(out, name), "w", encoding="utf-8") as f:
            f.write(text)
        hashes.append((hashlib.sha256(text.encode()).hexdigest(), name))
    with open(os.path.join(out, "README.md"), "w", encoding="utf-8") as f:
        f.write("# Saddle (Arbitrum One) — запись своего узла, %s\n\n" % day)
        f.write("Записано tools/arbitrum_record/saddle.py (решение 451): только чтение, без ключей и подписей.\n")
        f.write("Блок B = %d (%s), время %d, chainId %s. Пулы: %s. Узел: %s.\n\n" % (
            b, tag, files["meta.json"]["timestamp"], chain, ", ".join(pools), args.rpc.split("@")[-1]))
        f.write("Файлы — шапка скрипта.\n\n## SHA-256\n\n")
        for h, n in hashes:
            f.write("%s  %s\n" % (h, n))
    print("записано в", out, file=sys.stderr)


if __name__ == "__main__":
    main()
