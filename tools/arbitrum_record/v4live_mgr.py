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
  команды (meta.json pools), --bv3-pool и (T477-01, решение 478) getPools() фабрик списков meta.json lists;
  токены — Vault.getPoolTokenInfo; квотер — Router
  querySwapSingleTokenExactIn без from (Vault.quote: tx.origin = 0); кредит — Swap(pool, tokenIn, tokenOut, amountIn,
  amountOut, …) хранилища. В строке записи hooks = 0: у шага BALANCER_V3 класс только «без крючка» (live_class.rs);
  крючок пула — поле bv3_hook. Координатор (T457-02): пулы с крючком, которые бот берёт (вид крючка — как
  pools/balancer_v3_hooks.rs kind: static — крючок без динамической комиссии, surge — StableSurge из
  dex.bv3_stable_surge_hooks у «открытого» выпуска, mev — MEV Capture из dex.bv3_mev_capture_hooks; списки — meta.json
  записи balancer_v3.py), тоже входят в запись — поле bv3_hook_kind; пулы, которые бот не берёт, — skipped.
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


def v4_inits(node, head, a, skipped):
    """События Initialize PoolManager за --v4-blocks до head (решение 460: только логи, до выбора блока H — узел хранит
    только недавнее состояние): пулы без крючка, обе валюты ERC-20, PoolId = keccak256(ключа); новые — первыми.
    Ошибок-исключений нет, кроме сетевых."""
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
    return found


def v4_pools(node, tag, found, a, skipped):
    """Ликвидность (StateView.getLiquidity на блоке H) найденных пулов: остаются пулы с ликвидностью > 0. Ошибок-
    исключений нет, кроме сетевых и StaleState."""
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


# Порядок проверки фабрик и «открытые» выпуски — pools/balancer_v3_factories.rs LISTS и Release::open.
BV3_LISTS = ["dex.bv3_weighted_factories", "dex.bv3_weighted_min_factories", "dex.bv3_stable_factories",
             "dex.bv3_stable_open_factories", "dex.bv3_stable_guard_factories", "dex.bv3_gyro_2clp_factories",
             "dex.bv3_gyro_eclp_factories", "dex.bv3_reclamm_factories", "dex.bv3_reclamm_v3_factories",
             "dex.bv3_lbp_v1_factories", "dex.bv3_lbp_v2_factories", "dex.bv3_lbp_v3_factories",
             "dex.bv3_lbp_v4_factories", "dex.bv3_fixed_lbp_factories", "dex.bv3_quantamm_factories"]
BV3_OPEN = {"dex.bv3_weighted_min_factories", "dex.bv3_stable_open_factories", "dex.bv3_stable_guard_factories",
            "dex.bv3_lbp_v3_factories", "dex.bv3_lbp_v4_factories", "dex.bv3_fixed_lbp_factories"}
# Вид крючка пула в записи (поле bv3_hook_kind): как pools/balancer_v3_hooks.rs kind.
BV3_HOOK_KINDS = ["none", "static", "surge", "mev"]


def bv3_hook_kind(node, tag, pool, h, lists):
    """Вид крючка так, как его берёт бот (balancer_v3_hooks.rs kind; HooksConfig — VaultTypes.sol L73-L85: слово 3
    computeDynamicSwapFee, 4 / 5 before / after swap, 10 адрес). Возвращает: (вид, None) или (None, почему бот пул не
    берёт). Ошибок-исключений нет, кроме сетевых."""
    hook = adr(h, 10)
    # Ревью t233, п. 6: как balancer_v3_factories.rs release — опрашиваются ВСЕ фабрики; подтвердили фабрики разных
    # выпусков (списков) — пул не берётся.
    confirmed = []
    for name in BV3_LISTS:
        for f in lists.get(name, []):
            r, _ = node.call(f, "0x" + sel("isPoolFromFactory(address)") + ad(pool), tag)
            if r and len(r) >= 66 and word(r, 0) == 1 and name not in confirmed:
                confirmed.append(name)
    if not confirmed:
        return None, "пул не из фабрик списков dex.bv3_*_factories"
    if len(confirmed) > 1:
        return None, f"пул подтвердили фабрики разных выпусков ({', '.join(confirmed)}) — бот пул не берёт"
    release = confirmed[0]
    if word(h, 4) or word(h, 5):
        return None, f"крючок {hook} до / после обмена — бот пул не берёт"
    if not word(h, 3):
        return ("none" if hook == ZERO else "static"), None
    if hook in lists.get("dex.bv3_stable_surge_hooks", []):
        if release not in BV3_OPEN:
            return None, f"StableSurge {hook} у выпуска {release} (onSwap только хранилищу) — бот пул не берёт"
        return "surge", None
    if hook in lists.get("dex.bv3_mev_capture_hooks", []):
        return "mev", None
    return None, f"крючок динамической комиссии {hook} не из списков — бот пул не берёт"


