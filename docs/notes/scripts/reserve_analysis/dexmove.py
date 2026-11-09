"""Файл: docs/notes/scripts/reserve_analysis/dexmove.py
Что делает: реальное время «решение → продажа» по исполненным сделкам и движение цены DEX для отказов биржа → DEX
(«что дала бы продажа») и для сделок биржа → DEX (проверка метода против факта продажи).
Кто вызывает: run.py.
От чего зависит: pools.py, node.py, common.py.

Время T_real: момент решения (plan.at — Calc.at, конец расчёта) → первая продажа на B завершена (sells[].finished_at).
Это ровно отрезок, за который цена продажи может уйти против нас, включая покупку, вывод, зачисление и перевод на
торговый счёт; transfer-секунды журнала (core.transfer_credited) этот отрезок не покрывают целиком
(docs/notes/transit-times-2026-11-09.md п.1: «зачислен» в хронологии — после to_trading). Берётся медиана и p75 по паре
площадок (A, B) среди чистых сделок того же типа; меньше min_n сделок — по площадке-источнику A (все B того же типа);
меньше и там — «не посчитано».
"""

import json

from common import median, quantile, log
from pools import Unsupported, ends, route_ratio


def t_real_table(deals, min_n):
    """Таблица T_real: {(тип, A, B): (мед, p75, n, 'пара')}, {(тип, A): (…, 'источник')}. Ошибок нет."""
    pair, src = {}, {}
    for d in deals:
        pair.setdefault((d["type"], d["A"], d["B"]), []).append(d["T_real"])
        src.setdefault((d["type"], d["A"]), []).append(d["T_real"])
    tp = {k: (median(v), quantile(v, 0.75), len(v), "пара") for k, v in pair.items() if len(v) >= min_n}
    ts = {k: (median(v), quantile(v, 0.75), len(v), "источник") for k, v in src.items() if len(v) >= min_n}
    return tp, ts


def lookup(tab, r):
    """T_real строки: (мед, p75, n, откуда) или None. Ошибок нет."""
    tp, ts = tab
    return tp.get((r["type"], r["A"], r["B"])) or ts.get((r["type"], r["A"]))


def _route_key(q, b0, targets):
    return json.dumps([q.get("route"), b0, targets], sort_keys=True, default=str)


def rows_cf(node, rows, tab, lookback):
    """Для строк биржа → DEX «только из-за запаса»: r_med/r_p75 (изменение цены продажи на DEX за T_real) и исход
    cf = прибыль до запаса + выручка × (r − 1). Ошибка строки — в cf_err (вид пула, нет T, узел). Ошибок нет."""
    memo = {}
    todo = [r for r in rows if r["type"] == "cex_dex"]
    for i, r in enumerate(todo, 1):
        if i % 50 == 0:
            log("отказы биржа→DEX: %d из %d; %s" % (i, len(todo), node.stats()))
        t = lookup(tab, r)
        if t is None:
            r["cf_err"] = "нет T_real: меньше min_n чистых сделок по паре и по источнику"
            continue
        r["T_real"], r["T_real_p75"] = t[0], t[1]
        sq = r["sell_quote"]
        b0 = (sq.get("route") or {}).get("block")
        if b0 is None:
            r["cf_err"] = "нет блока котировки"
            continue
        try:
            b1m, b1p = node.block_at(r["at"] + t[0]), node.block_at(r["at"] + t[1])
            if b1m is None or b1p is None:
                r["cf_err"] = "момент продажи ещё не наступил по голове узла"
                continue
            b1m, b1p = max(b1m, b0), max(b1p, b0)
            r["b0"], r["b1_med"], r["b1_p75"] = b0, b1m, b1p
            key = _route_key(sq, b0, [b1m, b1p])
            if key not in memo:
                try:
                    start, end = ends(sq, r["coin_addr"], "sell")
                    memo[key] = route_ratio(node, sq, start, end, b0, [b1m, b1p], lookback)
                except Unsupported as e:
                    memo[key] = str(e)
        except (RuntimeError, ValueError) as e:
            r["cf_err"] = "узел: %s" % str(e)[:160]
            continue
        if isinstance(memo[key], str):
            r["cf_err"] = memo[key]
            continue
        rr, cnt, kinds = memo[key]
        r["r_med"], r["r_p75"], r["events_med"] = rr[b1m], rr[b1p], cnt[b1m]
        r["pool_kinds"] = ";".join(kinds)
        r["cf_med"] = r["pb"] + r["proceeds"] * (rr[b1m] - 1)
        r["cf_p75"] = r["pb"] + r["proceeds"] * (rr[b1p] - 1)


def deals_model(node, deals, lookback):
    """Сделки биржа → DEX: модель пула от блока котировки плана до блока перед блоком нашей продажи (своё влияние
    на пул исключено целиком блоком) — model_r; сравнивается с фактом move. Ошибок нет (ошибка — model_err)."""
    todo = [d for d in deals if d["type"] == "cex_dex"]
    for i, d in enumerate(todo, 1):
        if i % 50 == 0:
            log("сделки биржа→DEX (проверка метода): %d из %d; %s" % (i, len(todo), node.stats()))
        sq = d["sell_quote"]
        b0 = (sq.get("route") or {}).get("block")
        if b0 is None or d.get("sell_block") is None:
            d["model_err"] = "нет блока котировки или блока продажи"
            continue
        b1 = max(b0, int(d["sell_block"]) - 1)
        d["b0"] = b0
        try:
            start, end = ends(sq, d["coin_addr"], "sell")
            rr, _cnt, _k = route_ratio(node, sq, start, end, b0, [b1], lookback)
            d["model_r"] = rr[b1]
        except Unsupported as e:
            d["model_err"] = str(e)
        except (RuntimeError, ValueError) as e:
            d["model_err"] = "узел: %s" % str(e)[:160]
