"""Файл: tools/arbitrum_record/v4live_mgr.py
Что делает: решение 459 (T457-02) — пулы шагов-менеджеров исполнителя v4 на Arbitrum, которые судит правило приёмки
  решения 400 (src/venues/bsc_dex/exec/executor/live_class.rs CLASSES): UNI_V4 = 19 (Uniswap V4 PoolManager) и
  BALANCER_V3 = 23 (Vault V3). Для каждого вида: поиск пулов на узле, шаг исполнителя (kind, pool, data — как
  contracts/ArbExecutor.sol Step), квотер (eth_call на H и тот же вызов внутри eth_simulateV1 — «квотер в том же
  состоянии»), кредит события Swap менеджера из логов вызова.
  Uniswap V4 (Uniswap/docs v4/deployments.mdx, Arbitrum; docs/notes/arbitrum/dex-settings-arbitrum.md §2.3): пулы —
  события Initialize PoolManager за окно блоков; ликвидность — StateView.getLiquidity; квотер — V4Quoter
  quoteExactInputSingle; кредит — Swap(PoolId, sender, amount0, amount1, …) менеджера. Нативная валюта (address 0)
  — не шаг исполнителя (ArbExecutor.sol, UNI_V4) — такие пулы не берутся; с крючком — не берутся (класс «с крючком»
  правило не принимает никогда, live_rule.rs).
  Balancer V3 (docs/FIRST_RUN.md «Balancer V3»; pools/abi_balancer_v3.rs): пулы — из записи balancer_v3.py этой же
  команды (meta.json pools) и --bv3-pool; токены — Vault.getPoolTokenInfo; квотер — Router
  querySwapSingleTokenExactIn без from (Vault.quote: tx.origin = 0); кредит — Swap(pool, tokenIn, tokenOut, amountIn,
  amountOut, …) хранилища. В строке записи hooks = 0: у шага BALANCER_V3 класс только «без крючка» (live_class.rs);
  крючок пула — поле bv3_hook.
Кто вызывает: tools/arbitrum_record/exec_v4_live.py.
От чего зависит: v4live_rpc.py.
Ошибки: сбой узла — исключение как есть; пул, у которого вызов не ответил, пропускается (причина — в списке skipped).
"""

import glob
import json
import os

from curve_crypto_rpc import keccak256
from v4live_rpc import ZERO, ad, adr, sel, sword, topic, w, word

V4_INIT = topic("Initialize(bytes32,address,address,uint24,int24,address,uint160,int24)")
V4_SWAP = topic("Swap(bytes32,address,int128,int128,uint160,uint128,int24,uint24)")
V4_QSIG = "quoteExactInputSingle(((address,address,uint24,int24,address),bool,uint128,bytes))"
BV3_SWAP = topic("Swap(address,address,address,uint256,uint256,uint256,uint256)")
BV3_QSIG = "querySwapSingleTokenExactIn(address,address,address,uint256,address,bytes)"


def _s24(x):
    x &= 0xFFFFFF
    return x - (1 << 24) if x >> 23 else x


