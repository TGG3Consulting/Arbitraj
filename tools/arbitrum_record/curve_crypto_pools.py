"""Чтение пулов для записи crypto-пулов Curve (решения 442, 453; вынесено из curve_crypto.py — ревью 453): вызовы бота
по calldata (pools/load_crypto.rs и load_crypto_v1.rs calls() — тот же порядок и данные), лесенка get_dy, eth_getCode,
holdings (balanceOf монет и ETH пула), decimals монет v1, пулы фабрик NG (по MATH()) и v1.

Кто вызывает: tools/arbitrum_record/curve_crypto.py.
"""

import sys

from curve_crypto_rpc import addr_word, as_addr, as_int, selector, word
from curve_sim import Sim

# Решение 472 (T472-02): симуляция обмена — exchange(uint256,uint256,uint256,uint256) у NG и v1 (abi_crypto.rs,
# abi_crypto_v1.rs; шаг STABLE_U256 исполнителя).
EXCHANGE_U = "exchange(uint256,uint256,uint256,uint256)"

# --- запись -------------------------------------------------------------------------------------------------------------

# Доли баланса монеты входа для лесенки get_dy (числитель над 10^6).
LADDER_PPM = [1, 100, 1000, 10000, 100000, 500000]

# Вызовы бота: pools/load_crypto.rs calls() (NG) и pools/load_crypto_v1.rs calls() (v1) — тот же порядок и те же данные.
TWEAK_TAIL = ["totalSupply()", "xcp_profit()", "virtual_price()", "packed_rebalancing_params()"]
NG_HEAD = ["A()", "gamma()", "D()", "mid_fee()", "out_fee()", "fee_gamma()", "future_A_gamma_time()"]


def bot_calls(n, v1):
    """Сигнатуры и аргументы вызовов бота (calldata). n — число монет, v1 — пул первого поколения."""
    out = [(sig, "") for sig in NG_HEAD]
    if not v1:
        out.append(("precisions()", ""))
    for k in range(n):
        out.append(("coins(uint256)", word(k)))
        out.append(("balances(uint256)", word(k)))
    if n == 2:
        out.append(("price_scale()", ""))
    else:
        for k in range(n - 1):
            out.append(("price_scale(uint256)", word(k)))
    if v1:
        if n == 3:
            out.append(("is_killed()", ""))
    else:
        out.append(("MATH()", ""))
        # Решение 472: состояние tweak_price NG — pools/load_crypto_tweak.rs calls(), тот же порядок.
        if n == 2:
            out.append(("price_oracle()", ""))
        else:
            out += [("price_oracle(uint256)", word(k)) for k in range(n - 1)]
        out += [(sig, "") for sig in TWEAK_TAIL]
    return out


def record_calls(node, pool, n, v1, tag):
    """Ответы на вызовы бота и проверочные — словарь calldata → ответ."""
    calls = {}
    sigs = bot_calls(n, v1) + ([] if v1 else [("version()", "")])
    for sig, args in sigs:
        data = selector(sig) + args
        calls[data] = node.call(pool, data, tag)
    return calls


def answer(calls, sig, args=""):
    return calls.get(selector(sig) + args, {})


def ladder(node, pool, n, calls, tag):
    rows = []
    for i in range(n):
        bal = as_int(answer(calls, "balances(uint256)", word(i))) or 0
        for j in range(n):
            if i == j:
                continue
            for ppm in LADDER_PPM:
                dx = bal * ppm // 1_000_000
                if dx == 0:
                    continue
                data = selector("get_dy(uint256,uint256,uint256)") + word(i) + word(j) + word(dx)
                rows.append({"i": i, "j": j, "dx": str(dx), "get_dy": node.call(pool, data, tag)})
    return rows


def code(node, addr, tag):
    """eth_getCode: hex кода (или ошибка узла)."""
    j = node.rpc("eth_getCode", [addr, tag])
    return {"code": j["result"]} if "result" in j else {"error": j.get("error")}


