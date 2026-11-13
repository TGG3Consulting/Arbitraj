//! Файл: core/busy_budget.rs
//! Что делает: решение 267 (дополнение, Тигран 13.11) — бюджет ТЕНЕВОГО расчёта считается без вычета нашей текущей
//! сделки по монете: цель замера — «какой объём был бы выгоден, если бы мы не были заняты». Признак «теневой расчёт
//! монеты» задаётся на задачу tokio (task_local, как класс срочности gateway/urgency.rs и признак финальной котировки
//! gateway/quote_stage.rs); новая задача его не наследует. Только при этом признаке limits_parts::parts возвращает в
//! остаток лимита на монету и лимита «в пути» то, что из них вычла текущая сделка монеты (открытая сделка монеты из
//! учёта ядра, последняя по началу — та же, что в сводке замера): её «вложено − получено», как считает учёт
//! (ledger.rs recompute_exposure). Признака нет (кандидат, предпроверка, «без потолка», всё остальное) — ничего не
//! меняется. Свободный баланс площадки не читается (как у кандидата, решение 94).
//! Кто вызывает: core/busy_calc.rs (scope, freed), core/limits_parts.rs (add_back).
//! От чего зависит: deal.rs, coin_key.rs, limits_parts.rs (Parts), engine.rs, tokio (task_local).
//! Настройки: нет.

use std::future::Future;

use rust_decimal::Decimal;

use crate::core::types::Direction;

use super::coin_key::CoinKey;
use super::deal::Step;
use super::engine::{Core, lk};
use super::limits_parts::Parts;

tokio::task_local! {
    static SHADOW: CoinKey;
}

/// Выполняет работу как теневой расчёт монеты `key`. Принимает: ключ монеты, работу. Возвращает: её результат.
pub async fn scope<F: Future>(key: CoinKey, work: F) -> F::Output {
    SHADOW.scope(key, work).await
}

/// Что вычла из лимитов текущая сделка монеты теневого расчёта.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Freed {
    /// «Вложено − получено» сделки, USDT (не меньше 0).
    pub left: Decimal,
    /// Учтено ли в лимите на монету этого направления (учёт ведётся по монете сделки, как её знает сделка).
    pub coin: bool,
    /// Учтено ли в лимите «в пути» (сделка на шаге перевода).
    pub transit: bool,
}

/// Что делает: если текущая задача — теневой расчёт, находит текущую сделку его монеты и её долю в лимитах.
/// Принимает: ядро, направление расчёта. Возвращает: None — не теневой расчёт или открытой сделки монеты нет.
pub fn freed(core: &Core, d: &Direction) -> Option<Freed> {
    let key = SHADOW.try_with(Clone::clone).ok()?;
    let deals = lk(&core.inner.deals);
    let x = deals
        .values()
        .filter(|x| x.is_open() && CoinKey::of(&x.direction.asset) == key)
        .max_by_key(|x| x.started_at)?;
    Some(Freed {
        left: (x.invested - x.proceeds).max(Decimal::ZERO),
        coin: x.direction.asset == d.asset,
        transit: x.step == Step::Transfer,
    })
}

/// Что делает: в теневом расчёте возвращает в остатки лимитов долю текущей сделки монеты; иначе ничего.
/// Принимает: ядро, направление, составляющие бюджета (меняются на месте). Ошибок нет.
pub fn add_back(core: &Core, d: &Direction, p: &mut Parts) {
    let Some(f) = freed(core, d) else { return };
    if f.coin {
        p.coin_left += f.left;
    }
    if f.transit {
        p.transit_left += f.left;
    }
}
