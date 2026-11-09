"""Файл: docs/notes/scripts/reserve_analysis/report.py
Что делает: сводки ответов (А), (Б), проверка метода на сделках и запись выходных файлов out/ (summary.json,
episodes_cex_dex.csv, rows_reserve_only.csv, deals_check.csv, variants.csv, README.txt).
Кто вызывает: run.py.
От чего зависит: common.py, records.py (passes).
"""

from decimal import Decimal

from common import median, quantile, iso, write_csv
from records import passes

Z = Decimal(0)


def _stats(vals):
    v = [x for x in vals if x is not None]
    return {"n": len(v), "sum": sum(v, Z), "median": median(v), "p25": quantile(v, 0.25), "p75": quantile(v, 0.75)}


def _cf_block(rs, th):
    known = [r for r in rs if r.get("cf_med") is not None]
    return {
        "n": len(rs), "computed": len(known), "not_computed": len(rs) - len(known),
        "plus_med_n": sum(1 for r in known if r["cf_med"] > 0),
        "pass_threshold_med_n": sum(1 for r in known if passes(r["cf_med"], r["invested"], th)),
        "plus_p75_n": sum(1 for r in known if r.get("cf_p75") is not None and r["cf_p75"] > 0),
        "cf_med_sum": sum((r["cf_med"] for r in known), Z),
        "cf_p75_sum": sum((r["cf_p75"] for r in known if r.get("cf_p75") is not None), Z),
        "plan_profit_before_reserve_sum": sum((r["pb"] for r in known), Z),
        "reserve_sum": sum((r["R"] for r in known), Z),
        "dex_move_med_pct": _stats([(r["r_med"] - 1) * 100 for r in known]),
        "loss_med_n": sum(1 for r in known if r["cf_med"] < 0),
        "loss_med_sum": sum((r["cf_med"] for r in known if r["cf_med"] < 0), Z),
    }


def summary_a(eps, th):
    """(А): отказы биржа → DEX «только из-за запаса» — по строкам и по эпизодам (первая такая строка эпизода)."""
    rows = [r for ep in eps for r in ep["ro"] if r["type"] == "cex_dex"]
    firsts = [ep["ro"][0] for ep in eps if ep["ro"] and ep["ro"][0]["type"] == "cex_dex"]
    reasons = {}
    for r in rows:
        if r.get("cf_med") is None:
            reasons[r.get("cf_err") or "?"] = reasons.get(r.get("cf_err") or "?", 0) + 1
    by_a = {}
    for r in firsts:
        by_a.setdefault(r["A"], []).append(r)
    return {
        "rows": _cf_block(rows, th),
        "episodes": _cf_block(firsts, th),
        "pairs_coin_direction": len({ep["key"] for ep in eps if ep["ro"] and ep["ro"][0]["type"] == "cex_dex"}),
        "not_computed_reasons": dict(sorted(reasons.items(), key=lambda x: -x[1])),
        "episodes_by_source": {a: _cf_block(v, th) for a, v in sorted(by_a.items())},
    }


def move_vs_reserve(ds):
    """Запас плана против движения продажи по сделкам (как в разборе price-move-vs-reserve, п.4): масштаб частичной
    покупки — scale. Движение в USDT = move × проданное × plan.sell_avg (минус — против нас)."""
    out = {}
    for typ in ("cex_dex", "dex_cex", "cex_cex"):
        s = [d for d in ds if d["type"] == typ]
        mv = [d["move"] * d["sold_qty"] * d["sell_avg"] for d in s]
        out[typ] = {"n": len(s), "reserve_sum": sum((d["R"] * d["scale"] for d in s), Z),
                    "move_net_usdt": -sum(mv, Z), "move_adverse_usdt": -sum((m for m in mv if m < 0), Z),
                    "actual_sum": sum((d["actual"] for d in s), Z), "loss_n": sum(1 for d in s if d["actual"] < 0),
                    "loss_sum": sum((d["actual"] for d in s if d["actual"] < 0), Z),
                    "T_real_s": _stats([d["T_real"] for d in s]), "T_plan_s": _stats([d["T"] for d in s])}
    return out


