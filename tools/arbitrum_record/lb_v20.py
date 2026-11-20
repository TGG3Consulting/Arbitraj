#!/usr/bin/env python3
# Файл: tools/arbitrum_record/lb_v20.py
# Что делает: решение 445 — запись ответов СВОЕГО узла Arbitrum One для семейства Liquidity Book v2.0 (LFJ V2 /
#   Trader Joe V2, вид lb_v20): на одном закреплённом блоке — фабрика v2.0 (число пар, адреса пар, реализация пары и её
#   код), по каждому пулу — factory / tokenX / tokenY / feeParameters / getReservesAndId, реестр фабрики
#   getLBPairInformation, ВСЕ непустые бины в обе стороны (findFirstNonEmptyBinId до отката или предела --max-bins,
#   getBin), ответы роутера LBRouter v2.0 getSwapOut на ряд сумм в обе стороны (и откаты), слоты хранилища условия
#   пулов (6, 7, 10), логи событий пула за последние --log-blocks блоков, заголовок блока (время, хеш), eth_chainId.
#   Только чтение: eth_call / eth_getStorageAt / eth_getCode / eth_getLogs / eth_getBlockByNumber / eth_chainId.
#   Ключей, подписей и транзакций нет. Пишет сырые пары «запрос → ответ» JSON-RPC.
# Кто вызывает: Тигран вручную (tests/*: tests/bsc_dex_lb_v20_arbitrum.rs читает записанное).
# От чего зависит: Python 3, requests.
# Запуск: python3 tools/arbitrum_record/lb_v20.py http://<узел>:8547 [--pools 0x…,0x…] [--max-pools 5]
#   [--max-bins 300] [--log-blocks 20000] --factory <LBFactory v2.0> --router <LBRouter v2.0>
#   Адреса фабрики и роутера v2.0 — обязательные аргументы (ревью 445: без зашитых адресов). Официальные значения —
#   документация LFJ (traderjoe-xyz/joe-docs 61c16665 versioned_docs/version-V2.2/deployment-addresses/arbitrum.md
#   L52-L58): LBFactory 0x1886D09C9Ade0c5DB822D85D21678Db67B6c2982, LBRouter 0x7BFd7192E76D950832c77BB412aaE841049D8D9B.
# Выход: tests/data/arbitrum_lb_v20_<дата UTC>/ — meta.json, factory.json, pool_<адрес>.json, logs_<адрес>.json,
#   README.md.
#
# Селекторы и topic0 — keccak256 подписей из исходника lfj-gg/joe-v2@v2.0.0 (07f10212): src/interfaces/ILBFactory.sol,
# ILBPair.sol, ILBRouter.sol; тест на записи сверяет их с кодированием alloy (pools/abi_lb20.rs).

import argparse
import datetime
import hashlib
import json
import os
import sys

import requests

SEL = {
    "getNumberOfLBPairs()": "0x4e937c3a",
    "allLBPairs(uint256)": "0x72e47b8c",
    "LBPairImplementation()": "0x509ceb90",
    "getLBPairInformation(address,address,uint256)": "0x704037bd",
    "factory()": "0xc45a0155",
    "tokenX()": "0x16dc165b",
    "tokenY()": "0xb7d19fc4",
    "feeParameters()": "0x98c7adf3",
    "getReservesAndId()": "0x1b05b83e",
    "getBin(uint24)": "0x0abe9688",
    "findFirstNonEmptyBinId(uint24,bool)": "0x8f919a83",
    "getSwapOut(address,uint256,bool)": "0x2004b724",
    "decimals()": "0x313ce567",
}

TOPICS = {
    "Swap": "0xc528cda9e500228b16ce84fadae290d9a49aecb17483110004c5af0a07f6fd73",
    "DepositedToBin": "0x4216cc3bd0c40a90259d92f800c06ede5c47765f41a488072b7e7104a1f95841",
    "WithdrawnFromBin": "0xda5e7177dface55f5e0eff7dfc67420a1db4243ddfcf0ecc84ed93e034dd8cc2",
    "CompositionFee": "0x56f8e764728c77dd99ffbc1b64e6d02e227e6ec8214f165d4ef31351de136a0d",
}

