#!/usr/bin/env python3
# Файл: tools/arbitrum_record/balancer_v3_buffer.py
# Что делает: решение 474 — запись ответов своего узла Arbitrum для обмена Balancer V3 через буфер ERC4626 хранилища
#   (boosted-пулы: токен пула — обёртка ERC4626, монета — её базовый токен) на ОДНОМ блоке (только чтение, eth_call без
#   from; ключей и подписей нет). Пулы — перечислением фабрик (getPools() каждой фабрики из --list, без GeckoTerminal и
#   без скана истории); берутся пулы, у которых хотя бы у одного токена инициализирован буфер (getERC4626BufferAsset ≠ 0).
#   По каждому пулу: все вызовы бота к пулу (как balancer_v3.py) и к буферам (areBuffersPaused, getMinimumWrapAmount,
#   getERC4626BufferAsset, getBufferBalance; у обёртки asset(), previewDeposit / previewRedeem опорной суммы
#   --preview-raw); ответы BatchRouter querySwapExactIn на путь «буфер → пул → буфер» (и с буфером только на одной
#   стороне) на несколько сумм; точные превью обёртки на суммах обмена (previewDeposit(вход − 1); previewRedeem(выход
#   пула − 1), выход пула — тот же BatchRouter без последнего шага) — тест сверяет расчёт бота побитово при точных
#   превью и оценивает расхождение превью по одной точке; события Wrap / Unwrap / LiquidityAddedToBuffer /
#   LiquidityRemovedFromBuffer хранилища узким окном последних --log-blocks блоков.
# Кто вызывает: Тигран вручную или record_all.py (record_steps.py); тест — tests/arbitrum_balancer_v3_buffer.rs.
# От чего зависит: Python 3 (стандартная библиотека; HTTP — stdhttp.py), balancer_v3.py этой папки (keccak, узел,
#   списки вызовов бота). Адреса — официальные (balancer/balancer-deployments@b55dc2b8 output/arbitrum.json: Vault
#   v3/tasks/20241204-v3-vault, BatchRouter v3/tasks/20241205-v3-batch-router; фабрики — docs/FIRST_RUN.md).
#
# Пример:
#   python3 tools/arbitrum_record/balancer_v3_buffer.py --rpc http://127.0.0.1:8547 \
#     --vault 0xbA1333333333a1BA1108E8412f11850A5C319bA9 --batch-router 0xaD89051bEd8d96f045E8912aE1672c6C0bF8a85E \
#     --preview-raw 1000000000000000000000000000000 --list dex.bv3_weighted_factories=0xD961… (все списки — как
#     у balancer_v3.py, record_steps.py BV3_LISTS) --max-pools 30

import argparse
import datetime
import hashlib
import json
import os
import sys
import time

import balancer_v3 as bv3

DIVS = (1_000_000, 10_000, 1_000, 100)
Q = "querySwapExactIn((address,(address,address,bool)[],uint256,uint256)[],address,bytes)"


def query_data(token_in, steps, amount):
    """Данные querySwapExactIn с одним путём. steps — [(pool, tokenOut, isBuffer)]. Кодирование ABI вручную."""
    w = bv3.word_int
    n = len(steps)
    path = [w(1), w(0x20), bv3.word_addr(token_in), w(0x80), w(amount), w(0), w(n)]
    for pool, out, buf in steps:
        path += [bv3.word_addr(pool), bv3.word_addr(out), w(1 if buf else 0)]
    head = [w(0x60), bv3.word_addr("0x0"), w(0x60 + 32 * len(path))]
    return bv3.sel(Q) + "".join(head + path + [w(0)])


def first_out(rec):
    """pathAmountsOut[0] из ответа querySwapExactIn или None."""
    if "result" not in rec:
        return None
    ws = bv3.words(rec["result"])
    arr = bv3.dyn_array(ws, 0)
    return arr[0] if arr else None


