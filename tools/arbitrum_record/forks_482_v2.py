"""Файл: tools/arbitrum_record/forks_482_v2.py
Что делает: решение 482 — часть записи forks_482.py для пар V2-форков Arbitrum (Joe DEX / LFJ V1, Rubicon Aquila,
  Swapsicle V1, Arbitrum Exchange V2): выбор пар фабрики, запросы состояния пары, getAmountsOut роутера и симуляция
  обмена прямо в паре (out — формула шага V2 исполнителя, и out + 1 — пара должна откатить проверку K).
Кто вызывает: tools/arbitrum_record/forks_482.py.
От чего зависит: curve_plain_pools.py (call), curve_sim.py (SENDER), v4live_rpc.py (sel, w, ad, word).
Ошибки: исключений нет, кроме сетевых и StaleState (stdhttp.py).

Подписи (Sourcify 42161, 20.11.2026, exact_match / match): JoeFactory, SicleFactory, ArbDexFactory — allPairsLength(),
  allPairs(uint256), getPair(address,address); пары JoePair / SiclePair / AquilaPair / ArbDexPair — getReserves(),
  token0(), token1(), factory(), swap(uint256,uint256,address,bytes) (у Joe uint256, у Aquila uint — тот же ABI);
  ArbDexPair — swapFee() (= 25, сеттера нет); роутеры JoeRouter02 / AquilaRouter — getAmountsOut(uint256,address[]).
"""

from curve_plain_pools import call
from curve_sim import SENDER
from v4live_rpc import ad, sel, w, word

WETH = "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1"  # dex.native_wrapped Arbitrum (dex-settings-arbitrum.md §0)


def pick_pairs(b, dex, scan, keep):
    """Пары фабрики V2 на «latest»: до scan последних allPairs, из них keep с наибольшим резервом WETH (иначе первые с
    обоими резервами > 0). Возвращает: [(пара, token0, token1)]."""
    n = int(b.one("eth_call", [{"to": dex["factory"], "data": "0x" + sel("allPairsLength()")}, "latest"])["result"], 16)
    idx = list(range(max(0, n - scan), n))
    got = b.run([call(dex["factory"], "0x" + sel("allPairs(uint256)") + w(i), "latest") for i in idx])
    pairs = ["0x" + g["result"][-40:] for g in got if g.get("result")]
    reqs = []
    for p in pairs:
        reqs += [call(p, "0x" + sel(s), "latest") for s in ("token0()", "token1()", "getReserves()")]
    ans = b.run(reqs)
    cands = []
    for k, p in enumerate(pairs):
        t0, t1, r = (ans[3 * k + j].get("result") for j in range(3))
        if not (t0 and t1 and r):
            continue
        t0, t1, r0, r1 = "0x" + t0[-40:], "0x" + t1[-40:], word(r, 0), word(r, 1)
        if r0 and r1:
            weth = r0 if t0.lower() == WETH.lower() else r1 if t1.lower() == WETH.lower() else 0
            cands.append((weth, p, t0, t1))
    cands.sort(key=lambda c: -c[0])
    return [(p, t0, t1) for _, p, t0, t1 in cands[:keep]]


def v2_state_reqs(dex, pair, t0, t1, tag):
    """Пакет B1 пары: getReserves, factory, token0, token1, getPair фабрики и метод комиссии (если задан)."""
    reqs = [call(pair, "0x" + sel(s), tag) for s in ("getReserves()", "factory()", "token0()", "token1()")]
    reqs.append(call(dex["factory"], "0x" + sel("getPair(address,address)") + ad(t0) + ad(t1), tag))
    if dex.get("fee_method"):
        reqs.append(call(pair, "0x" + sel(dex["fee_method"]), tag))
    return reqs


def v2_out(dx, r_in, r_out, fee):
    """Выход шага V2 исполнителя (ArbExecutor.sol _v2Out): вход·(1e6 − fee)·rOut / (rIn·1e6 + вход·(1e6 − fee))."""
    f = dx * (1_000_000 - fee)
    return f * r_out // (r_in * 1_000_000 + f)


def v2_quote_reqs(dex, rec, slot_of, fractions, tag):
    """Пакет B2 пары: getAmountsOut роутера и симуляции обмена (out и out + 1) в обе стороны. Принимает: DEX, запись
    пары (state B1), слоты баланса {(токен, пара): слот}, доли резерва входа, блок. Возвращает: (запросы, метки)."""
    reqs, marks = [], []
    st = rec["state"]
    r0, r1 = word(st["getReserves()"]["result"], 0), word(st["getReserves()"]["result"], 1)
    for zfo in (True, False):
        tin, tout = (rec["token0"], rec["token1"]) if zfo else (rec["token1"], rec["token0"])
        r_in, r_out = (r0, r1) if zfo else (r1, r0)
        slot = slot_of.get((tin.lower(), rec["pair"].lower()))
        slot_out = slot_of.get((tout.lower(), rec["pair"].lower()))
        for f in fractions:
            dx = max(1, int(r_in * f))
            out = v2_out(dx, r_in, r_out, dex["fee_pips"])
            if dex.get("router"):
                d = "0x" + sel("getAmountsOut(uint256,address[])") + w(dx) + w(64) + w(2) + ad(tin) + ad(tout)
                reqs.append(call(dex["router"], d, tag))
                marks.append(("router", zfo, str(dx), str(out)))
            if slot is None or slot_out is None:
                continue
            # Баланс входа = резерв + dx (вход ровно dx), выхода = резерв (излишек пары не добавляет входа стороне выхода).
            ov = {tin: {"stateDiff": {slot: "0x" + w(r_in + dx)}}, tout: {"stateDiff": {slot_out: "0x" + w(r_out)}}}
            for extra in (0, 1):
                o = out + extra
                d = "0x" + sel("swap(uint256,uint256,address,bytes)") + w(0 if zfo else o) + w(o if zfo else 0) \
                    + ad(SENDER) + w(128) + w(0)
                reqs.append(("eth_call", [{"from": SENDER, "to": rec["pair"], "data": d}, tag, ov]))
                marks.append(("sim", zfo, str(dx), str(o)))
    return reqs, marks