# Данные отката «бинов в эту сторону нет»: селектор TreeMath__ErrorDepthSearch() и Panic(0x11) (src/libraries/TreeMath.sol
# L60; BitMath.sol L19 — переполнение uint8). Иной откат — не край стороны (конец записи — «error»).
EDGE = {"0x10d64861", "0x4e487b71" + format(0x11, "064x")}

# Доли суммарного резерва входного токена пары для getSwapOut: от пыли до больше пула (ожидается откат роутера).
FRACTIONS = [(1, 1_000_000), (1, 10_000), (1, 1_000), (1, 100), (1, 10), (1, 2), (2, 1)]


class Rpc:
    """JSON-RPC узла: каждая пара «запрос → ответ» копится в журнал (сырые ответы, как отдал узел)."""

    def __init__(self, url):
        self.url = url
        self.n = 0
        self.s = requests.Session()

    def call(self, method, params):
        self.n += 1
        req = {"jsonrpc": "2.0", "id": self.n, "method": method, "params": params}
        r = self.s.post(self.url, json=req, timeout=60)
        r.raise_for_status()
        return {"request": req, "response": r.json()}


def word(x):
    return format(x, "064x")


def addr_word(a):
    return word(int(a, 16))


def result(rec):
    """Байты ответа eth_call или None при откате/ошибке."""
    return rec["response"].get("result")


def u256(hexs, i=0):
    h = hexs[2:] if hexs.startswith("0x") else hexs
    return int(h[64 * i : 64 * (i + 1)], 16)


def addr_of(hexs, i=0):
    return "0x" + format(u256(hexs, i), "040x")


def eth_call(rpc, to, data, tag):
    return rpc.call("eth_call", [{"to": to, "input": data}, tag])


