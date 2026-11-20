#!/usr/bin/env python3
# Файл: tools/arbitrum_record/fluid_t1.py
# Что делает: решение 440 — запись ответов СВОЕГО узла Arbitrum One для тестов семейства Fluid DEX T1 (вид fluid_t1):
#   на одном блоке B — время и хеш блока; список пулов резолвера (getAllPoolAddresses); по каждому пулу — те же вызовы,
#   что читает бот (pools/load_fluid.rs: getPoolReservesAdjusted резолвера, constantsView, constantsView2,
#   readFromStorage(0), readFromStorage(1), VERSION), isDex фабрики (pools/verify_fluid.rs) и лесенка обменов в обе
#   стороны: swapIn пула с получателем 0x…dEaD (пул откатывается FluidDexSwapResult(выход) — точный выход кода пула,
#   main.sol L338) и estimateSwapIn резолвера (сверка бота, quote/quoter_fluid.rs). Ключей и подписей нет — только
#   eth_call / eth_getBlockByNumber. Пишет JSON в tests/data/arbitrum_fluid_t1_<дата>/ и README.md с блоком и SHA-256.
# Кто вызывает: Тигран, со своего сервера (узел Arbitrum — аргумент).
# От чего зависит: Python 3, requests.
# Настройки: нет (адреса — аргументы; официальные значения — docs/FIRST_RUN.md, раздел решения 440).
#
# Запуск (адреса — Instadapp/fluid-contracts-public@404d73f deployments/arbitrum/*.json):
#   python3 tools/arbitrum_record/fluid_t1.py --node http://127.0.0.1:8547 \
#     --factory 0x91716C4EDA1Fb55e84Bf8b4c7085f84285c19085 --resolver 0x05Bd8269A20C472b148246De20E6852091BF16Ff \
#     --wrapped <dex.native_wrapped площадки arb_dex>
# Необязательно: --block N (по умолчанию — голова минус --lag блоков), --gt (записать ещё страницу пулов
# GeckoTerminal networks/arbitrum/dexes/fluid-arbitrum/pools — какие адреса токенов отдаёт справочник у пулов с ETH).

import argparse
import datetime
import hashlib
import json
import os
import sys

import requests

DEAD = "0x000000000000000000000000000000000000dead"
# Селекторы — keccak256 подписей исходника (pools/abi_fluid.rs, там же сверка с кодом в сети).
SEL = {
    "getAllPoolAddresses": "0xc56f1b44",
    "getPoolReservesAdjusted": "0xbd964d38",
    "estimateSwapIn": "0xbb39e3a1",
    "constantsView": "0xb7791bf2",
    "constantsView2": "0x1595cbd3",
    "readFromStorage": "0xb5c736e4",
    "VERSION": "0xffa1ad74",
    "isDex": "0x3c11e12a",
    "swapIn": "0x2668dfaa",
}
# Доли мнимого резерва входа для лесенки сумм (1e-6 … 0.4): ниже 0.5 — предел пула (main.sol L194-L202).
LADDER = [1e-6, 1e-5, 1e-4, 1e-3, 1e-2, 5e-2, 0.1, 0.2, 0.4]


def word(v):
    """Слово ABI: адрес, bool или целое → 64 hex-символа."""
    if isinstance(v, bool):
        v = int(v)
    if isinstance(v, str):
        v = int(v, 16)
    return format(v, "064x")


