"""Файл: docs/notes/scripts/reserve_analysis/checks.py
Что делает: проверки разбора на данных бота: порог каждой сделки на момент плана (исполненная сделка обязана его
проходить), текущие значения настроек в областях перебора, проверка «как сейчас» (вариант, равный текущему значению,
сколько сделок отсёк бы), счётчик строк по типам.
Кто вызывает: run.py.
От чего зависит: settings_at.py, records.py (passes), evaluate.py (GRIDS).
"""

from evaluate import GRIDS
from records import passes
from settings_at import thresholds_at, thresholds_now


def deal_thresholds(S, dl):
    """Порог каждой сделки на момент плана (settings_at.thresholds_at). Счётчики: plan_below_threshold_at_time — план
    исполненной сделки НЕ проходит порог своего момента (должен быть 0), plan_below_threshold_now — не проходит
    текущий (так прежняя версия «отсекала» сделки). Возвращает: (сделки с th, счётчики). Ошибок нет."""
    out, cnt = [], {"no_threshold_at_time": 0, "plan_below_threshold_at_time": 0, "plan_below_threshold_now": 0}
    now = thresholds_now(S)
    for d in dl:
        try:
            d["th"] = thresholds_at(S, d)
        except RuntimeError:
            cnt["no_threshold_at_time"] += 1
            continue
        cnt["plan_below_threshold_at_time"] += int(not passes(d["profit"], d["invested"], d["th"]))
        if now["min_usd"] is not None and now["min_pct"] is not None:
            cnt["plan_below_threshold_now"] += int(not passes(d["profit"], d["invested"], now))
        out.append(d)
    return out, cnt


def currents(S, va, vb):
    """Текущие значения настроек в областях перебора: {(настройка, область): Decimal | None}. Ошибок нет."""
    out = {}
    for st in GRIDS:
        if st in ("profit.price_move_k", "profit.sigma_min_changes"):
            out[(st, "global")] = S.dec(st, "global")
            continue
        for sid in (va if st == "profit.transit_manual_s" else vb):
            out[(st, sid)] = None if st.startswith("NEW") else S.dec(st, "venue:%s" % sid)
    return out


def check_current(table, cur):
    """Проверка «как сейчас»: вариант, равный текущему значению настройки в области, — сколько сделок он отсёк бы
    (порог и прочие настройки — на момент сделки). У настройки, не менявшейся за период сделок, должно быть 0; у
    менявшейся (settings_history_relevant) отсекаются сделки до изменения. Ошибок нет."""
    out = []
    for t in table:
        c = cur.get((t["setting"], t["scope"]))
        if c is not None and t["value"] == c:
            out.append({k: t[k] for k in ("setting", "scope", "value", "segment", "touched", "period_deals_cut",
                                          "period_cut_sum", "all_deals_cut", "all_cut_sum")})
    return out


def by_type(rs):
    """Число строк по типу направления. Ошибок нет."""
    out = {}
    for r in rs:
        out[r["type"]] = out.get(r["type"], 0) + 1
    return out
