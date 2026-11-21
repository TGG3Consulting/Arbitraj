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

Защита от зависания (раздел 8 заметки): строки и сделки идут от старых к новым (у границы истории узла — первыми,
пока она не сдвинулась), перед каждой — строка в журнале прогресса; блок старше earliest и не в кэше прошлых
запусков — причина без запроса (node.py Pruned);
узел не ответил за срок — причина «узел не ответил за N с»; max_timeouts таких подряд — остальное пропускается
с причиной (узел недоступен), разбор не стоит.
"""

import json

from common import median, quantile, log, iso
from node import Pruned
from pools import Unsupported, ends, route_ratio
from rpc import NodeTimeout, RpcError


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


class Guard:
    """Счётчик подряд не ответивших запросов: после max_n — остальное пропускается с причиной. Ошибок нет."""

    def __init__(self, max_n):
        self.max_n, self.run, self.tripped = max_n, 0, None

    def err(self, e, node):
        """Исключение узла → текст причины (одинаковый для одинаковых причин — для счётчиков)."""
        if isinstance(e, Pruned):
            self.run = 0
            return str(e)
        if isinstance(e, NodeTimeout):
            self.run += 1
            if self.run >= self.max_n:
                self.tripped = "пропущено: узел %d раз подряд не ответил за срок" % self.run
                log("СТОП фазы узла: %s" % self.tripped)
            return str(e)
        self.run = 0
        if isinstance(e, RpcError):
            return "узел: " + str(e)[:200]
        return "узел: %s" % str(e)[:160]

    def ok(self):
        self.run = 0


def rows_cf(node, rows, tab, lookback, max_timeouts):
    """Для строк биржа → DEX «только из-за запаса»: r_med/r_p75 (изменение цены продажи на DEX за T_real) и исход
    cf = прибыль до запаса + выручка × (r − 1). Ошибка строки — в cf_err (вид пула, нет T, история узла, срок).
    Ошибок нет."""
    memo = {}
    g = Guard(max_timeouts)
    todo = sorted((r for r in rows if r["type"] == "cex_dex"), key=lambda r: (r["at"], r["id"]))
    for i, r in enumerate(todo, 1):
        if i % 50 == 0:
            log("отказы биржа→DEX: %d из %d; %s" % (i, len(todo), node.stats()["node"]))
        if g.tripped:
            r["cf_err"] = g.tripped
            continue
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
        log("строка %d/%d id %s %s %s→%s at %s блок котировки %s" % (i, len(todo), r["id"], r["coin"], r["A"], r["B"],
                                                                   iso(r["at"]), b0), quiet=True)
        r["b0"] = b0
        try:
            b1m, b1p = node.block_at(r["at"] + t[0]), node.block_at(r["at"] + t[1])
            if b1m is None or b1p is None:
                r["cf_err"] = "момент продажи ещё не наступил по голове узла"
                continue
            b1m, b1p = max(b1m, b0), max(b1p, b0)
            r["b1_med"], r["b1_p75"] = b1m, b1p
            key = _route_key(sq, b0, [b1m, b1p])
            if key not in memo:
                try:
                    start, end = ends(sq, r["coin_addr"], "sell")
                    memo[key] = route_ratio(node, sq, start, end, b0, [b1m, b1p], lookback)
                except Unsupported as e:
                    memo[key] = str(e)
            g.ok()
        except (Pruned, RuntimeError, ValueError) as e:
            r["cf_err"] = g.err(e, node)
            continue
        if isinstance(memo[key], str):
            r["cf_err"] = memo[key]
            continue
        rr, cnt, kinds = memo[key]
        r["r_med"], r["r_p75"], r["events_med"] = rr[b1m], rr[b1p], cnt[b1m]
        r["pool_kinds"] = ";".join(kinds)
        r["cf_med"] = r["pb"] + r["proceeds"] * (rr[b1m] - 1)
        r["cf_p75"] = r["pb"] + r["proceeds"] * (rr[b1p] - 1)


def deals_model(node, deals, lookback, max_timeouts):
    """Сделки биржа → DEX: модель пула от блока котировки плана до блока перед блоком нашей продажи (своё влияние
    на пул исключено целиком блоком) — model_r; сравнивается с фактом move. Ошибок нет (ошибка — model_err)."""
    g = Guard(max_timeouts)
    todo = sorted((d for d in deals if d["type"] == "cex_dex"), key=lambda d: (d["at"], str(d["id"])))
    for i, d in enumerate(todo, 1):
        if i % 50 == 0:
            log("сделки биржа→DEX (проверка метода): %d из %d; %s" % (i, len(todo), node.stats()["node"]))
        if g.tripped:
            d["model_err"] = g.tripped
            continue
        sq = d["sell_quote"]
        b0 = (sq.get("route") or {}).get("block")
        if b0 is None or d.get("sell_block") is None:
            d["model_err"] = "нет блока котировки или блока продажи"
            continue
        b1 = max(b0, int(d["sell_block"]) - 1)
        d["b0"] = b0
        log("сделка %d/%d id %s %s блоки %d–%d" % (i, len(todo), d["id"], d["coin"], b0, b1), quiet=True)
        try:
            start, end = ends(sq, d["coin_addr"], "sell")
            rr, _cnt, _k = route_ratio(node, sq, start, end, b0, [b1], lookback)
            d["model_r"] = rr[b1]
            g.ok()
        except Unsupported as e:
            d["model_err"] = str(e)
        except (Pruned, RuntimeError, ValueError) as e:
            d["model_err"] = g.err(e, node)