def main():
    ap = argparse.ArgumentParser(description="Запись ответов узла Arbitrum для Liquidity Book v2.0 (решение 445)")
    ap.add_argument("node", help="адрес HTTP JSON-RPC своего узла Arbitrum One")
    ap.add_argument("--factory", required=True, help="адрес LBFactory v2.0")
    ap.add_argument("--router", required=True, help="адрес LBRouter v2.0 (getSwapOut)")
    ap.add_argument("--pools", default="", help="адреса пулов через запятую (иначе — первые непустые из фабрики)")
    ap.add_argument("--max-pools", type=int, default=5)
    ap.add_argument("--max-bins", type=int, default=300, help="предел бинов на сторону")
    ap.add_argument("--log-blocks", type=int, default=20000, help="сколько последних блоков логов пула")
    ap.add_argument("--out", default="")
    a = ap.parse_args()

    rpc = Rpc(a.node)
    chain = rpc.call("eth_chainId", [])
    head = rpc.call("eth_blockNumber", [])
    block = int(head["response"]["result"], 16)
    tag = hex(block)
    header = rpc.call("eth_getBlockByNumber", [tag, False])
    hdr = header["response"]["result"]
    day = datetime.datetime.fromtimestamp(int(hdr["timestamp"], 16), datetime.timezone.utc).strftime("%Y-%m-%d")
    out = a.out or os.path.join("tests", "data", f"arbitrum_lb_v20_{day}")
    os.makedirs(out, exist_ok=True)

    # Фабрика: число пар, реализация пары и её код (для сверки байткода с исходником v2.0.0 вне бота).
    fac = {"factory": a.factory, "router": a.router, "calls": []}
    num = eth_call(rpc, a.factory, SEL["getNumberOfLBPairs()"], tag)
    impl = eth_call(rpc, a.factory, SEL["LBPairImplementation()"], tag)
    fac["calls"] += [num, impl]
    if result(impl):
        code = rpc.call("eth_getCode", [addr_of(result(impl)), tag])
        fac["implementation_code"] = code
        raw = bytes.fromhex(code["response"].get("result", "0x")[2:])
        fac["implementation_code_sha256"] = hashlib.sha256(raw).hexdigest()
        fac["implementation_code_size"] = len(raw)

    pools = [p.strip() for p in a.pools.split(",") if p.strip()]
    if not pools and result(num):
        for i in range(u256(result(num))):
            if len(pools) >= a.max_pools:
                break
            r = eth_call(rpc, a.factory, SEL["allLBPairs(uint256)"] + word(i), tag)
            fac["calls"].append(r)
            if not result(r):
                continue
            p = addr_of(result(r))
            st = eth_call(rpc, p, SEL["getReservesAndId()"], tag)
            fac["calls"].append(st)
            if result(st) and u256(result(st), 0) > 0 and u256(result(st), 1) > 0:
                pools.append(p)
    with open(os.path.join(out, "factory.json"), "w") as f:
        json.dump(fac, f, indent=1)

    names = []
    for p in pools:
        rec = record_pool(rpc, a, p, tag)
        name = f"pool_{p.lower()}.json"
        with open(os.path.join(out, name), "w") as f:
            json.dump(rec, f, indent=1)
        logs = rpc.call(
            "eth_getLogs",
            [{
                "address": p,
                "fromBlock": hex(max(0, block - a.log_blocks)),
                "toBlock": tag,
                "topics": [list(TOPICS.values())],
            }],
        )
        with open(os.path.join(out, f"logs_{p.lower()}.json"), "w") as f:
            json.dump({"pool": p, "topics": TOPICS, "logs": logs}, f, indent=1)
        names.append((p, rec["summary"]))

    meta = {
        "what": "Liquidity Book v2.0 (LFJ V2), Arbitrum One — решение 445",
        "block": block,
        "block_hash": hdr["hash"],
        "timestamp": int(hdr["timestamp"], 16),
        "chain_id": chain,
        "head": head,
        "header": header,
        "factory": a.factory,
        "router": a.router,
        "pools": pools,
        "max_bins": a.max_bins,
        "log_blocks": a.log_blocks,
    }
    with open(os.path.join(out, "meta.json"), "w") as f:
        json.dump(meta, f, indent=1)
    write_readme(out, meta, names, fac)
    print(f"записано: {out} (блок {block}, пулов {len(pools)})")