def need_to_cut(d, th):
    """Сколько запаса нужно было, чтобы сделку отсечь: R > pb − требуемая прибыль; в долях: от выручки, k, доля
    закрытия, граница σ (при текущем k и T плана). Ошибок нет."""
    req = max(th["min_usd"], th["min_pct"] / 100 * d["invested"])
    need = d["pb"] - req
    P, sq = d["proceeds"], d["sqrtT"]
    spread = (d["sell_avg"] - d["buy_avg"]) / d["buy_avg"] if d["buy_avg"] else None
    k = d["cur"]["k"]
    return {"need_usdt": need, "share_of_proceeds": need / P if P else None,
            "k_needed": need / (d["sigma"] * sq * P) if d["sigma"] and sq and P else None,
            "closure_share_needed": need / (spread * P) if spread and spread > 0 and P else None,
            "sigma_floor_needed": need / (k * sq * P) if k and sq and P else None,
            "current_share_of_proceeds": d["R"] / P if P else None}


def summary_b(deals_all, eps, windows, th):
    """(Б): DEX → биржа — по исполненным сделкам; отказы — только счётчик и покрытие окнами стакана."""
    s = [d for d in deals_all if d["type"] == "dex_cex"]
    losses = [d for d in s if d["actual"] < 0]
    nd = [need_to_cut(d, th) for d in losses]
    by_b = {}
    for d in s:
        by_b.setdefault(d["B"], []).append(d)
    rej = [r for ep in eps for r in ep["ro"][:1] if r["type"] == "dex_cex"]
    covered = 0
    for r in rej:
        t1 = r["at"] + (r.get("T_real") or r["T"])
        for w in windows:
            ad = ((w.get("asset") or {}).get("bsc_contract") or "").lower()
            vs = [v.get("venue") for v in w.get("venues") or []]
            st, en = w.get("_st"), w.get("_en")
            if ad == r["coin_addr"] and r["B"] in vs and st is not None and st <= r["at"] and (en is None or en >= t1):
                covered += 1
                break
    return {
        "deals": {"n": len(s), "actual_sum": sum((d["actual"] for d in s), Z), "loss_n": len(losses),
                  "loss_sum": sum((d["actual"] for d in losses), Z)},
        "by_sell_venue": {b: {"n": len(v), "actual_sum": sum((d["actual"] for d in v), Z),
                              "loss_n": sum(1 for d in v if d["actual"] < 0),
                              "loss_sum": sum((d["actual"] for d in v if d["actual"] < 0), Z),
                              "move_pct": _stats([d["move"] * 100 for d in v]),
                              "reserve_share_pct": _stats([d["R"] / d["proceeds"] * 100 for d in v if d["proceeds"]])}
                         for b, v in sorted(by_b.items())},
        "losing_deals_need": {k: _stats([x[k] for x in nd]) for k in ("need_usdt", "share_of_proceeds", "k_needed",
                                                                          "closure_share_needed", "sigma_floor_needed",
                                                                          "current_share_of_proceeds")},
        "rejections_reserve_only_episodes": len(rej),
        "rejections_covered_by_window": covered,
        "rejections_note": "не посчитано: цены биржи после отказа в данных бота нет — окна (/api/windows) пишутся, "
                           "когда кандидат прошёл (core/window_run.rs pass); покрытие окнами — rejections_covered_by_window",
    }


def deals_check_stats(ds):
    """Проверка метода: модель пула (r − 1) против факта продажи на DEX (move) у сделок биржа → DEX."""
    v = [d for d in ds if d["type"] == "cex_dex" and d.get("model_r") is not None]
    if not v:
        return {"n": 0}
    m = [d["model_r"] - 1 for d in v]
    f = [d["move"] for d in v]
    diff = [abs(a - b) * 100 for a, b in zip(m, f)]
    same = sum(1 for a, b in zip(m, f) if (a > 0) == (b > 0) or (a == 0 and abs(b) < Decimal("0.00005")))
    n = len(v)
    ma, mb = sum(m, Z) / n, sum(f, Z) / n
    cov = sum(((a - ma) * (b - mb) for a, b in zip(m, f)), Z)
    va = sum(((a - ma) ** 2 for a in m), Z)
    vb = sum(((b - mb) ** 2 for b in f), Z)
    corr = cov / (va.sqrt() * vb.sqrt()) if va > 0 and vb > 0 else None
    return {"n": n, "abs_diff_pct": _stats(diff), "same_sign_n": same, "pearson": corr,
            "model_move_pct": _stats([x * 100 for x in m]), "fact_move_pct": _stats([x * 100 for x in f]),
            "not_computed_reasons": _count(d.get("model_err") for d in ds if d["type"] == "cex_dex" and d.get("model_r") is None)}


