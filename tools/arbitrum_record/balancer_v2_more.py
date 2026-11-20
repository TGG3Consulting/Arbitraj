#!/usr/bin/env python3
"""Запись ответов своего узла Arbitrum One для решения 473 (T450-03…07): пулы Balancer V2 фабрик composable stable,
stable phantom, meta-stable, LBP, investment, managed, linear.

Что делает (решение 460: сначала поиск без скана истории, потом всё состояние на ОДНОМ свежем блоке B):
  1) пулы — GeckoTerminal /networks/arbitrum/dexes/balancer_arbitrum/pools (страницы --gt-pages; reserve_in_usd каждой
     строки пишется — порядок ликвидности по видам), фабрика пула — та из --factory, чей isPoolFromFactory = 1;
     до --max-pools пулов на фабрику;
  2) на блоке B у каждого пула — ровно вызовы бота (balancer_v2_more_calls.py = load_balancer_more_fixed.rs read и
     load_balancer_more.rs calls): неизменяемое (getPoolId, isPoolFromFactory, getPoolTokens, decimals токенов, вызовы
     вида) и пачка каждого блока; ответы Vault.queryBatchSwap GIVEN_IN по всем упорядоченным парам токенов (с BPT)
     лесенкой долей баланса входа (--ladder); время и хеш блока.
Пишет tests/data/arbitrum_balancer_v2_more_<дата>/ (pools.json, gt.json, block.json, README.md с SHA-256). Только
чтение (eth_blockNumber, eth_getBlockByNumber, eth_call) и GeckoTerminal — ключей, подписей и транзакций нет.
Потолок времени поиска --time-limit (решение 460 п. 9): вышел — пишется найденное, код выхода 3.

Запуск (из корня репозитория; адреса — docs/notes/arbitrum/dex-settings-arbitrum.md §2.11, balancer-deployments@b55dc2b8
addresses/arbitrum.json; Multicall3 — github.com/mds1/multicall3 README, «Deployments», Arbitrum One):
    python3 tools/arbitrum_record/balancer_v2_more.py --rpc http://АДРЕС_УЗЛА:8547
(фабрики по умолчанию — FACTORIES ниже; --factory ИМЯ:вид:адрес — заменить список). Тест —
tests/arbitrum_balancer_v2_more_live.rs (после записи снять #[ignore]).
"""

import argparse
import datetime
import hashlib
import json
import os
import sys

import balancer_v2_more_calls as bc
import recnode  # решение 460: свежий блок состояния, потолок времени
import stdhttp as requests  # решение 459: стандартная библиотека вместо пакета requests
from curve_crypto_rpc import Node

GT = "https://api.geckoterminal.com/api/v2"
VAULT = "0xBA12222222228d8Ba445958a75a0704d566BF2C8"  # 20210418-vault output/arbitrum.json
MULTICALL3 = "0xcA11bde05977b3631167028862bE2a173976CA11"  # mds1/multicall3 README, Arbitrum One
# ИМЯ:вид:фабрика — balancer-deployments@b55dc2b8 addresses/arbitrum.json (решение 473, dex-settings-arbitrum.md §2.11).
FACTORIES = [
    "cs1:composable_stable:0xaEb406b0E430BF5Ea2Dc0B9Fe62E4E53f74B3a33",
    "cs2:composable_stable:0x85a80afee867aDf27B50BdB7b76DA70f1E853062",
    "cs4:composable_stable:0x2498A2B0d6462d2260EAC50aE1C3e03F4829BA95",
    "cs5:composable_stable:0xA8920455934Da4D853faac1f94Fe7bEf72943eF1",
    "cs6:composable_stable:0x4bdCc2fb18AEb9e2d281b0278D946445070EAda7",
    "sph:stable_phantom:0xdAE7e32ADc5d490a43cCba1f0c736033F2b4eFca",
    "ms:meta_stable:0xEBFD5681977E38Af65A7487DC70B8221D089cCAD",
    "lbp:lbp:0x142B9666a0a3A30477b052962ddA81547E7029ab",
    "lbpn:lbp:0x1802953277FD955f9a254B80Aa0582f193cF1d77",
    "inv:investment:0xaCd615B3705B9c880E4E7293f1030B34e57B4c1c",
    "mg1:managed:0x956CCab09898C0AF2aCa5e6C229c3aD4E93d9288",
    "mg2:managed:0x8eA89804145c007e7D226001A96955ad53836087",
    "la1:linear_v1:0xdcdbf71A870cc60C6F9B621E28a7D3Ffd6Dd4965",
    "le2:linear_v1:0x2794953110874981a0d301286c986992022A62a1",
    "lu:linear_v1:0x8878f1273466157a79570DdE1A42130e551B6EAd",
    "la2:linear:0xe2E901AB09f37884BA31622dF3Ca7FC19AA443Be",
    "la4:linear:0xf23b4DB826DbA14c0e857029dfF076b1c0264843",
    "la5:linear:0x7396f99B48e7436b152427bfA3DD6Aa8C7C6d05B",
    "ly1:linear:0xD8B6b96c88ad626EB6209c4876e3B14f45f8803A",
    "ly2:linear:0x19DFEF0a828EEC0c85FbB335aa65437417390b85",
    "le4:linear:0x7ADbdabaA80F654568421887c12F09E0C7BD9629",
]