def record_pool(rpc, a, p, tag):
    rec = {"pool": p, "calls": {}, "bins": {"down": [], "up": [], "active": None}, "quotes": [], "storage": {}}
    for name in ["factory()", "tokenX()", "tokenY()", "feeParameters()", "getReservesAndId()"]:
        rec["calls"][name] = eth_call(rpc, p, SEL[name], tag)
    x = addr_of(result(rec["calls"]["tokenX()"]))
    y = addr_of(result(rec["calls"]["tokenY()"]))
    fp = result(rec["calls"]["feeParameters()"])
    step = u256(fp, 0)
    info = SEL["getLBPairInformation(address,address,uint256)"] + addr_word(x) + addr_word(y) + word(step)
    rec["calls"]["getLBPairInformation"] = eth_call(rpc, a.factory, info, tag)
    for t, n in [(x, "decimalsX"), (y, "decimalsY")]:
        rec["calls"][n] = eth_call(rpc, t, SEL["decimals()"], tag)

    rs = result(rec["calls"]["getReservesAndId()"])
    active = u256(rs, 2)
    rec["bins"]["active"] = eth_call(rpc, p, SEL["getBin(uint24)"] + word(active), tag)
    ends = {}
    for side, down in [("down", True), ("up", False)]:
        cur = active
        ends[side] = "limit"
        for _ in range(a.max_bins):
            nx = eth_call(rpc, p, SEL["findFirstNonEmptyBinId(uint24,bool)"] + word(cur) + word(1 if down else 0), tag)
            if not result(nx):
                rec["bins"][side].append({"next": nx})
                data = (nx["response"].get("error") or {}).get("data", "")
                ends[side] = "revert" if str(data).lower() in EDGE else "error"
                break
            nid = u256(result(nx))
            b = eth_call(rpc, p, SEL["getBin(uint24)"] + word(nid), tag)
            rec["bins"][side].append({"next": nx, "bin": b})
            cur = nid
    rec["bins"]["ends"] = ends

    for swap_for_y, total in [(True, u256(rs, 0)), (False, u256(rs, 1))]:
        base = total if total > 0 else 10**18
        for num, den in FRACTIONS:
            amt = max(1, base * num // den)
            data = SEL["getSwapOut(address,uint256,bool)"] + addr_word(p) + word(amt) + word(1 if swap_for_y else 0)
            r = eth_call(rpc, a.router, data, tag)
            rec["quotes"].append({"swapForY": swap_for_y, "amountIn": str(amt), "call": r})

    for s in [6, 7, 10]:
        rec["storage"][str(s)] = rpc.call("eth_getStorageAt", [p, hex(s), tag])
    rec["code"] = rpc.call("eth_getCode", [p, tag])
    rec["summary"] = {
        "tokenX": x,
        "tokenY": y,
        "binStep": step,
        "activeId": active,
        "bins_down": len([b for b in rec["bins"]["down"] if "bin" in b]),
        "bins_up": len([b for b in rec["bins"]["up"] if "bin" in b]),
        "ends": ends,
        "quotes_ok": len([q for q in rec["quotes"] if result(q["call"])]),
        "quotes": len(rec["quotes"]),
    }
    return rec


def write_readme(out, meta, names, fac):
    lines = [
        "# Arbitrum One — Liquidity Book v2.0 (LFJ V2), запись своего узла (решение 445)",
        "",
        "Записано скриптом `tools/arbitrum_record/lb_v20.py` (только чтение; ключей, подписей и транзакций нет).",
        "",
        f"- Блок: {meta['block']}, хеш {meta['block_hash']}, время {meta['timestamp']} (UTC "
        f"{datetime.datetime.fromtimestamp(meta['timestamp'], datetime.timezone.utc).isoformat()})",
        f"- eth_chainId: {meta['chain_id']['response'].get('result')}",
        f"- Фабрика LBFactory v2.0: {meta['factory']}; роутер LBRouter v2.0 (квотер getSwapOut): {meta['router']}",
        f"- Реализация пары: код {fac.get('implementation_code_size')} байт, SHA-256 {fac.get('implementation_code_sha256')}"
        " (сверка с исходником lfj-gg/joe-v2@v2.0.0 — вне бота)",
        f"- Предел бинов на сторону: {meta['max_bins']}; логи — {meta['log_blocks']} блоков до записанного",
        "",
        "## Файлы",
        "- `meta.json` — блок, заголовок блока, eth_chainId, адреса, список пулов.",
        "- `factory.json` — getNumberOfLBPairs, LBPairImplementation, код реализации, allLBPairs(i) и getReservesAndId при отборе.",
        "- `pool_<адрес>.json` — factory/tokenX/tokenY/feeParameters/getReservesAndId, getLBPairInformation фабрики,",
        "  decimals токенов, getBin активного бина, обход бинов (findFirstNonEmptyBinId + getBin; конец стороны —",
        "  откат или предел), getSwapOut роутера на долях резерва в обе стороны (с откатами), слоты 6, 7, 10, код пула.",
        "- `logs_<адрес>.json` — eth_getLogs пула (Swap, DepositedToBin, WithdrawnFromBin, CompositionFee).",
        "",
        "## Пулы",
    ]
    for p, s in names:
        lines.append(f"- {p}: X {s['tokenX']}, Y {s['tokenY']}, шаг {s['binStep']}, активный {s['activeId']}, "
                     f"бинов вниз {s['bins_down']} ({s['ends']['down']}), вверх {s['bins_up']} ({s['ends']['up']}), "
                     f"getSwapOut успешных {s['quotes_ok']} из {s['quotes']}")
    with open(os.path.join(out, "README.md"), "w") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    sys.exit(main())
