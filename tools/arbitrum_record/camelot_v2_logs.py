"""Файл: tools/arbitrum_record/camelot_v2_logs.py
Что делает: решение 460 — поиск редких событий Camelot V2 для camelot_v2.py (до чтения состояния блока B):
  1. lower_bound — нижняя граница поиска: --from-block или блок события PairCreated первой пары фабрики (allPairs(0);
     createPair кладёт пару в allPairs и шлёт PairCreated — CamelotFactory.sol L54-L70, verified-исходник Arbiscan
     фабрики 0x6EcCab42…A652, см. src/venues/bsc_dex/pools/abi_camelot.rs). Надёжного официального источника блока
     развёртывания фабрики нет (документация Camelot его не даёт) — граница берётся из лога, найденного той же
     записью: событий пар этой фабрики раньше создания её первой пары быть не может.
  2. rare_events — FeePercentUpdated / SetStableSwap пар этой фабрики (топик-фильтр без address: пары заранее не
     известны; своя ли пара — factory() пары на «latest», поле пары задаётся при создании) и PairCreated фабрики —
     назад от блока b0 частями по --chunk, при таймауте узла — половинами (recnode.logs_split), пока не наберётся
     --rare штук каждого или не пройдена нижняя граница. Не найдено до границы — факт «нет событий с блока N» (пишет
     camelot_v2.py), не ошибка.
  Прогон 20.11.2026: eth_getLogs {'code': -32002, 'message': 'request timed out'} на окне 2 000 000 блоков
  (поиск с головы 525985036 назад, 18 окон прошли без событий fee/stable).
Кто вызывает: tools/arbitrum_record/camelot_v2.py.
От чего зависит: recnode.py (logs_split); узел — объект Node из camelot_v2.py (one, calls).
Ошибки: ошибки узла (кроме таймаута, который делит окно) — исключения как есть.
"""

import recnode


def _word(h, i=0):
    h = h[2:] if h.startswith("0x") else h
    return int(h[64 * i:64 * (i + 1)] or "0", 16)


def _addr(h, i=0):
    return "0x" + format(_word(h, i), "040x")


def _full(h):
    return h is not None and len(h[2:] if h.startswith("0x") else h) >= 64


def _logs(node, flt, lo, hi, a, say):
    """eth_getLogs окна [lo, hi] с делением по таймауту. Возвращает список по возрастанию блоков."""
    return recnode.logs_split(lambda x, y: node.one("eth_getLogs", [dict(flt, fromBlock=hex(x), toBlock=hex(y))]),
                              lo, hi, a.min_chunk, say)


def lower_bound(node, fac, b0, a, S, T, say):
    """Нижняя граница поиска. Возвращает: (блок, источник — текст). Ошибки: ошибки узла."""
    if a.from_block is not None:
        return a.from_block, "задана --from-block"
    p0 = node.calls([(fac, S["allPairs(uint256)"] + "0" * 64)], "latest")[0]
    if not _full(p0):
        return 0, "allPairs(0) фабрики не ответил — граница 0"
    pair0 = _addr(p0)
    tk = node.calls([(pair0, S["token0()"]), (pair0, S["token1()"])], "latest")
    if not all(_full(x) for x in tk):
        return 0, f"token0/token1 первой пары {pair0} не ответили — граница 0"
    t0, t1 = ("0x" + format(_word(x), "064x") for x in tk)
    say(f"нижняя граница: PairCreated первой пары {pair0} (блоки 0..{b0})")
    got = _logs(node, {"address": fac, "topics": [T["created"], t0, t1]}, 0, b0, a, say)
    for g in got:
        if _addr(g["data"], 0).lower() == pair0.lower():
            n = int(g["blockNumber"], 16)
            return n, f"блок PairCreated первой пары фабрики allPairs(0) = {pair0} (транзакция {g['transactionHash']})"
    return 0, f"PairCreated первой пары {pair0} не найден в блоках 0..{b0} — граница 0"


def rare_events(node, fac, b0, bound, a, S, T, say):
    """Редкие события назад от b0 до bound. Возвращает: (словарь fee/stable/created — новые первыми, нижний
    пройденный блок). Ошибки: ошибки узла."""
    rare = {"fee": [], "stable": [], "created": []}
    hi = b0
    while hi >= bound and any(len(v) < a.rare for v in rare.values()):
        lo2 = max(bound, hi - a.chunk + 1)
        if len(rare["fee"]) < a.rare or len(rare["stable"]) < a.rare:
            got = _logs(node, {"topics": [[T["fee"], T["stable"]]]}, lo2, hi, a, say)
            own = sorted({g["address"] for g in got})
            facs = node.calls([(x, S["factory()"]) for x in own], "latest") if own else []
            ours = {x for x, f in zip(own, facs) if _full(f) and _addr(f).lower() == fac.lower()}
            for g in reversed(got):
                k = "fee" if g["topics"][0] == T["fee"] else "stable"
                if g["address"] in ours and len(rare[k]) < a.rare:
                    rare[k].append(g)
        if len(rare["created"]) < a.rare:
            cr = _logs(node, {"address": fac, "topics": [T["created"]]}, lo2, hi, a, say)
            rare["created"] += list(reversed(cr))[: a.rare - len(rare["created"])]
        say(f"поиск событий {lo2}..{hi}: fee {len(rare['fee'])}, stable {len(rare['stable'])}, "
            f"created {len(rare['created'])}")
        hi = lo2 - 1
    return rare, hi + 1