def ask(node, call, tag):
    """eth_call вызова → тот же словарь с result / error (откат — ответ). Ошибок нет."""
    return dict(call, **node.call(call["to"], call["data"], tag))


def search(node, a, dl):
    """GeckoTerminal + isPoolFromFactory (на latest — только поиск; состояние — на блоке B). Возвращает: (пулы по
    фабрикам {спец: [адрес]}, строки GeckoTerminal, причина остановки или None). Ошибки: нет (TimeUp — причина)."""
    facs = [f.split(":") for f in a.factory]
    found, rows, stop = {f: [] for f in a.factory}, [], None
    try:
        for page in range(1, a.gt_pages + 1):
            dl.check(f"GeckoTerminal страница {page}")
            r = requests.get(f"{GT}/networks/arbitrum/dexes/balancer_arbitrum/pools", params={"page": page},
                             headers={"Accept": "application/json"}, timeout=60)
            if r.status_code != 200:
                rows.append({"page": page, "status": r.status_code})
                break
            for p in r.json().get("data", []):
                at = p.get("attributes", {})
                x = at.get("address", "")
                pool = "0x" + x[2:42].lower() if len(x) in (42, 66) else None
                row = {"page": page, "address": x, "name": at.get("name"), "reserve_usd": at.get("reserve_in_usd"),
                       "pool": pool, "factory": None}
                rows.append(row)
                if not pool:
                    continue
                dl.check("isPoolFromFactory")
                for spec, (_, _, f) in zip(a.factory, facs):
                    res = node.call(f, bc.selector("isPoolFromFactory(address)") + bc.addr_word(pool), "latest")
                    if res.get("result") and int(res["result"], 16) == 1:
                        row["factory"] = spec
                        if len(found[spec]) < a.max_pools:
                            found[spec].append(pool)
                        break
    except recnode.TimeUp as e:
        stop = str(e)
    return found, rows, stop


