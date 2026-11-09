"""Файл: docs/notes/scripts/reserve_analysis/settings_at.py
Что делает: значения настроек бота НА МОМЕНТ строки/сделки — по текущим значениям (/api/settings) и истории изменений
(/api/settings/history), чтение как у бота (монета → площадка → вся система; решение 242 — область «площадка · тип
направления» по правилу стороны) и пороги прибыли на момент.
Кто вызывает: reserve.py, run.py.
От чего зависит: common.py (ts).

Факты кода бота:
- история — settings/history.rs: {name, scope (ключ SQLite: «global», «venue:bingx», «coin:0x…», «venue_dir:bingx:cex_dex»,
  «venue_dir:dex_dex» — settings/scope.rs to_key), old, new, by, at}, новые первыми; /api/settings — {name,
  scope{kind, id}, value} (serde tag kind, content id);
- значение на момент t: new последнего изменения с at ≤ t; изменений до t нет — old первого изменения после t
  (null — не было задано); изменений нет вовсе — текущее значение;
- правило стороны (core/venue_dir.rs rule): биржа → DEX — (A, cex_dex); DEX → биржа — (B, dex_cex); биржа → биржа —
  (A, cex_cex); DEX ↔ DEX — «dex_dex». Вид площадки (биржа/DEX) здесь — по наличию маршрута DEX в котировке стороны
  (records.py calc: type), у бота — по видам шлюза;
- пороги money.min_profit_usd / money.min_profit_pct: пара → вся система (core/venue_dir.rs thresholds → decimal_dir);
- profit.price_move_k: монета → пара → вся система (settings/venue_dir.rs decimal_coin_dir, решение 242 доп. п. 2);
- profit.closure_share: пара → площадка B → вся система (core/reserve.rs closure_share).
"""

from decimal import Decimal

from common import ts


def key_of(scope):
    """Область /api/settings {kind, id} → ключ истории. Ошибок нет (неизвестный вид — «kind:id» строчными)."""
    kind = (scope or {}).get("kind")
    if kind in (None, "Global"):
        return "global"
    k = {"Venue": "venue", "Coin": "coin", "Event": "event", "Dex": "dex", "Channel": "channel",
         "VenueDir": "venue_dir"}.get(kind, str(kind).lower())
    return "%s:%s" % (k, scope.get("id"))


def pair_key(r):
    """Ключ области «площадка · тип направления» строки по правилу стороны. Ошибок нет."""
    t = r["type"]
    if t == "dex_dex":
        return "venue_dir:dex_dex"
    venue = r["B"] if t == "dex_cex" else r["A"]
    return "venue_dir:%s:%s" % (venue, t)


class SettingsAt:
    """Текущие значения + история → значение на момент. items — /api/settings, hist — /api/settings/history."""

    def __init__(self, items, hist):
        self.now = {}
        for it in items:
            self.now[(it["name"], key_of(it.get("scope")))] = it.get("value")
        self.changes = {}
        for h in hist or []:
            if not isinstance(h, dict) or "name" not in h:
                continue
            self.changes.setdefault((h["name"], h.get("scope")), []).append((ts(h["at"]), h.get("old"), h.get("new")))
        for v in self.changes.values():
            v.sort(key=lambda x: x[0])
        self.hist_n = len(hist or [])

    def exact(self, name, key, t=None):
        """Значение ровно в области key на момент t (None — сейчас). Ошибок нет (None — не задано)."""
        ch = self.changes.get((name, key))
        if t is None or not ch:
            return self.now.get((name, key))
        val, seen = None, False
        for at, old, new in ch:
            if at <= t:
                val, seen = new, True
            else:
                return val if seen else old
        return val

    def dec(self, name, key, t=None):
        x = self.exact(name, key, t)
        return None if x is None else Decimal(str(x))

    def chain(self, name, keys, t=None):
        """Первое заданное по цепочке ключей. Возвращает: (Decimal | None, ключ | None). Ошибок нет."""
        for k in keys:
            x = self.dec(name, k, t)
            if x is not None:
                return x, k
        return None, None

    def keys(self, name):
        """Все ключи областей, где настройка задана сейчас или менялась. Ошибок нет."""
        return sorted({k for (n, k) in list(self.now) + list(self.changes) if n == name})


def thresholds_at(S, r):
    """Пороги прибыли строки на её момент r["at"]: пара → вся система. Возвращает: {min_usd, min_pct, src}.
    Ошибки: RuntimeError — не заданы на этот момент."""
    pk = pair_key(r)
    u, ku = S.chain("money.min_profit_usd", (pk, "global"), r["at"])
    p, kp = S.chain("money.min_profit_pct", (pk, "global"), r["at"])
    if u is None or p is None:
        raise RuntimeError("пороги money.min_profit_usd / money.min_profit_pct не заданы на %s" % r["at"])
    return {"min_usd": u, "min_pct": p, "src": "%s|%s" % (ku, kp)}


def thresholds_now(S):
    """Пороги сейчас (вся система) — для сводки. Ошибок нет (None — не заданы)."""
    return {"min_usd": S.dec("money.min_profit_usd", "global"), "min_pct": S.dec("money.min_profit_pct", "global")}


def timeline(S, names):
    """Изменения настроек names по времени — для сводки: [{name, scope, at, old, new}]. Ошибок нет."""
    out = []
    for (n, k), ch in S.changes.items():
        if n in names:
            out.extend({"name": n, "scope": k, "at": at, "old": old, "new": new} for at, old, new in ch)
    return sorted(out, key=lambda x: x["at"])
