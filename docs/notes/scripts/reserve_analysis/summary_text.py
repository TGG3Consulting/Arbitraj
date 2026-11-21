"""Файл: docs/notes/scripts/reserve_analysis/summary_text.py
Что делает: текст README.txt каталога out/ (что в файлах, метод, приближения) и краткая сводка в консоль.
Кто вызывает: run.py.
От чего зависит: common.py.
"""

from common import rnd

RELEVANT = ("profit.price_move_k", "profit.closure_share", "profit.sigma_floor_per_s", "profit.sigma_min_changes",
            "profit.sigma_window_min", "profit.transit_mode", "profit.transit_manual_s", "profit.transit_credit_s",
            "profit.transit_auto_after", "money.min_profit_usd", "money.min_profit_pct", "profit.liquidity_share_h")


def readme(a):
    """Текст README.txt. Принимает: аргументы запуска. Ошибок нет."""
    return """Разбор «запас на движение цены против реального движения» (решение 240). Скрипт: run.py, параметры: %s

Файлы:
- summary.json — всё вместе: пороги и настройки бота сейчас и история их изменений, счётчики,
  A_cex_dex_missed (вопрос А), B_dex_cex (вопрос Б), C_best (вопрос В), move_vs_reserve_*, method_check_deals,
  check_current_values_cut (вариант = текущему значению — сколько сделок отсёк бы), node (голова, earliest — самый
  ранний блок, который отдаёт узел, запросы, «не ответил за срок»).
- progress.log — ход разбора; перед КАЖДЫМ запросом к узлу — строка «узел → метод параметры», перед каждой строкой/
  сделкой — её id и блоки (последняя строка показывает, на чём стоит разбор).
- episodes_cex_dex.csv — эпизоды отказов биржа→DEX «только из-за запаса» (монета+направление, разрыв > %s с);
  числа — первой такой строки эпизода: plan (profit, R, pb = profit + R), T_plan, T_real_med/p75, блок котировки
  route_block, целевые блоки b1_med/b1_p75, изменение цены маршрута DEX, cf_med/cf_p75 = pb + proceeds × (r − 1).
- rows_reserve_only.csv — то же по каждой строке журнала (все типы направлений; у не биржа→DEX cf пусто).
- deals_check.csv — чистые сделки: план, факт (result_usd), T_real (решение → первая продажа), движение продажи (move),
  у биржа→DEX — модель пула от блока котировки до блока перед продажей (model_move_pct) для проверки метода.
- variants.csv — каждое значение каждой настройки в каждой области и сегменте: отсечённые сделки (period — за период
  журнала, all — вся история сделок) и пропущенные эпизоды (их cf), net_period_med = cf допущенных − факт отсечённых.
- cache_api/, cache_node.jsonl — кэш ответов (повторный запуск их использует; удалять не нужно).

Определения:
- «отказ только из-за запаса»: текст «отказ — прибыль … ниже порога» (core/profit.rs) И прибыль плана не проходит
  пороги money.min_profit_usd / money.min_profit_pct И прибыль + запас (PriceMoveReserve) проходит.
- T_real: plan.at → первая продажа на B завершена; медиана/p75 по паре (A,B) типа направления, иначе по A (min %s сделок).
- Блок «момент t»: последний блок с timestamp ≤ t (метки блоков узла, точность 1 с).
- Цена пула: V2 — Sync (reserve1/reserve0), V3/CL — Swap sqrtPriceX96² (token1 за token0); r хопа = цена «выход за вход»
  в целевом блоке / в блоке котировки; r доли — произведение по хопам, r маршрута — среднее по expected_out долей.

ПРИБЛИЖЕНИЯ (важно при чтении):
1) cf = прибыль до запаса + выручка × (r − 1): глубина пула не пересчитывается (меняется только предельная цена),
   цена покупки на бирже — по плану, затраты — по плану. Проверка на сделках — method_check_deals.
2) Объём — как в записи; бот подбирает объём по прибыли С запасом (size_search), при другом запасе объём мог быть иным.
3) Эпизод даёт не больше одной сделки (первая прошедшая строка). Строка журнала с повторами — числа последнего повтора.
4) У DEX→биржа и биржа→биржа исход отказа не считается (нет цены биржи после отказа) — admitted_unknown.
5) Варианты T меняют только строки с T плана = текущему «вручную» (manual_s[A] + credit_s[B]); у «авто» T не меняется.
6) Вид пула вне «V2, V3, CL в синглтоне» — «не поддержан разбором» (счётчик причин).
7) Пороги прибыли и настройки запаса строки/сделки — НА ЕЁ МОМЕНТ (Calc.at) по истории настроек бота
   (/api/settings/history) с областью «площадка · направление» (решение 242), не текущие.
8) «блок старше самого раннего доступного узлу (earliest = X)» — узел не хранит эти блоки/логи; такие строки и сделки
   не посчитаны (не оценены как «цена не менялась»). Ответы, уже лежащие в кэше прошлых запусков, используются и ниже
   earliest (node.cache_logs_below_earliest_used; --ignore-cache-below-earliest — нет). Архивный узел (--archive-url)
   — только если задан явно; его запросы — node.archive.
9) «узел не ответил за N с» — запрос не уложился в общий срок (--rpc-deadline-s); строка/сделка пропущена.
""" % (vars(a), a.gap_s, a.min_pair_deals)


def brief(s):
    """Краткая сводка в консоль. Ошибок нет."""
    out = ["== СВОДКА (подробно — summary.json) =="]
    A = s.get("A_cex_dex_missed")
    if isinstance(A, dict):
        for k in ("rows", "episodes"):
            b = A[k]
            out.append("(А) %s: всего %d, посчитано %d, в плюс (T мед) %d, сумма cf мед %s, cf p75 %s, не посчитано %d"
                       % (k, b["n"], b["computed"], b["plus_med_n"], rnd(b["cf_med_sum"], 4), rnd(b["cf_p75_sum"], 4),
                          b["not_computed"]))
    nd = s.get("node") or {}
    out.append("узел: голова %s, earliest %s (%s), %s" % (nd.get("head"), nd.get("earliest"), nd.get("earliest_at"),
                                                       nd.get("node")))
    dd = s["deals"]
    out.append("сделки: чистых %d; план ниже порога своего момента %d, ниже текущего %d"
               % (dd["clean"], dd["plan_below_threshold_at_time"], dd["plan_below_threshold_now"]))
    B = s["B_dex_cex"]["deals"]
    out.append("(Б) DEX→биржа сделок %d, итог %s, убыточных %d на %s" % (B["n"], rnd(B["actual_sum"], 4), B["loss_n"],
                                                                      rnd(B["loss_sum"], 4)))
    for c in s["C_best"]:
        if c["segment"] != "all":
            continue
        out.append("(В) %s [%s]: лучшее за период %s (net мед %s), по сделкам всей истории %s (net %s)"
                   % (c["setting"], c["scope"], c["best_period_value"], rnd(c["best_period_net_med"], 4),
                      c["best_all_deals_value"], rnd(c["best_all_deals_net"], 4)))
    return "\n".join(out)
