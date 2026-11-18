//! Файл: core/manual_wait_pause.rs
//! Что делает: решение 405 (t191, ответ Тиграна на ревью Н3; вынесено из manual_wait.rs — размер файла) — пауза новых
//! сделок монеты, пока ручное действие ждёт окончания её сделок: paused / paused_why (входы исполнителя, очередь
//! расчёта, take — coin_ride.rs), start_retry (сделки кончились — «повтор идёт»), claim / is_retry (ревью 4, Н3: пауза
//! держится, пока повтор той же команды не возьмёт монету в take).
//! Кто вызывает: coin_ride.rs, balance_sell.rs, manual_wait_run.rs (через реэкспорт manual_wait::*).
//! От чего зависит: manual_wait.rs (ожидания, current_action), coin_key.rs, engine.rs.
//! Настройки: нет.

use std::sync::atomic::Ordering;

use super::coin_key::CoinKey;
use super::engine::{Core, lk};

/// Что делает: стоит ли пауза новых сделок монеты (ждёт ручное действие). Быстрый путь — атомарный счётчик.
/// Принимает: ядро, ключ монеты. Ошибок нет.
pub fn paused(core: &Core, key: &CoinKey) -> bool {
    core.inner.manual_waits_live.load(Ordering::Acquire) != 0
        && lk(&core.inner.manual_waits).contains_key(key)
}

/// Что делает: повтор ждавшей команды берёт монету (ревью 4, Н3): пауза держится, пока повтор не возьмёт монету, —
/// иначе между снятием паузы и take (в обработчике команды до take есть await) могла войти новая сделка. Пауза есть,
/// повтор идёт (running) и текущая команда задачи — та же (manual_wait::scope) — ожидание снимается этим вызовом.
/// Принимает: ядро, ключ. Возвращает: true — это повтор, пауза снята; false — пауза для этого вызывающего. Ошибок нет.
/// Вызывается под замком `active` (coin_ride::take; порядок active → manual_waits).
pub fn claim(core: &Core, key: &CoinKey) -> bool {
    if !is_retry(core, key) {
        return false;
    }
    let mut m = lk(&core.inner.manual_waits);
    if m.remove(key).is_some() {
        core.inner.manual_waits_live.fetch_sub(1, Ordering::AcqRel);
    }
    true
}

/// Что делает: сделки монеты кончились — ожидание переходит в «повтор идёт» (running): пауза держится, пока повтор не
/// возьмёт монету (claim). Вызывает manual_wait_run.rs (и тест порядка — ревью 4 М5). Принимает: ядро, ключ.
/// Возвращает: монету ожидания (None — ожидания нет). Ошибок нет.
pub fn start_retry(core: &Core, key: &CoinKey) -> Option<String> {
    let mut m = lk(&core.inner.manual_waits);
    m.get_mut(key).map(|w| {
        w.running = true;
        w.coin.clone()
    })
}

/// Что делает: текущая команда задачи — повтор ждавшей команды монеты (ожидание в состоянии running, то же действие).
/// Принимает: ядро, ключ. Ошибок нет.
pub fn is_retry(core: &Core, key: &CoinKey) -> bool {
    let Some(action) = super::manual_wait::current_action() else {
        return false;
    };
    lk(&core.inner.manual_waits)
        .get(key)
        .is_some_and(|w| w.running && w.action == action)
}

/// Что делает: причина отказа входа сделки при паузе. Принимает: ядро, ключ. Возвращает: текст; None — паузы нет.
/// Ошибок нет.
pub fn paused_why(core: &Core, key: &CoinKey) -> Option<String> {
    if core.inner.manual_waits_live.load(Ordering::Acquire) == 0 {
        return None;
    }
    lk(&core.inner.manual_waits).get(key).map(|w| {
        format!(
            "новые сделки {} на паузе: ручное действие «{}» ждёт окончания сделок (решение 405)",
            w.coin, w.what
        )
    })
}
