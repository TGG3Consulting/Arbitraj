"""Файл: tools/arbitrum_record/v4live_old.py
Что делает: решение 459 (T457-02) — пулы шагов исполнителя v4 на Arbitrum, которые правило приёмки решения 400 НЕ
  судит (не шаги менеджера: live_proof.rs number → None), но которые есть у площадки arb_dex: FLUID = 22, LB20 = 24
  (новые шаги v4) и старые V2 = 0, V3 = 1. Их строки в summary.json — свидетельство «шаг v4 исполняется на живом
  пуле, получено = квотер в том же состоянии, на исполнителе 0»; кредит события у них не пишется (null).
  Fluid DEX T1 (решение 440; fluid_t1.py): пулы — getAllPoolAddresses резолвера, токены и слой ликвидности —
  constantsView пула (слова 9, 10 и 1: pools/abi_fluid.rs ConstantViews), пулы с нативной монетой 0xEeee… не
  берутся; квотер — estimateSwapIn резолвера; data = (swap0to1, native 0, liquidity).
  LB v2.0 (решение 445; lb_v20.py): пары — allLBPairs фабрики v2.0 (последние), tokenX / tokenY; квотер — getSwapOut
  LBRouter v2.0 (слово 0 — выход); data пусто.
  Uniswap V3 (dex-settings-arbitrum.md §2.2): пул — getPool фабрики по парам токенов уже выбранных пулов и уровням
  --v3-fee; квотер — QuoterV2 quoteExactInputSingle; data пусто.
  Uniswap V2 (§2.1): пара — getPair фабрики по тем же парам; квотер — формула пары по getReserves (комиссия
  --v2-fee-pips, v2-core UniswapV2Pair.sol L180-L182: 0,3 % = 3000); data = abi.encode(uint32 feePips).
Кто вызывает: tools/arbitrum_record/exec_v4_live.py.
От чего зависит: v4live_rpc.py.
Ошибки: сбой узла — исключение как есть; пул без ответа — пропуск с причиной (skipped).
"""

from v4live_rpc import ZERO, ad, adr, sel, w, word

NATIVE = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"


def _row(fam, kind, pool, tokens, data):
    return dict(fam=fam, kind=kind, pool_id=pool.lower(), tokens=[t.lower() for t in tokens], hooks=ZERO,
                manager=pool.lower(), pool_manager=None, data=data, step_pool=pool.lower())


def fluid_pools(node, tag, a, skipped):
    """Пулы Fluid DEX T1 без нативной монеты. Ошибок-исключений нет, кроме сетевых."""
    r, e = node.call(a.fluid_resolver, "0x" + sel("getAllPoolAddresses()"), tag)
    if r is None or len(r) < 130:
        skipped.append({"kind": 22, "why": f"getAllPoolAddresses: {e or r}"})
        return []
    off = word(r, 0) // 32
    pools = [adr(r, off + 1 + i) for i in range(word(r, off))]
    out = []
    for p in pools:
        c, e = node.call(p, "0x" + sel("constantsView()"), tag)
        if c is None or len(c) < 2 + 64 * 11:
            skipped.append({"kind": 22, "pool_id": p, "why": f"constantsView: {e or c}"})
            continue
        t0, t1, liq = adr(c, 9), adr(c, 10), adr(c, 1)
        if NATIVE in (t0, t1):
            continue
        row = _row("fluid", 22, p, [t0, t1], None)
        row["liquidity"] = liq
        out.append(row)
    return out


def lb20_pools(node, tag, a, skipped):
    """Последние --lb20-scan пар фабрики LB v2.0. Ошибок-исключений нет, кроме сетевых."""
    r, e = node.call(a.lb20_factory, "0x" + sel("getNumberOfLBPairs()"), tag)
    if r is None or len(r) < 66:
        skipped.append({"kind": 24, "why": f"getNumberOfLBPairs: {e or r}"})
        return []
    n, out = word(r, 0), []
    for i in range(n - 1, max(-1, n - 1 - a.lb20_scan), -1):
        pr, e = node.call(a.lb20_factory, "0x" + sel("allLBPairs(uint256)") + w(i), tag)
        x, _ = node.call(adr(pr, 0), "0x" + sel("tokenX()"), tag) if pr else (None, None)
        y, _ = node.call(adr(pr, 0), "0x" + sel("tokenY()"), tag) if pr else (None, None)
        if not (pr and x and y):
            skipped.append({"kind": 24, "index": i, "why": f"allLBPairs / tokenX / tokenY: {e}"})
            continue
        out.append(_row("lb20", 24, adr(pr, 0), [adr(x, 0), adr(y, 0)], "0x"))
    return out


def classic_pools(node, tag, a, pairs, skipped):
    """Пулы Uniswap V3 (getPool по уровням) и пары V2 (getPair) для пар токенов. Ошибок-исключений нет, кроме сетевых."""
    out = []
    for t0, t1 in pairs:
        for fee in a.v3_fee:
            r, _ = node.call(a.v3_factory, "0x" + sel("getPool(address,address,uint24)") + ad(t0) + ad(t1) + w(fee),
                             tag)
            if r and len(r) >= 66 and int(adr(r, 0), 16):
                row = _row("v3", 1, adr(r, 0), sorted([t0, t1]), "0x")
                row["fee"] = fee
                out.append(row)
        r, _ = node.call(a.v2_factory, "0x" + sel("getPair(address,address)") + ad(t0) + ad(t1), tag)
        if r and len(r) >= 66 and int(adr(r, 0), 16):
            out.append(_row("v2", 0, adr(r, 0), sorted([t0, t1]), "0x" + w(a.v2_fee_pips)))
    return out


def _v2_out(rd, p, tin, amt, fee_pips):
    r0, r1 = word(rd, 0), word(rd, 1)
    rin, rout = (r0, r1) if tin == p["tokens"][0] else (r1, r0)
    x = amt * (10**6 - fee_pips)
    return x * rout // (rin * 10**6 + x)


def quote_call(p, a, w_addr, tin, tout, amt):
    """Вызов квотера вида и разбор ответа. Ошибок нет."""
    if p["fam"] == "fluid":
        zf = tin == p["tokens"][0]
        data = "0x" + sel("estimateSwapIn(address,bool,uint256,uint256)") + ad(p["pool_id"]) + w(int(zf)) + w(amt) + w(0)
        return {"from": w_addr, "to": a.fluid_resolver, "input": data}, lambda rd: word(rd, 0)
    if p["fam"] == "lb20":
        data = "0x" + sel("getSwapOut(address,uint256,bool)") + ad(p["pool_id"]) + w(amt) + w(int(tin == p["tokens"][0]))
        return {"from": w_addr, "to": a.lb20_router, "input": data}, lambda rd: word(rd, 0)
    if p["fam"] == "v3":
        data = ("0x" + sel("quoteExactInputSingle((address,address,uint256,uint24,uint160))") + ad(tin) + ad(tout)
                + w(amt) + w(p["fee"]) + w(0))
        return {"from": w_addr, "to": a.v3_quoter, "input": data}, lambda rd: word(rd, 0)
    return ({"from": w_addr, "to": p["pool_id"], "input": "0x" + sel("getReserves()")},
            lambda rd: _v2_out(rd, p, tin, amt, a.v2_fee_pips))


def step_data(p, tin):
    """data шага (contracts/ArbExecutor.sol, комментарий Step). Ошибок нет."""
    if p["fam"] == "fluid":
        return "0x" + w(int(tin == p["tokens"][0])) + w(0) + ad(p["liquidity"])
    return p["data"]
