"""Файл: docs/notes/scripts/reserve_analysis/reserve.py
Что делает: формула запаса бота и её варианты существующими настройками — пересчёт запаса строки (отказа или плана
сделки) при другом значении одной настройки в её области.
Кто вызывает: run.py, report.py.
От чего зависит: common.py, records.py (passes).

Факты кода бота:
- запас = k·σ·√T·выручка; задана доля закрытия — max(того же, доля · спред · выручка), спред = (sell_avg − buy_avg) /
  buy_avg, спред ≤ 0 — доля не действует (core/reserve.rs amount, вызов core/costs.rs:146-154 с Calc.buy_avg/sell_avg);
- k = profit.price_move_k, область монета → вся система (core/dirctx.rs:184-186, settings/store.rs resolve: монета →
  площадка → глобально; у k площадки нет — каталог GC);
- доля = profit.closure_share [площадка B] → глобально (reserve.rs at_b); σ плана = max(σ ряда, profit.sigma_floor_per_s
  [B]) (reserve.rs sigma_now) — σ в плане уже после границы;
- profit.sigma_min_changes: σ = 0 ⇔ все изменения цены в окне нулевые (vol.rs measure: changes — число ненулевых) →
  при значении 1 отсекаются ровно строки с σ = 0; при ≥ 2 по данным плана не проверить (число изменений не пишется);
- T: режим «вручную» = transit_manual_s [A] + transit_credit_s [B] (не задана — 0), «manual_auto» — среднее последних
  transit_auto_after переводов монеты в направлении, иначе «вручную» (core/transit.rs seconds; области — ровно площадка).
  Строка считается «T вручную», если T плана = текущему «вручную» — только у неё варианты T что-то меняют.
"""

from decimal import Decimal

from records import passes


class Settings:
    """Значения /api/settings: [{name, scope{kind, id}, value}] → чтение как у бота."""

    def __init__(self, items):
        self.v = {}
        for it in items:
            sc = it.get("scope") or {}
            self.v[(it["name"], sc.get("kind"), sc.get("id"))] = it.get("value")

    def exact(self, name, kind="Global", sid=None):
        """Значение ровно в области или None. Ошибок нет."""
        return self.v.get((name, kind, sid))

    def resolve(self, name, venue=None, coin=None):
        """Как settings/store.rs resolve (без псевдонимов тикеров): монета → площадка → глобально. Ошибок нет."""
        for k, i in (("Coin", coin), ("Venue", venue), ("Global", None)):
            if k != "Global" and i is None:
                continue
            x = self.exact(name, k, i)
            if x is not None:
                return x
        return None

    def dec(self, name, venue=None, coin=None):
        x = self.resolve(name, venue, coin)
        return None if x is None else Decimal(str(x))

    def venues_with(self, name):
        """Площадки, у которых настройка задана. Ошибок нет."""
        return sorted(i for (n, k, i) in self.v if n == name and k == "Venue")


def thresholds(s):
    """Пороги money.* (глобально). Ошибки: RuntimeError — не заданы."""
    u, p = s.dec("money.min_profit_usd"), s.dec("money.min_profit_pct")
    if u is None or p is None:
        raise RuntimeError("не заданы money.min_profit_usd / money.min_profit_pct")
    return {"min_usd": u, "min_pct": p}


def current(s, r):
    """Текущие параметры запаса строки: k, доля, граница σ, «вручную» и режим T. Ошибок нет (None — не задано)."""
    send = s.exact("profit.transit_manual_s", "Venue", r["A"])
    credit = s.exact("profit.transit_credit_s", "Venue", r["B"])
    manual = None if send is None else Decimal(str(send)) + Decimal(str(credit or 0))
    return {
        "k": s.dec("profit.price_move_k", coin=r["coin_key"]),
        "k_coin": s.exact("profit.price_move_k", "Coin", r["coin_key"]) is not None,
        "share": s.dec("profit.closure_share", venue=r["B"]),
        "floor": s.dec("profit.sigma_floor_per_s", venue=r["B"]),
        "send": None if send is None else Decimal(str(send)),
        "credit": Decimal(str(credit or 0)),
        "manual": manual,
        "mode": s.exact("profit.transit_mode", "Venue", r["A"]),
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
    if st == "profit.price_move_k" and not cur["k_coin"]:
        k, hit = v, True
    elif st == "profit.closure_share" and r["B"] == var["sid"]:
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


def admitted(r, cur, var, th):
    """Проходит ли строка порог при варианте (по прибыли до запаса минус новый запас). Ошибок нет."""
    R2, hit = new_reserve(r, cur, var)
    if R2 is None:
        return False, hit
    return passes(r["pb"] - R2, r["invested"], th), hit