def rpc(node, method, params):
    """Один запрос JSON-RPC. Возвращает весь ответ (result или error как есть)."""
    body = {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
    r = requests.post(node, json=body, timeout=60)
    r.raise_for_status()
    return r.json()


def call(node, to, data, tag):
    """eth_call на блоке tag. Возвращает запись {to, data, result | error}."""
    resp = rpc(node, "eth_call", [{"to": to, "data": data}, tag])
    out = {"to": to, "data": data}
    if "result" in resp:
        out["result"] = resp["result"]
    else:
        out["error"] = resp.get("error")
    return out


def words(hexstr):
    """Ответ eth_call → список слов (int)."""
    h = hexstr[2:]
    return [int(h[i:i + 64], 16) for i in range(0, len(h), 64)]


def ladder(res, consts, swap0to1):
    """Суммы входа (единицы токена) — доли мнимого резерва входа обеих частей (приведённые → единицы токена)."""
    w = words(res["result"])
    # PoolWithReserves (статический кортеж): pool, token0, token1, fee, centerPrice, col[4], debt[6], limits[12].
    col = w[5:9]
    debt = w[9:15]
    cv2 = words(consts["result"])
    num, den = (cv2[0], cv2[1]) if swap0to1 else (cv2[2], cv2[3])
    imag_in = (col[2] + debt[4]) if swap0to1 else (col[3] + debt[5])
    tokens = imag_in * den // num if num else 0
    return sorted({int(tokens * f) for f in LADDER if int(tokens * f) > 0})


def main():
    ap = argparse.ArgumentParser(description="Запись ответов узла Arbitrum для тестов Fluid DEX T1 (решение 440)")
    ap.add_argument("--node", required=True, help="HTTP JSON-RPC своего узла Arbitrum One")
    ap.add_argument("--factory", required=True, help="FluidDexFactory (dex.factory)")
    ap.add_argument("--resolver", required=True, help="FluidDexReservesResolver (dex.quoter)")
    ap.add_argument("--wrapped", required=True, help="обёртка нативной монеты (dex.native_wrapped площадки arb_dex)")
    ap.add_argument("--block", type=int, help="номер блока (по умолчанию голова минус --lag)")
    ap.add_argument("--lag", type=int, default=20, help="отступ от головы, если --block не задан")
    ap.add_argument("--gt", action="store_true", help="записать страницу пулов GeckoTerminal fluid-arbitrum")
    a = ap.parse_args()

    chain = rpc(a.node, "eth_chainId", [])
    if a.block is None:
        head = int(rpc(a.node, "eth_blockNumber", [])["result"], 16)
        a.block = head - a.lag
    tag = hex(a.block)
    blk = rpc(a.node, "eth_getBlockByNumber", [tag, False])["result"]
    day = datetime.date.today().isoformat()
    root = os.path.join(os.path.dirname(__file__), "..", "..", "tests", "data", f"arbitrum_fluid_t1_{day}")
    root = os.path.normpath(root)
    os.makedirs(root, exist_ok=True)
    written = []

    def save(name, obj):
        path = os.path.join(root, name)
        with open(path, "w") as f:
            json.dump(obj, f, indent=1, sort_keys=True)
        written.append(name)

    save("block.json", {"chainId": chain.get("result"), "number": a.block, "hash": blk["hash"],
                        "timestamp": int(blk["timestamp"], 16), "factory": a.factory, "resolver": a.resolver, "wrapped": a.wrapped})
    listing = call(a.node, a.resolver, SEL["getAllPoolAddresses"], tag)
    save("pools.json", listing)
    if "result" not in listing:
        print("getAllPoolAddresses откатился:", listing.get("error"), file=sys.stderr)
        return 1
    w = words(listing["result"])
    pools = ["0x" + format(x, "040x") for x in w[2:2 + w[1]]]
    for p in pools:
        pw = word(p)
        rec = {
            "pool": p,
            "block": a.block,
            "reserves": call(a.node, a.resolver, SEL["getPoolReservesAdjusted"] + pw, tag),
            "constantsView": call(a.node, p, SEL["constantsView"], tag),
            "constantsView2": call(a.node, p, SEL["constantsView2"], tag),
            "vars": call(a.node, p, SEL["readFromStorage"] + word(0), tag),
            "vars2": call(a.node, p, SEL["readFromStorage"] + word(1), tag),
            "version": call(a.node, p, SEL["VERSION"], tag),
            "isDex": call(a.node, a.factory, SEL["isDex"] + pw, tag),
            "quotes": [],
        }
        if "result" in rec["reserves"] and "result" in rec["constantsView2"]:
            for d in (True, False):
                for amt in ladder(rec["reserves"], rec["constantsView2"], d):
                    args = word(d) + word(amt) + word(0)
                    rec["quotes"].append({
                        "swap0to1": d,
                        "amountIn": str(amt),
                        "pool": call(a.node, p, SEL["swapIn"] + args + word(DEAD), tag),
                        "resolver": call(a.node, a.resolver, SEL["estimateSwapIn"] + pw + args, tag),
                    })
        save(f"pool_{p.lower()}.json", rec)
        print(p, "обменов:", len(rec["quotes"]))
    if a.gt:
        url = "https://api.geckoterminal.com/api/v2/networks/arbitrum/dexes/fluid-arbitrum/pools?page=1"
        r = requests.get(url, headers={"accept": "application/json"}, timeout=60)
        save("geckoterminal_fluid_arbitrum_page1.json", {"url": url, "status": r.status_code, "body": r.json()})

    lines = [
        "# Fluid DEX T1, Arbitrum One — запись своего узла (решение 440)",
        "",
        f"Записано tools/arbitrum_record/fluid_t1.py {day}: блок {a.block} ({tag}), хеш {blk['hash']}, время "
        f"{int(blk['timestamp'], 16)}, chainId {chain.get('result')}.",
        f"Фабрика {a.factory}, резолвер {a.resolver}. Пулов: {len(pools)}. Ответы eth_call — как есть (result или error).",
        "Обмены: swapIn пула с получателем 0x…dEaD (откат FluidDexSwapResult — точный выход пула) и estimateSwapIn "
        "резолвера на тех же суммах. Ключей и подписей нет.",
        "",
        "SHA-256:",
        "",
    ]
    for n in sorted(written):
        with open(os.path.join(root, n), "rb") as f:
            lines.append(f"- `{hashlib.sha256(f.read()).hexdigest()}  {n}`")
    with open(os.path.join(root, "README.md"), "w") as f:
        f.write("\n".join(lines) + "\n")
    print("записано в", root)
    return 0


if __name__ == "__main__":
    sys.exit(main())