def addr_of(rec):
    return "0x" + format(bv3.words(rec["result"])[0], "040x") if "result" in rec else None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rpc", required=True, help="HTTP JSON-RPC своего узла Arbitrum")
    ap.add_argument("--vault", required=True)
    ap.add_argument("--batch-router", required=True)
    ap.add_argument("--preview-raw", required=True, help="опорная сумма превью (dex.bv3_buffer_preview_raw)")
    ap.add_argument("--list", action="append", default=[], help="настройка=адрес,адрес — как у balancer_v3.py")
    ap.add_argument("--arb-sys", default="0x0000000000000000000000000000000000000064")
    ap.add_argument("--arb-owner-public", default="0x000000000000000000000000000000000000006b")
    ap.add_argument("--max-pools", type=int, default=30, help="сколько пулов с буфером записать")
    ap.add_argument("--log-blocks", type=int, default=2000, help="окно событий буфера — последние N блоков")
    ap.add_argument("--limit-s", type=int, default=1500, help="потолок времени записи пулов, секунд")
    ap.add_argument("--out", default="")
    a = ap.parse_args()
    t0 = time.time()
    node = bv3.Node(a.rpc)
    head = int(node.rpc("eth_blockNumber", [])["result"], 16)
    tag = hex(head)
    blk = node.rpc("eth_getBlockByNumber", [tag, False])["result"]
    chain = node.rpc("eth_chainId", [])["result"]
    date = datetime.datetime.fromtimestamp(int(blk["timestamp"], 16), datetime.timezone.utc).strftime("%Y-%m-%d")
    out = a.out or os.path.join("tests", "data", f"arbitrum_bv3_buffer_{date}")
    os.makedirs(out, exist_ok=True)
    lists = {}
    for item in a.list:
        name, _, addrs = item.partition("=")
        lists[name.strip()] = [x.strip() for x in addrs.split(",") if x.strip()]
    factories = [f for n, l in lists.items() if n.endswith("_factories") for f in l]
    v, r = a.vault, int(a.preview_raw)
    call = lambda to, data: node.call(to, data, tag)  # noqa: E731

    # Пулы — перечислением фабрик (состояние на блоке tag), отбор — токен с буфером.
    found = []
    for f in factories:
        rec = call(f, bv3.sel("getPools()"))
        if "result" not in rec:
            print("фабрика", f, "getPools откатился:", rec.get("error"), file=sys.stderr)
            continue
        for p in bv3.dyn_array(bv3.words(rec["result"]), 0):
            pool = "0x" + format(p, "040x")
            if pool not in found:
                found.append(pool)
    pools = []
    for pool in found:
        if len(pools) >= a.max_pools or time.time() - t0 > a.limit_s:
            break
        info = call(v, bv3.sel("getPoolTokenInfo(address)") + bv3.word_addr(pool))
        if "result" not in info:
            continue
        ws = bv3.words(info["result"])
        tokens = ["0x" + format(t, "040x") for t in bv3.dyn_array(ws, 0)]
        under = [addr_of(call(v, bv3.sel("getERC4626BufferAsset(address)") + bv3.word_addr(t))) for t in tokens]
        if any(u and int(u, 16) for u in under):
            pools.append((pool, tokens, bv3.dyn_array(ws, 2), under))

    meta = {"chain_id": chain, "block": head, "block_hash": blk["hash"], "timestamp": int(blk["timestamp"], 16),
            "vault": v, "batch_router": a.batch_router, "preview_raw": str(r), "lists": lists,
            "arb_sys": a.arb_sys, "arb_owner_public": a.arb_owner_public, "factory_pools": len(found),
            "pools": [p[0] for p in pools]}
    with open(os.path.join(out, "meta.json"), "w") as fh:
        json.dump(meta, fh, indent=1)

    for pool, tokens, raw, under in pools:
        pw = bv3.word_addr(pool)
        rec = {"pool": pool, "tokens": tokens, "underlying": under, "calls": [], "quotes": [], "previews": []}
        add = lambda to, data: rec["calls"].append(call(to, data))  # noqa: E731
        for sig in bv3.VAULT_POOL:
            add(v, bv3.sel(sig) + pw)
        add(v, bv3.sel("getMinimumTradeAmount()"))
        add(v, bv3.sel("isVaultPaused()"))
        for f in factories:
            add(f, bv3.sel("isPoolFromFactory(address)") + pw)
        for sig in bv3.POOL_VIEWS:
            add(pool, bv3.sel(sig))
        hw = bv3.words(rec["calls"][0]["result"]) if "result" in rec["calls"][0] else []
        if len(hw) >= 11 and hw[10] != 0:
            hook = "0x" + format(hw[10], "040x")
            for sig in bv3.HOOK_POOL:
                add(hook, bv3.sel(sig) + pw)
            for sig in bv3.HOOK_GLOBAL:
                add(hook, bv3.sel(sig))
            add(a.arb_sys, bv3.sel("arbOSVersion()"))
            add(a.arb_owner_public, bv3.sel("getCollectTips()"))
        add(v, bv3.sel("areBuffersPaused()"))
        add(v, bv3.sel("getMinimumWrapAmount()"))
        for t in tokens:
            tw = bv3.word_addr(t)
            add(v, bv3.sel("getERC4626BufferAsset(address)") + tw)
            add(v, bv3.sel("getBufferBalance(address)") + tw)
            add(t, bv3.sel("asset()"))
            add(t, bv3.sel("previewDeposit(uint256)") + bv3.word_int(r))
            add(t, bv3.sel("previewRedeem(uint256)") + bv3.word_int(r))
        buf = [bool(u and int(u, 16)) for u in under]
        for i, ti in enumerate(tokens):
            for j, tj in enumerate(tokens):
                if i == j:
                    continue
                for bi, bj in ((True, True), (True, False), (False, True)):
                    if (bi and not buf[i]) or (bj and not buf[j]):
                        continue
                    t_in = under[i] if bi else ti
                    t_out = under[j] if bj else tj
                    for div in DIVS:
                        amt = raw[i] // div
                        if amt < 2:
                            continue
                        steps = ([(ti, ti, True)] if bi else []) + [(pool, tj, False)]
                        q = {"i": i, "j": j, "wrap_in": bi, "unwrap_out": bj, "token_in": t_in, "token_out": t_out,
                             "amount": str(amt)}
                        if bi:
                            p = call(ti, bv3.sel("previewDeposit(uint256)") + bv3.word_int(amt - 1))
                            rec["previews"].append(dict(p, wrapper=ti, kind="deposit", amount=str(amt - 1)))
                        part = call(a.batch_router, query_data(t_in, steps, amt))
                        q["pool_out"] = dict(part)
                        o = first_out(part)
                        if bj and o is not None and o >= 2:
                            p = call(tj, bv3.sel("previewRedeem(uint256)") + bv3.word_int(o - 1))
                            rec["previews"].append(dict(p, wrapper=tj, kind="redeem", amount=str(o - 1)))
                        full = steps + ([(tj, t_out, True)] if bj else [])
                        q["answer"] = dict(call(a.batch_router, query_data(t_in, full, amt)))
                        rec["quotes"].append(q)
        with open(os.path.join(out, f"pool_{pool.lower()}.json"), "w") as fh:
            json.dump(rec, fh, indent=1)
        print(pool, "вызовов:", len(rec["calls"]), "квот:", len(rec["quotes"]), file=sys.stderr)

    # События буфера — узкое окно (лог от хранилища; topic1 — обёртка).
    topics = [bv3.keccak256(s.encode()).hex() for s in (
        "Wrap(address,uint256,uint256,bytes32)", "Unwrap(address,uint256,uint256,bytes32)",
        "LiquidityAddedToBuffer(address,uint256,uint256,bytes32)",
        "LiquidityRemovedFromBuffer(address,uint256,uint256,bytes32)")]
    lg = node.rpc("eth_getLogs", [{"address": v, "fromBlock": hex(max(0, head - a.log_blocks)), "toBlock": tag,
                                   "topics": [["0x" + t for t in topics]]}])
    with open(os.path.join(out, "buffer_logs.json"), "w") as fh:
        json.dump(lg, fh, indent=1)

    lines = ["# Balancer V3 через буфер ERC4626 — запись своего узла (решение 474)", "",
             f"Блок {head}, хеш {blk['hash']}, время {blk['timestamp']} ({date}), chainId {chain}.",
             "Только eth_call / eth_getLogs, без from; ключей и подписей нет. Скрипт — "
             "tools/arbitrum_record/balancer_v3_buffer.py.",
             f"Хранилище {v}, BatchRouter {a.batch_router}, опорная сумма превью {r}. Пулов фабрик {len(found)}, "
             f"с буфером записано {len(pools)}.", "",
             "Файлы: meta.json; pool_<адрес>.json — calls (вызовы бота к пулу и буферам: to, data, result / error), "
             "quotes (BatchRouter querySwapExactIn: i, j, wrap_in, unwrap_out, сумма; pool_out — путь без последнего "
             "буфера, answer — весь путь), previews (точные previewDeposit / previewRedeem обёртки на суммах обмена); "
             f"buffer_logs.json — события буфера хранилища за {a.log_blocks} блоков.", "", "## SHA-256 файлов"]
    for name in sorted(os.listdir(out)):
        if name != "README.md":
            with open(os.path.join(out, name), "rb") as fh:
                lines.append(f"{hashlib.sha256(fh.read()).hexdigest()}  {name}")
    with open(os.path.join(out, "README.md"), "w") as fh:
        fh.write("\n".join(lines) + "\n")
    print("записано:", out, file=sys.stderr)


if __name__ == "__main__":
    main()