def holdings(node, pool, calls, n, tag):
    """Что пул держит на самом деле: balanceOf(pool) у каждой монеты и eth_getBalance(pool) — для вопроса 5 решения 442
    (Tricrypto-NG с нативным ETH держит ETH, а не WETH; источник встречного токена пробы берётся по balanceOf)."""
    out = {"eth_balance": node.rpc("eth_getBalance", [pool, tag]).get("result")}
    for k in range(n):
        c = as_addr(answer(calls, "coins(uint256)", word(k)))
        if c:
            out[c] = node.call(c, selector("balanceOf(address)") + addr_word(pool), tag)
    return out


def coin_decimals(node, calls, n, tag):
    out = []
    for k in range(n):
        c = as_addr(answer(calls, "coins(uint256)", word(k)))
        out.append(as_int(node.call(c, selector("decimals()"), tag)) if c else None)
    return out


def factory_pools(node, factory, tag):
    count = as_int(node.call(factory, selector("pool_count()"), tag)) or 0
    return count, lambda k: as_addr(node.call(factory, selector("pool_list(uint256)") + word(k), tag))


def ng_rows(node, factory, n, maths, per_math, tag):
    """Пулы фабрики NG с конца pool_list: до per_math пулов каждой математики из maths (MATH() пула)."""
    count, at = factory_pools(node, factory, tag)
    taken = {m.lower(): 0 for m in maths}
    rows = []
    for k in reversed(range(count)):
        if all(v >= per_math for v in taken.values()):
            break
        p = at(k)
        if not p:
            continue
        calls = record_calls(node, p, n, False, tag)
        m = as_addr(answer(calls, "MATH()"))
        if m is None or m.lower() not in taken or taken[m.lower()] >= per_math:
            continue
        taken[m.lower()] += 1
        rows.append({
            "pool": p,
            "index": k,
            "calls": calls,
            "factory_get_coins": node.call(factory, selector("get_coins(address)") + addr_word(p), tag),
            "code": code(node, p, tag),
            "holdings": holdings(node, p, calls, n, tag),
            "get_dy": ladder(node, p, n, calls, tag),
        })
        rows[-1]["exchange_sim"] = sims(node, p, n, calls, rows[-1]["get_dy"], tag)
        print(f"NG({n}) {p}: MATH {m}, {len(rows[-1]['get_dy'])} ответов get_dy", file=sys.stderr)
    return count, rows


def v1_row(node, pool, n, tag, factory=None):
    calls = record_calls(node, pool, n, True, tag)
    row = {
        "pool": pool,
        "calls": calls,
        "decimals": coin_decimals(node, calls, n, tag),
        "code": code(node, pool, tag),
        "holdings": holdings(node, pool, calls, n, tag),
        "get_dy": ladder(node, pool, n, calls, tag),
    }
    row["exchange_sim"] = sims(node, pool, n, calls, row["get_dy"], tag)
    if factory:
        row["factory_get_coins"] = node.call(factory, selector("get_coins(address)") + addr_word(pool), tag)
        row["factory_get_decimals"] = node.call(factory, selector("get_decimals(address)") + addr_word(pool), tag)
    print(f"v1({n}) {pool}: {len(row['get_dy'])} ответов get_dy", file=sys.stderr)
    return row


def sims(node, pool, n, calls, rows, tag):
    """Решение 472 (T472-02): exchange на суммах лесенки — eth_call с подменой баланса и разрешения у пустого
    отправителя (curve_sim.py), одним пакетом на том же блоке. Возвращает: записи симуляций."""
    sim = Sim(node.url, tag)
    coins = [as_addr(answer(calls, "coins(uint256)", word(k))) for k in range(n)]
    for c in coins:
        if c:
            sim.prefill(c, pool)
    for r in rows:
        c = coins[int(r["i"])]
        if c:
            sim.add(pool, EXCHANGE_U, int(r["i"]), int(r["j"]), int(r["dx"]), c)
    return sim.run()
