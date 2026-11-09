"""Файл: docs/notes/scripts/reserve_analysis/records.py
Что делает: разбор данных бота в строки разбора: расчёт кандидата (Calc), отказы core.candidate_rejected, эпизоды
(монета + направление, разрыв > gap), чистые сделки с фактом продажи и временем «решение → продажа».
Кто вызывает: run.py.
От чего зависит: common.py.

Факты кода бота:
- данные отказа = serde Calc лучшего расчёта (core/candidates.rs:170, profit.rs Verdict::Reject best) — поля
  core/costs_types.rs Calc: direction, qty, sell_qty, buy_avg, sell_avg, invested, proceeds, costs[CostLine{kind, usdt…}],
  profit_usd, profit_pct (= profit / invested × 100), transit_s, sigma, buy_quote, sell_quote, at (Utc::now в конце расчёта);
- запас — строка CostLine kind = PriceMoveReserve, stage «ядро», в USDT (core/costs.rs:146-163);
- порог: отказ «прибыль X USDT ниже порога» (money.min_profit_usd) или «прибыль X% ниже порога» (money.min_profit_pct,
  в процентах) — core/profit.rs:151-171; прошёл — profit_usd ≥ min_usd И profit_pct ≥ min_pct;
- строка журнала с повторами хранит текст и данные ПОСЛЕДНЕГО повтора (core/reject_log.rs, решение 155): одна строка —
  один набор чисел за период [at, last_at];
- сделка — core/deal.rs Deal: plan (Calc на момент решения), step, buy, sells[TradeResult{request.venue, filled_qty,
  avg_price, chain.block, finished_at}], relays, timeline [[время, текст]], result_usd, return_cost_usd (решение 236).
"""

import re
from decimal import Decimal

from common import D, ts

RE_THRESHOLD = re.compile(r"отказ — прибыль -?[0-9.]+(?: USDT|%) ниже порога")
MANUAL_MARKS = ("вручную", "Беру на себя", "алерт", "Алерт")


def calc(data):
    """Calc из JSON бота → плоский словарь с Decimal. Ошибки: KeyError/ValueError — не тот формат."""
    d = data["direction"]
    a = d["asset"]
    sq, bq = data.get("sell_quote") or {}, data.get("buy_quote") or {}
    sell_dex, buy_dex = bool(sq.get("route")), bool(bq.get("route"))
    typ = "dex_dex" if (sell_dex and buy_dex) else "cex_dex" if sell_dex else "dex_cex" if buy_dex else "cex_cex"
    res = [c for c in data.get("costs") or [] if c.get("kind") == "PriceMoveReserve"]
    reserve = sum((D(c["usdt"]) for c in res if c.get("usdt") is not None), Decimal(0)) if res else None
    addr = (a.get("bsc_contract") or "").lower() or None
    return {
        "coin": a.get("ticker"), "coin_addr": addr, "coin_key": addr or a.get("ticker"),
        "A": d["buy_on"], "B": d["sell_on"], "type": typ,
        "at": ts(data["at"]), "qty": D(data["qty"]), "sell_qty": D(data.get("sell_qty")),
        "buy_avg": D(data["buy_avg"]), "sell_avg": D(data["sell_avg"]),
        "invested": D(data["invested"]), "proceeds": D(data["proceeds"]),
        "profit": D(data["profit_usd"]), "profit_pct": D(data["profit_pct"]),
        "sigma": D(data["sigma"]), "T": D(data["transit_s"]), "R": reserve,
        "buy_quote": bq, "sell_quote": sq,
    }


def passes(profit, invested, th):
    """Пороги core/profit.rs: profit ≥ min_usd и profit / invested × 100 ≥ min_pct (invested = 0 → pct = 0).
    Ошибок нет."""
    pct = profit / invested * 100 if invested else Decimal(0)
    return profit >= th["min_usd"] and pct >= th["min_pct"]


