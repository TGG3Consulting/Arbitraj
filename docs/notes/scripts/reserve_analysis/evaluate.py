"""Файл: docs/notes/scripts/reserve_analysis/evaluate.py
Что делает: перебор значений настроек запаса (по одной настройке в её области) и итог на истории:
сделки, которые вариант отсёк бы (их факт), плюс эпизоды отказов «только из-за запаса», которые он пропустил бы
(их расчётный исход по цене пула DEX). Сетки значений — GRIDS (это перебор для разбора, не советы и не настройки бота).
Кто вызывает: run.py.
От чего зависит: reserve.py, common.py.

Допущения (явно в выводе): объём сделки — как в записи (подбор объёма при другом запасе не повторяется); эпизод
даёт не больше одной сделки — по первой строке эпизода, которая прошла бы; исход отказа считается только у
биржа → DEX (цена пула), у остальных типов — «не оценено» (счётчик unknown).
"""

from decimal import Decimal

from reserve import admitted

GRIDS = {
    "profit.price_move_k": ["0", "0.5", "1", "1.5", "2", "2.5", "3", "3.5", "4", "4.5", "5", "6", "7", "8", "9", "12"],
    "profit.closure_share": ["0.05", "0.1", "0.15", "0.2", "0.25", "0.3", "0.35", "0.4", "0.5", "0.6", "0.8", "1"],
    "profit.sigma_floor_per_s": ["0.00002", "0.00005", "0.0001", "0.00015", "0.0002", "0.0003", "0.0005", "0.001"],
    "profit.transit_credit_s": ["0", "2", "5", "10", "15", "20", "30", "40", "60", "80", "100"],
    "profit.transit_manual_s": ["1", "5", "10", "15", "20", "30", "40", "50", "60", "80", "100"],
    "profit.sigma_min_changes": ["1"],
    "NEW reserve_min_share": ["0.0005", "0.001", "0.002", "0.003", "0.005", "0.0075", "0.01", "0.02", "0.03"],
}
SEGMENTS = ("all", "cex_dex", "dex_cex", "cex_cex")


def variants(venues_a, venues_b):
    """Список вариантов {setting, kind, sid, value, new}. venues_a/venues_b — площадки-источники/получатели из данных.
    Ошибок нет."""
    out = []
    for st, vals in GRIDS.items():
        if st in ("profit.price_move_k", "profit.sigma_min_changes"):
            scopes = [("Global", None)]
        elif st == "profit.transit_manual_s":
            scopes = [("Venue", v) for v in venues_a]
        else:
            scopes = [("Venue", v) for v in venues_b]
        for kind, sid in scopes:
            for v in vals:
                out.append({"setting": st, "kind": kind, "sid": sid, "value": Decimal(v), "new": st.startswith("NEW")})
    return out


def scope_rows(var, rows):
    """Строки, которые вариант может задеть: по области настройки (площадка B, площадка A или все). Ошибок нет."""
    st = var["setting"]
    if var["kind"] == "Global":
        return rows
    key = "A" if st == "profit.transit_manual_s" else "B"
    return [r for r in rows if r[key] == var["sid"]]


def _blank(var, seg):
    o = {"setting": var["setting"], "scope": var["sid"] or "global", "value": var["value"],
         "new_setting": var["new"], "segment": seg, "touched": 0}
    for label in ("period", "all"):
        o.update({label + "_deals_cut": 0, label + "_cut_sum": Decimal(0), label + "_cut_loss_n": 0,
                  label + "_cut_loss_sum": Decimal(0), label + "_cut_profit_sum": Decimal(0)})
    o.update({"episodes_admitted": 0, "admitted_unknown": 0, "admitted_cf_med_sum": Decimal(0),
              "admitted_cf_p75_sum": Decimal(0), "admitted_plus_n": 0})
    return o


def run_variant(var, deals_period, deals_all, eps, th):
    """Итог варианта по сегментам за один проход. deals_* — строки сделок (с cur, period), eps — эпизоды, у которых
    ro — строки «только из-за запаса» по времени (с cur, cf_med, cf_p75). Возвращает: список итогов по сегментам, где
    вариант задел хоть одну строку. Ошибок нет."""
    res = {seg: _blank(var, seg) for seg in SEGMENTS}

    def add(r, key, val):
        for seg in ("all", r["type"]):
            if seg in res:
                res[seg][key] += val

    for label, ds in (("period", deals_period), ("all", deals_all)):
        for d in scope_rows(var, ds):
            ok, hit = admitted(d, d["cur"], var, th)
            if not hit:
                continue
            add(d, "touched", 1)
            if not ok:
                add(d, label + "_deals_cut", 1)
                add(d, label + "_cut_sum", d["actual"])
                if d["actual"] < 0:
                    add(d, label + "_cut_loss_n", 1)
                    add(d, label + "_cut_loss_sum", d["actual"])
                else:
                    add(d, label + "_cut_profit_sum", d["actual"])
    for ep in eps:
        rows = scope_rows(var, ep["ro"])
        first = None
        for r in rows:
            ok, hit = admitted(r, r["cur"], var, th)
            if hit:
                add(r, "touched", 1)
            if ok:
                first = r
                break
        if first is None:
            continue
        add(first, "episodes_admitted", 1)
        if first.get("cf_med") is None:
            add(first, "admitted_unknown", 1)
            continue
        add(first, "admitted_cf_med_sum", first["cf_med"])
        add(first, "admitted_cf_p75_sum", first["cf_p75"] if first.get("cf_p75") is not None else Decimal(0))
        add(first, "admitted_plus_n", 1 if first["cf_med"] > 0 else 0)
    out = []
    for seg in SEGMENTS:
        o = res[seg]
        if not o["touched"]:
            continue
        o["net_period_med"] = o["admitted_cf_med_sum"] - o["period_cut_sum"]
        o["net_period_p75"] = o["admitted_cf_p75_sum"] - o["period_cut_sum"]
        o["net_all_deals_only"] = -o["all_cut_sum"]
        out.append(o)
    return out


def best(table, current):
    """Лучшее значение по (настройка, область, сегмент): наибольший net_period_med среди вариантов без неоценённых
    допусков (admitted_unknown = 0) и отдельно — по одним сделкам за всю историю (net_all_deals_only). При равенстве —
    ближайшее к текущему значению (current[(настройка, область)]; не задана — ближайшее к 0). Ошибок нет."""
    groups = {}
    for t in table:
        groups.setdefault((t["setting"], t["scope"], t["segment"]), []).append(t)
    out = []
    for (st, sc, seg), ts in sorted(groups.items()):
        cur = current.get((st, sc))
        ref = cur if cur is not None else Decimal(0)

        def key(field):
            return lambda t: (t[field], -abs(t["value"] - ref))

        clean = [t for t in ts if t["admitted_unknown"] == 0]
        b = max(clean, key=key("net_period_med")) if clean else None
        b_all = max(ts, key=key("net_all_deals_only"))
        out.append({"setting": st, "scope": sc, "segment": seg, "current": cur,
                    "best_period_value": b["value"] if b else None,
                    "best_period_net_med": b["net_period_med"] if b else None,
                    "best_period_net_p75": b["net_period_p75"] if b else None,
                    "best_period_deals_cut": b["period_deals_cut"] if b else None,
                    "best_period_episodes_admitted": b["episodes_admitted"] if b else None,
                    "best_all_deals_value": b_all["value"], "best_all_deals_net": b_all["net_all_deals_only"],
                    "best_all_deals_cut": b_all["all_deals_cut"],
                    "values_with_unknown": sum(1 for t in ts if t["admitted_unknown"] > 0)})
    return out