def _count(it):
    out = {}
    for x in it:
        out[x or "?"] = out.get(x or "?", 0) + 1
    return dict(sorted(out.items(), key=lambda kv: -kv[1]))


EP_COLS = ["episode", "type", "coin", "coin_addr", "A", "B", "start", "end", "rows", "reserve_only_rows", "first_id", "first_at",
           "profit", "R", "pb", "invested", "proceeds", "sigma", "T_plan", "T_real_med", "T_real_p75", "route_block",
           "b1_med", "b1_p75", "events_med", "dex_move_med_pct", "dex_move_p75_pct", "cf_med", "cf_p75", "plus_med",
           "pool_kinds", "cf_err"]


def ep_line(i, ep):
    """Эпизод → строка CSV (данные первой строки «только из-за запаса»). Ошибок нет."""
    r = ep["ro"][0]
    return {"episode": i, "type": r["type"], "coin": r["coin"], "coin_addr": r["coin_addr"], "A": r["A"], "B": r["B"],
            "start": iso(ep["start"]), "end": iso(ep["end"]), "rows": len(ep["rows"]), "reserve_only_rows": len(ep["ro"]),
            "first_id": r["id"], "first_at": iso(r["at"]), "profit": r["profit"], "R": r["R"], "pb": r["pb"],
            "invested": r["invested"], "proceeds": r["proceeds"], "sigma": r["sigma"], "T_plan": r["T"],
            "T_real_med": r.get("T_real"), "T_real_p75": r.get("T_real_p75"), "route_block": r.get("b0"),
            "b1_med": r.get("b1_med"), "b1_p75": r.get("b1_p75"), "events_med": r.get("events_med"),
            "dex_move_med_pct": (r["r_med"] - 1) * 100 if r.get("r_med") is not None else None,
            "dex_move_p75_pct": (r["r_p75"] - 1) * 100 if r.get("r_p75") is not None else None,
            "cf_med": r.get("cf_med"), "cf_p75": r.get("cf_p75"),
            "plus_med": None if r.get("cf_med") is None else int(r["cf_med"] > 0),
            "pool_kinds": r.get("pool_kinds"), "cf_err": r.get("cf_err")}


DEAL_COLS = ["id", "type", "coin", "A", "B", "plan_at", "profit", "R", "pb", "actual", "return_cost_known", "scale",
             "T_plan", "T_real", "move_pct", "R_share_pct", "route_block", "sell_block", "model_move_pct", "model_err",
             "in_period"]


def deal_line(d):
    """Сделка → строка CSV. Ошибок нет."""
    return {"id": d["id"], "type": d["type"], "coin": d["coin"], "A": d["A"], "B": d["B"], "plan_at": iso(d["at"]),
            "profit": d["profit"], "R": d["R"], "pb": d["pb"], "actual": d["actual"],
            "return_cost_known": int(d["return_cost_known"]), "scale": d["scale"], "T_plan": d["T"], "T_real": d["T_real"],
            "move_pct": d["move"] * 100, "R_share_pct": d["R"] / d["proceeds"] * 100 if d["proceeds"] else None,
            "route_block": d.get("b0"), "sell_block": d.get("sell_block"),
            "model_move_pct": (d["model_r"] - 1) * 100 if d.get("model_r") is not None else None,
            "model_err": d.get("model_err"), "in_period": int(d["period"])}


def write_tables(out, eps, rows_ro, deals, table):
    """Таблицы CSV в каталог out. Ошибки: OSError."""
    cex = [ep for ep in eps if ep["ro"] and ep["ro"][0]["type"] == "cex_dex"]
    write_csv(out + "/episodes_cex_dex.csv", [ep_line(i, ep) for i, ep in enumerate(cex, 1)], EP_COLS)
    rl = [dict(ep_line(0, {"ro": [r], "rows": [r], "start": r["j_at"], "end": r["j_last"]}), episode=r.get("ep"))
          for r in rows_ro]
    write_csv(out + "/rows_reserve_only.csv", rl, EP_COLS)
    write_csv(out + "/deals_check.csv", [deal_line(d) for d in deals], DEAL_COLS)
    cols = list(table[0].keys()) if table else ["setting"]
    write_csv(out + "/variants.csv", table, cols)