def reject_rows(entries, th):
    """Записи журнала core.candidate_rejected → строки с расчётом. Возвращает: (строки, счётчики).
    Строка: calc + id, j_at, j_last, repeats, threshold (отказ по порогу), pb (прибыль до запаса), reserve_only."""
    rows, cnt = [], {"entries": len(entries), "no_data": 0, "bad_format": 0, "no_reserve_line": 0}
    for e in entries:
        data = e.get("data")
        if not data:
            cnt["no_data"] += 1
            continue
        try:
            c = calc(data)
        except (KeyError, ValueError, TypeError, ArithmeticError):
            cnt["bad_format"] += 1
            continue
        c["id"] = e["id"]
        c["j_at"] = ts(e["at"])
        c["j_last"] = ts(e.get("last_at") or e["at"])
        c["repeats"] = e.get("repeats") or 1
        c["threshold"] = bool(RE_THRESHOLD.search(e.get("message") or ""))
        if c["R"] is None:
            cnt["no_reserve_line"] += 1
            c["pb"], c["reserve_only"] = None, False
        else:
            c["pb"] = c["profit"] + c["R"]
            c["reserve_only"] = (c["threshold"] and not passes(c["profit"], c["invested"], th)
                                 and passes(c["pb"], c["invested"], th))
        rows.append(c)
    return rows, cnt


def episodes(rows, gap_s):
    """Склейка строк в эпизоды: монета + направление, следующая строка позже конца предыдущей больше чем на gap_s.
    Возвращает: список эпизодов {key, rows (по времени), start, end}. Ошибок нет."""
    by = {}
    for r in rows:
        by.setdefault((r["coin_key"], r["A"], r["B"]), []).append(r)
    out = []
    for key, rs in by.items():
        rs.sort(key=lambda r: (r["j_at"], r["id"]))
        cur = None
        for r in rs:
            if cur is None or r["j_at"] - cur["end"] > gap_s:
                cur = {"key": key, "rows": [], "start": r["j_at"], "end": r["j_last"]}
                out.append(cur)
            cur["rows"].append(r)
            cur["end"] = max(cur["end"], r["j_last"])
    out.sort(key=lambda e: e["start"])
    return out


def deal_row(dl):
    """Сделка → строка разбора или причина исключения. Возвращает: (строка | None, причина | None). Ошибок нет."""
    try:
        c = calc(dl["plan"])
    except (KeyError, ValueError, TypeError, ArithmeticError):
        return None, "план не разобран"
    c["id"] = dl["id"]
    B = c["B"]
    buy = dl.get("buy")
    sells = dl.get("sells") or []
    sb = [s for s in sells if (s.get("request") or {}).get("venue") == B]
    text = " ".join(str(t[1]) for t in dl.get("timeline") or [] if isinstance(t, list) and len(t) > 1)
    if dl.get("step") != "Closed":
        return None, "не закрыта (%s)" % dl.get("step")
    if not buy or not sb:
        return None, "нет покупки или продажи на B"
    if len(sb) != len(sells) or (dl.get("relays") or 0) > 0:
        return None, "перегон или продажа не на B"
    if any(m in text for m in MANUAL_MARKS):
        return None, "ручные действия в хронологии"
    if dl.get("result_usd") is None or c["R"] is None:
        return None, "нет итога или строки запаса в плане"
    q = sum((D(s["filled_qty"]) for s in sb), Decimal(0))
    if q <= 0 or not c["sell_avg"]:
        return None, "продано 0"
    vwap = sum((D(s["filled_qty"]) * D(s["avg_price"]) for s in sb), Decimal(0)) / q
    first = min(sb, key=lambda s: ts(s["finished_at"]))
    filled = D(buy.get("filled_qty")) or Decimal(0)
    c.update({
        "scale": min(Decimal(1), filled / c["qty"]) if c["qty"] else Decimal(1),
        "actual": D(dl["result_usd"]), "return_cost_known": "return_cost_usd" in dl,
        "sell_vwap": vwap, "move": vwap / c["sell_avg"] - 1, "sold_qty": q,
        "t_sale": ts(first["finished_at"]), "T_real": ts(first["finished_at"]) - c["at"],
        "sell_block": ((first.get("chain") or {}).get("block")),
        "pb": c["profit"] + c["R"],
    })
    return c, None


def deals(raw):
    """Все сделки → (чистые строки, счётчик исключений по причинам). Ошибок нет."""
    rows, excl = [], {}
    for dl in raw:
        r, why = deal_row(dl)
        if r is None:
            excl[why] = excl.get(why, 0) + 1
        else:
            rows.append(r)
    return rows, excl
