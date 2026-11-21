"""Файл: docs/notes/scripts/reserve_analysis/reserve.py
Что делает: формула запаса бота и её варианты существующими настройками — пересчёт запаса строки (отказа или плана
сделки) при другом значении одной настройки в её области.
Кто вызывает: run.py, report.py.
От чего зависит: common.py, records.py (passes), settings_at.py (значения на момент, правило стороны).

Факты кода бота:
- запас = k·σ·√T·выручка; задана доля закрытия — max(того же, доля · спред · выручка), спред = (sell_avg − buy_avg) /
  buy_avg, спред ≤ 0 — доля не действует (core/reserve.rs amount, вызов core/costs.rs:146-154 с Calc.buy_avg/sell_avg);
- k = profit.price_move_k: монета → площадка · тип направления → вся система (core/dirctx.rs decimal_coin_dir, решение
  242 доп. п. 2);
- доля = profit.closure_share: площадка · тип направления → площадка B → вся система (reserve.rs closure_share);
  σ плана = max(σ ряда, profit.sigma_floor_per_s [B]) (reserve.rs sigma_now) — σ в плане уже после границы;
- все значения — НА МОМЕНТ строки/сделки (Calc.at) по истории настроек (settings_at.py), не текущие;
- profit.sigma_min_changes: σ = 0 ⇔ все изменения цены в окне нулевые (vol.rs measure: changes — число ненулевых) →
  при значении 1 отсекаются ровно строки с σ = 0; при ≥ 2 по данным плана не проверить (число изменений не пишется);
- T: режим «вручную» = transit_manual_s [A] + transit_credit_s [B] (не задана — 0), «manual_auto» — среднее последних
  transit_auto_after переводов монеты в направлении, иначе «вручную» (core/transit.rs seconds; области — ровно площадка).
  Строка считается «T вручную», если T плана = «вручную» на её момент — только у неё варианты T что-то меняют.
"""

from decimal import Decimal

from records import passes
from settings_at import pair_key


def current(S, r):
    """Параметры запаса строки НА ЕЁ МОМЕНТ r["at"] (история настроек, settings_at.py): k, доля, граница σ, «вручную»
    и режим T, откуда взяты k и доля (ключ области). Ошибок нет (None — не задано на тот момент)."""
    t, pk = r["at"], pair_key(r)
    A, B = "venue:%s" % r["A"], "venue:%s" % r["B"]
    k, k_src = S.chain("profit.price_move_k", ("coin:%s" % r["coin_key"], pk, "global"), t)
    share, share_src = S.chain("profit.closure_share", (pk, B, "global"), t)
    floor, _ = S.chain("profit.sigma_floor_per_s", (B, "global"), t)
    send = S.dec("profit.transit_manual_s", A, t)
    credit = S.dec("profit.transit_credit_s", B, t) or Decimal(0)
    return {
        "k": k, "k_src": k_src, "share": share, "share_src": share_src, "floor": floor,
        "send": send, "credit": credit, "manual": None if send is None else send + credit,
        "mode": S.exact("profit.transit_mode", A, t),
    }


def amount(k, sigma, sqrt_t, P, share, buy_avg, sell_avg, min_share=None):
    """Запас в USDT — core/reserve.rs amount (√T передаётся готовым); min_share — НОВАЯ (не существующая) нижняя
    граница доли выручки. Ошибок нет."""
    base = k * sigma * sqrt_t * P
    if share is not None and buy_avg and buy_avg > 0:
        spread = (sell_avg - buy_avg) / buy_avg
        if spread > 0:
            base = max(base, share * spread * P)
    if min_share is not None:
        base = max(base, min_share * P)
    return base


def model_check(r, cur):
    """Сходится ли запас строки с формулой при текущих настройках (k по настройке). Возвращает: (сходится, k по
    записи). Ошибок нет."""
    if cur["k"] is None or r["R"] is None:
        return False, None
    m = amount(cur["k"], r["sigma"], r["sqrtT"], r["proceeds"], cur["share"], r["buy_avg"], r["sell_avg"])
    base = r["sigma"] * r["sqrtT"] * r["proceeds"]
    k_impl = (r["R"] / base) if base else None
    tol = Decimal("1e-4") * max(Decimal(1), abs(r["R"])) if r["R"] < 1 else Decimal("1e-4") * abs(r["R"])
    return abs(m - r["R"]) <= max(tol, Decimal("1e-8")), k_impl


def new_reserve(r, cur, var):
    """Запас строки при варианте var {setting, kind, sid, value}. Возвращает: (запас, применён ли вариант к строке).
    Ошибок нет."""
    k, sigma, sq, share, ms = cur["k"], r["sigma"], r["sqrtT"], cur["share"], None
    st, v = var["setting"], var["value"]
    hit = False
    if st == "profit.price_move_k" and cur["k_src"] == "global":
        k, hit = v, True
    elif st == "profit.closure_share" and r["B"] == var["sid"] and cur["share_src"] != pair_key(r):
        share, hit = v, True
    elif st == "profit.sigma_floor_per_s" and r["B"] == var["sid"]:
        sigma, hit = max(sigma, v), True
    elif st == "profit.sigma_min_changes":
        hit = r["sigma"] == 0
    elif st == "profit.transit_credit_s" and r["B"] == var["sid"] and cur["manual"] is not None and r["T"] == cur["manual"]:
        sq, hit = (cur["send"] + v).sqrt(), True
    elif st == "profit.transit_manual_s" and r["A"] == var["sid"] and cur["manual"] is not None and r["T"] == cur["manual"]:
        sq, hit = (v + cur["credit"]).sqrt(), True
    elif st == "NEW reserve_min_share" and r["B"] == var["sid"]:
        ms, hit = v, True
    if not hit:
        return r["R"], False
    if st == "profit.sigma_min_changes":
        return None, True  # σ = 0 → σ не измерена → котировки направления нет (сделки нет)
    return amount(k, sigma, sq, r["proceeds"], share, r["buy_avg"], r["sell_avg"], ms), True


def admitted(r, cur, var):
    """Проходит ли строка порог НА СВОЙ МОМЕНТ (r["th"], settings_at.thresholds_at) при варианте — по прибыли до
    запаса минус новый запас. Ошибок нет."""
    R2, hit = new_reserve(r, cur, var)
    if R2 is None:
        return False, hit
    return passes(r["pb"] - R2, r["invested"], r["th"]), hit