def read_pool(node, tag, spec, pool, a):
    """Все вызовы бота к пулу на блоке tag и лесенка queryBatchSwap. Возвращает: словарь пула. Ошибок нет (откат —
    ответ; не по ABI — причина в "skipped")."""
    name, ty, factory = spec.split(":")
    p = {"pool": pool, "factory": factory, "key": name, "type": ty}
    p["fixed"] = [ask(node, x, tag) for x in bc.fixed_calls(ty, pool, factory, a.vault)]
    pid = p["fixed"][0].get("result")
    if not pid or len(pid) != 66:
        p["skipped"] = "getPoolId() откатился"
        return p
    pt = ask(node, bc.c(a.vault, "getPoolTokens(bytes32)", pid[2:]), tag)
    try:
        tokens = [bc.addr(w) for w in bc.dyn(pt["result"], 0)]
        bals = [int(w, 16) for w in bc.dyn(pt["result"], 1)]
    except (KeyError, ValueError, IndexError) as e:
        p["skipped"] = f"getPoolTokens: {e}"
        return p
    p["tokens"], p["pool_tokens"] = tokens, pt
    p["decimals"] = [ask(node, bc.c(t, "decimals()"), tag) for t in tokens]
    p["type_fixed"] = [ask(node, x, tag) for x in bc.type_calls(ty, pool)]
    providers, bpt = [], None
    try:
        if ty in ("composable_stable", "meta_stable"):
            providers = [bc.addr(w) for w in bc.dyn(p["type_fixed"][-1]["result"], 0)]
        if ty in ("composable_stable", "stable_phantom") or ty in bc.LINEAR:
            bpt = int(p["type_fixed"][2 if ty in bc.LINEAR else 0]["result"], 16)
        if ty == "managed":
            bpt = tokens.index(pool) if pool in tokens else None
    except (KeyError, ValueError, IndexError) as e:
        p["skipped"] = f"неизменяемое вида: {e}"
        return p
    p["batch"] = [ask(node, x, tag) for x in bc.batch_calls(ty, pool, a.vault, pid, tokens, providers, bpt,
                                                           a.multicall)]
    me = "0x" + "00" * 19 + "01"
    q = []
    for i in range(len(tokens)):
        for j in range(len(tokens)):
            for den in ([] if i == j else a.ladder):
                if bals[i] // den:
                    q.append(ask(node, bc.query_batch_swap(a.vault, pid, i, j, bals[i] // den, tokens, me), tag))
    p["query"] = q
    return p


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum One")
    ap.add_argument("--vault", default=VAULT)
    ap.add_argument("--multicall", default=MULTICALL3)
    ap.add_argument("--factory", action="append", default=None, help="ИМЯ:вид:фабрика (повторять)")
    ap.add_argument("--gt-pages", type=int, default=10)
    ap.add_argument("--max-pools", type=int, default=3)
    ap.add_argument("--ladder", type=int, action="append", default=None, help="делители баланса входа")
    ap.add_argument("--time-limit", type=int, default=900, help="потолок времени поиска, с (0 — без потолка)")
    a = ap.parse_args()
    a.factory = a.factory or FACTORIES
    a.ladder = a.ladder or [100000, 1000, 100]
    node = Node(a.rpc)
    say = lambda t: print(t, file=sys.stderr, flush=True)  # noqa: E731
    found, rows, stop = search(node, a, recnode.Deadline(a.time_limit))
    if stop:
        say(f"поиск остановлен: {stop}")

    def phase(head):
        tag = hex(head)
        blk = node.rpc("eth_getBlockByNumber", [tag, False])["result"]
        say(f"блок состояния B {head}")
        pools = [read_pool(node, tag, spec, pool, a) for spec in a.factory for pool in found[spec]]
        return head, blk, pools

    head, blk, pools = recnode.fresh(lambda: int(node.rpc("eth_blockNumber", [])["result"], 16), phase, say=say)
    day = os.environ.get("ARB_RECORD_DATE") or datetime.date.today().isoformat()
    root = os.path.join(os.path.dirname(__file__), "..", "..", "tests", "data", f"arbitrum_balancer_v2_more_{day}")
    os.makedirs(root, exist_ok=True)
    meta = {"block": head, "hash": blk["hash"], "timestamp": int(blk["timestamp"], 16), "vault": a.vault,
            "multicall": a.multicall, "stopped": stop, "args": vars(a)}
    files = {"pools.json": pools, "gt.json": rows, "block.json": meta}
    sums = []
    for n, data in files.items():
        raw = json.dumps(data, indent=1, ensure_ascii=False).encode()
        with open(os.path.join(root, n), "wb") as fh:
            fh.write(raw)
        sums.append(f"| {n} | {hashlib.sha256(raw).hexdigest()} |")
    lines = [f"# arbitrum_balancer_v2_more_{day} — tools/arbitrum_record/balancer_v2_more.py (решение 473)", "",
             f"Блок {head} ({blk['hash']}), время {meta['timestamp']}. Только чтение (eth_call, eth_getBlockByNumber) и "
             "GeckoTerminal. gt.json — строки balancer_arbitrum с reserve_in_usd и фабрикой пула (порядок ликвидности).",
             ("Поиск остановлен: " + stop + "." if stop else "Поиск уложился в потолок времени."),
             "Тест — tests/arbitrum_balancer_v2_more_live.rs (снять #[ignore]).", "", "| Файл | SHA-256 |",
             "| --- | --- |"] + sums
    with open(os.path.join(root, "README.md"), "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")
    print(root)
    if stop:
        sys.exit(3)


if __name__ == "__main__":
    main()