def v4_pools(node, tag, head, a, skipped):
    """Пулы Uniswap V4 без крючка, обе валюты ERC-20, ликвидность > 0 (новые — первыми). Ошибок-исключений нет, кроме
    сетевых."""
    found, lo = [], max(0, head - a.v4_blocks)
    hi, chunk = head, a.log_chunk
    while hi >= lo and len(found) < a.v4_scan:
        start = max(lo, hi - chunk + 1)
        j = node.rpc("eth_getLogs", [{"address": a.v4_pool_manager, "topics": [V4_INIT],
                                      "fromBlock": hex(start), "toBlock": hex(hi)}])
        if "result" not in j:
            # Узел отказал по диапазону / объёму — часть вчетверо меньше (до 1000 блоков), затем причина в skipped.
            if chunk > 1000:
                chunk = max(1000, chunk // 4)
                continue
            skipped.append({"kind": 19, "why": f"eth_getLogs Initialize [{start}, {hi}]: {j.get('error')}"})
            break
        for lg in reversed(j["result"]):
            c0, c1 = "0x" + lg["topics"][2][26:], "0x" + lg["topics"][3][26:]
            d = lg["data"]
            fee, ts, hooks = word(d, 0), _s24(word(d, 1)), adr(d, 2)
            if int(c0, 16) == 0 or hooks != ZERO:
                continue
            key = ad(c0) + ad(c1) + w(fee) + w(ts) + ad(hooks)
            pid = "0x" + keccak256(bytes.fromhex(key)).hex()
            if pid != lg["topics"][1]:
                skipped.append({"kind": 19, "pool_id": lg["topics"][1], "why": "PoolId ≠ keccak256(ключа)"})
                continue
            found.append({"pid": pid, "c0": c0, "c1": c1, "key": "0x" + key})
        hi = start - 1
    out = []
    for p in found:
        r, e = node.call(a.v4_state_view, "0x" + sel("getLiquidity(bytes32)") + p["pid"][2:], tag)
        if r is None or len(r) < 66:
            skipped.append({"kind": 19, "pool_id": p["pid"], "why": f"StateView.getLiquidity: {e or r}"})
            continue
        if word(r, 0) == 0:
            continue
        out.append(dict(fam="v4", kind=19, pool_id=p["pid"], tokens=[p["c0"], p["c1"]], hooks=ZERO,
                        manager=a.v4_pool_manager, pool_manager=a.v4_pool_manager, data=p["key"],
                        step_pool=a.v4_pool_manager))
    return out


def bv3_pools(node, tag, a, skipped):
    """Пулы Balancer V3: из последней записи balancer_v3.py (meta.json pools) и --bv3-pool. Ошибок-исключений нет,
    кроме сетевых."""
    addrs = list(a.bv3_pool)
    recs = sorted(glob.glob(os.path.join(a.data_root, "arbitrum_balancer_v3_*", "meta.json")))
    if recs:
        with open(recs[-1], encoding="utf-8") as fh:
            addrs += [x for x in json.load(fh).get("pools", []) if x not in addrs]
    out = []
    for pool in addrs:
        pool = pool.lower()
        r, e = node.call(a.bv3_vault, "0x" + sel("getPoolTokenInfo(address)") + ad(pool), tag)
        if r is None or len(r) < 66 * 2:
            skipped.append({"kind": 23, "pool_id": pool, "why": f"Vault.getPoolTokenInfo: {e or r}"})
            continue
        off = word(r, 0) // 32
        n = word(r, off)
        tokens = [adr(r, off + 1 + i) for i in range(n)]
        h, e = node.call(a.bv3_vault, "0x" + sel("getHooksConfig(address)") + ad(pool), tag)
        hook = adr(h, 10) if h and len(h) >= 2 + 64 * 11 else None
        if hook is None:
            skipped.append({"kind": 23, "pool_id": pool, "why": f"Vault.getHooksConfig: {e or h}"})
            continue
        out.append(dict(fam="bv3", kind=23, pool_id=pool, tokens=tokens, hooks=ZERO, bv3_hook=hook,
                        manager=a.bv3_vault, pool_manager=a.bv3_vault, data="0x" + ad(pool),
                        step_pool=a.bv3_vault))
    return out


def quote_call(p, a, w_addr, tin, tout, amt):
    """Вызов квотера (для eth_call на H и для eth_simulateV1): {"to", "input"[, "from"]} и функция разбора ответа.
    Ошибок нет."""
    if p["fam"] == "v4":
        zf = tin == p["tokens"][0]
        data = "0x" + sel(V4_QSIG) + w(32) + p["data"][2:] + w(1 if zf else 0) + w(amt) + w(32 * 8) + w(0)
        return {"from": w_addr, "to": a.v4_quoter, "input": data}, lambda rd: word(rd, 0)
    data = ("0x" + sel(BV3_QSIG) + ad(p["pool_id"]) + ad(tin) + ad(tout) + w(amt) + ad(ZERO) + w(0xC0) + w(0))
    return {"to": a.bv3_router, "input": data}, lambda rd: word(rd, 0)


def credit(p, logs, tout):
    """Кредит события Swap менеджера в логах вызова: {amount0, amount1 | amount_in, out_side} или None. Ошибок нет."""
    for lg in logs:
        t = lg.get("topics", [])
        if p["fam"] == "v4":
            if lg["address"].lower() == p["pool_manager"].lower() and t and t[0] == V4_SWAP and t[1] == p["pool_id"]:
                d = lg["data"]
                i = 0 if tout == p["tokens"][0] else 1
                return dict(amount0=sword(d, 0), amount1=sword(d, 1), out_side=sword(d, i))
        elif (lg["address"].lower() == p["pool_manager"].lower() and t and t[0] == BV3_SWAP
              and "0x" + t[1][26:] == p["pool_id"]):
            d = lg["data"]
            return dict(amount_in=word(d, 0), out_side=word(d, 1), token_out="0x" + t[3][26:])
    return None
