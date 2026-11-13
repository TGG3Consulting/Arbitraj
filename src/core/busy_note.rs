//! Файл: core/busy_note.rs
//! Что делает: решение 267 — учёт итогов теневых расчётов (busy_calc.rs): сводка направления (busy_sum.rs) в памяти
//! ядра и копией в открытом окне монеты (WindowInfo.busy — окно живёт до конца нашей сделки; окна нет или окна
//! выключены — только журнал); наша сделка для сравнения — открытая сделка монеты из учёта ядра (последняя по началу,
//! как start_reason::busy), без запросов. Когда монета освободилась (её нет в inner.active) — ОДНА строка журнала
//! core.busy_calc на направление и сводка забывается. Конец занятости ловит своя задача на монету: исполнитель и
//! замок не трогаются, поэтому она сверяет inner.active раз в window.busy_calc_min_ms (чтение памяти, не запрос).
//! Сменилась сделка монеты между двумя итогами (новая сделка началась раньше, чем задача увидела конец прежней) —
//! сводка прежней сделки пишется в журнал, для новой начинается своя.
//! Итог расчёта, пришедший после конца занятости, когда сводка уже записана, отбрасывается (второй строки нет).
//! Перезапуск бота сводку в памяти не сохраняет (в окне — то, что было записано к последнему сохранению сводки окна).
//! Задача монеты запускается с первым обращением тика по занятой монете (busy_calc.rs enqueue) — она же забывает
//! начало занятости, от которого отсчитывается первый теневой расчёт.
//! Кто вызывает: core/busy_calc.rs (record, watch_coin, open_deal).
//! От чего зависит: busy_calc.rs (State, min_ms, KIND), busy_sum.rs, window.rs (note_busy), deal.rs, coin_key.rs.
//! Настройки: window.busy_calc_min_ms (через busy_calc.rs).

use std::time::Duration;

use chrono::Utc;

use crate::core::types::Direction;
use crate::storage::journal::Level;

use super::busy_calc::{KIND, min_ms};
use super::busy_sum::{BusyDeal, BusySum, Outcome};
use super::coin_key::CoinKey;
use super::deal::Deal;
use super::engine::{Core, lk};

/// Что делает: итог теневого расчёта — в сводку направления и её копию в открытое окно монеты; первая запись по
/// монете запускает задачу, ждущую конца занятости. Только память (вызывается и из Drop). Принимает: ядро,
/// направление, итог. Ошибок нет.
pub fn record(core: &Core, d: &Direction, o: Outcome) {
    let st = &core.inner.busy_calc;
    let key = CoinKey::of(&d.asset);
    let busy = lk(&core.inner.active).contains(&key);
    let deal = open_deal(core, &key);
    let now = Utc::now();
    let (sum, old) = {
        let mut sums = lk(&st.sums);
        if !busy && !sums.contains_key(d) {
            // Монета уже свободна и сводка записана — поздний итог не начинает новую.
            return;
        }
        let changed = match (sums.get(d).and_then(|s| s.deal.as_ref()), &deal) {
            (Some(a), Some(b)) => a.id != b.id,
            _ => false,
        };
        let old = if changed { sums.remove(d) } else { None };
        let s = sums
            .entry(d.clone())
            .or_insert_with(|| BusySum::new(d.clone(), now));
        s.note(o, now);
        if deal.is_some() {
            s.deal = deal;
        }
        (s.clone(), old)
    };
    lk(&core.inner.windows).note_busy(&sum);
    // Из Drop при остановке рантайма задач не запускаем (строка журнала тогда не пишется).
    if let (Some(old), Ok(rt)) = (old, tokio::runtime::Handle::try_current()) {
        let c = core.clone();
        rt.spawn(async move { log(&c, old).await });
    }
    watch_coin(core, key);
}

/// Что делает: запускает задачу, ждущую конца занятости монеты, если её ещё нет (первое обращение тика по занятой
/// монете или первый итог). Принимает: ядро, ключ монеты. Ошибок нет: рантайма нет (остановка) — ничего.
pub(super) fn watch_coin(core: &Core, key: CoinKey) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    if lk(&core.inner.busy_calc.watching).insert(key.clone()) {
        rt.spawn(watch(core.clone(), key));
    }
}

/// Открытая сделка монеты из учёта ядра (последняя по началу) — для сравнения. Ошибок нет.
pub(super) fn open_deal(core: &Core, key: &CoinKey) -> Option<BusyDeal> {
    lk(&core.inner.deals)
        .values()
        .filter(|x| x.is_open() && &CoinKey::of(&x.direction.asset) == key)
        .max_by_key(|x| x.started_at)
        .map(deal_of)
}

/// Числа сделки для сравнения: вложено по факту, вложение и прибыль плана (из учёта, без запросов). Ошибок нет.
fn deal_of(x: &Deal) -> BusyDeal {
    BusyDeal {
        id: x.id.clone(),
        direction: x.direction.clone(),
        invested: x.invested,
        plan_invested: x
            .plan
            .get("invested")
            .and_then(|v| serde_json::from_value(v.clone()).ok()),
        plan_profit_usd: x.plan_profit_usd,
    }
}

/// Что делает: ждёт, пока монета освободится (её нет в inner.active; сверка раз в window.busy_calc_min_ms; настройка
/// снята — сразу), затем пишет по строке журнала на направление и забывает сводки монеты. Принимает: ядро, ключ монеты.
/// Ошибок нет.
async fn watch(core: Core, key: CoinKey) {
    while let Ok(ms) = min_ms(&core) {
        tokio::time::sleep(Duration::from_millis(ms)).await;
        if !lk(&core.inner.active).contains(&key) {
            break;
        }
    }
    let st = &core.inner.busy_calc;
    lk(&st.watching).remove(&key);
    let done: Vec<BusySum> = {
        let mut sums = lk(&st.sums);
        let dirs: Vec<Direction> = sums
            .keys()
            .filter(|d| CoinKey::of(&d.asset) == key)
            .cloned()
            .collect();
        dirs.iter().filter_map(|d| sums.remove(d)).collect()
    };
    st.forget(&key);
    for mut s in done {
        // Итоговые числа сделки (вложено по факту) — из учёта на момент записи.
        let id = s.deal.as_ref().map(|x| x.id.clone());
        if let Some(x) = id.and_then(|id| lk(&core.inner.deals).get(&id).map(deal_of)) {
            s.deal = Some(x);
        }
        lk(&core.inner.windows).note_busy(&s);
        log(&core, s).await;
    }
}

/// Одна строка журнала core.busy_calc со сводкой (данные записи — сводка целиком). Ошибок нет.
async fn log(core: &Core, s: BusySum) {
    let venue = s.direction.buy_on.0.clone();
    let data = serde_json::to_value(&s).ok();
    let deps = &core.inner.deps;
    deps.log(Level::Info, KIND, Some(&venue), s.text(), data)
        .await;
}