def _factory_pools(node, tag, lists, skipped):
    """T477-01: второй источник пулов Balancer V3 — перечисление фабрик списков dex.bv3_*_factories: getPools()
    (balancer-v3-monorepo@2b9ce4b1 pkg/pool-utils/contracts/BasePoolFactory.sol L73-L75 getPools — массив _pools, L127
    push при создании пула; так же tools/arbitrum_record/balancer_v3_buffer.py в ветке t244/arb-bv3-buffer).
    Порядок — с конца списка каждой фабрики (новые пулы первыми). Возвращает: [(пул, фабрика)]; откат getPools — в skipped. Ошибок-исключений нет, кроме сетевых."""
    out, seen = [], set()
    for name in BV3_LISTS:
        for f in lists.get(name, []):
            r, e = node.call(f, "0x" + sel("getPools()"), tag)
            if r is None or len(r) < 2 + 64 * 2:
                skipped.append({"kind": 23, "factory": f, "why": f"getPools() фабрики ({name}): {e or r}"})
                continue
            off = word(r, 0) // 32
            n = word(r, off)
            for i in reversed(range(n)):
                pool = adr(r, off + 1 + i)
                if pool not in seen:
                    seen.add(pool)
                    out.append((pool, f))
    return out


def bv3_pools(node, tag, a, skipped):
    """Пулы Balancer V3: из последней записи balancer_v3.py (meta.json pools, lists), --bv3-pool и (T477-01)
    перечислением getPools() фабрик списков; вид крючка — как у бота (bv3_hook_kind), пулы, которые бот не берёт, — в skipped; пул с
    нулевым балансом токена (getPoolTokenInfo balancesRaw) — в skipped. Кандидатов (пулов, которые бот берёт) — не
    больше --bv3-max-pools (чтение идёт на блоке H — узел хранит только недавнее состояние, решение 460). Поле src —
    откуда пул. Ошибок-исключений нет, кроме сетевых."""
    src = {x.lower(): "--bv3-pool" for x in a.bv3_pool}
    addrs, lists = [x.lower() for x in a.bv3_pool], {}
    recs = sorted(glob.glob(os.path.join(a.data_root, "arbitrum_balancer_v3_*", "meta.json")))
    if recs:
        with open(recs[-1], encoding="utf-8") as fh:
            meta = json.load(fh)
        for x in meta.get("pools", []):
            if x.lower() not in addrs:
                addrs.append(x.lower())
                src[x.lower()] = "запись balancer_v3.py " + os.path.basename(os.path.dirname(recs[-1]))
        lists = {k: [x.lower() for x in v] for k, v in meta.get("lists", {}).items()}
    if not lists:
        skipped.append({"kind": 23, "why": "нет записи balancer_v3.py со списками фабрик и крючков (meta.json lists)"})
        return []
    for pool, f in _factory_pools(node, tag, lists, skipped):
        if pool not in addrs:
            addrs.append(pool)
            src[pool] = f"getPools() фабрики {f}"
    out = []
    for pool in addrs:
        if len(out) >= a.bv3_max_pools:
            break
        r, e = node.call(a.bv3_vault, "0x" + sel("getPoolTokenInfo(address)") + ad(pool), tag)
        if r is None or len(r) < 66 * 2:
            skipped.append({"kind": 23, "pool_id": pool, "why": f"Vault.getPoolTokenInfo: {e or r}"})
            continue
        off = word(r, 0) // 32
        n = word(r, off)
        tokens = [adr(r, off + 1 + i) for i in range(n)]
        boff = word(r, 2) // 32
        if any(word(r, boff + 1 + i) == 0 for i in range(n)):
            skipped.append({"kind": 23, "pool_id": pool, "why": "баланс токена пула 0 (getPoolTokenInfo balancesRaw)"})
            continue
        h, e = node.call(a.bv3_vault, "0x" + sel("getHooksConfig(address)") + ad(pool), tag)
        if not h or len(h) < 2 + 64 * 11:
            skipped.append({"kind": 23, "pool_id": pool, "why": f"Vault.getHooksConfig: {e or h}"})
            continue
        hk, why = bv3_hook_kind(node, tag, pool, h, lists)
        if hk is None:
            skipped.append({"kind": 23, "pool_id": pool, "why": why})
            continue
        out.append(dict(fam="bv3", kind=23, pool_id=pool, tokens=tokens, hooks=ZERO, bv3_hook=adr(h, 10),
                        bv3_hook_kind=hk, manager=a.bv3_vault, pool_manager=a.bv3_vault, data="0x" + ad(pool),
                        step_pool=a.bv3_vault, src=src.get(pool)))
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
