//! Файл: core/transit_source.rs
//! Что делает: решение 464 (t238, живой NUM 20.11: Gate → DEX — план 39 с из настроек 37 + 2, факт 58 с; по паре
//! gate → bsc_dex в истории 123 перевода других монет, среднее 49,1 с) — откуда взято время в пути T в режиме
//! «вручную + авто» (manual_auto, transit.rs): у монеты по паре площадок переводов не меньше
//! profit.transit_auto_after — среднее её последних n (как раньше); меньше, но у ПАРЫ площадок (все монеты, тот же
//! откуда → куда) записанных переводов не меньше n — среднее записанных переводов пары (тот же способ — среднее; у
//! каждой монеты в истории хранятся её последние n, времени перевода в записи нет, поэтому «последние n пары» не
//! выделить — берутся все записанные); иначе — значение из настроек. Источник числа — строкой в хронологию сделки.
//! Основание: Тигран 20.11: да (решение 464).
//! Ревью t238: сумма пары — готовая (Pairs, обновляется в transit.rs load / record), расчёт не проходит историю.
//! Решение 487 (T487-03, координатор): пара — в СЕТИ перевода (Asset.chain монеты: None — сеть по умолчанию, канонический
//! вид решения 430; Some — другая сеть): переводы BSC и Arbitrum одной пары площадок не смешиваются. Старые записи учёта
//! (до 416, без chain) читаются с chain = None — сеть по умолчанию (BSC), как ключи монет решения 416.
//! Кто вызывает: core/transit.rs (seconds_from, pair_update), core/executor.rs (note — строка хронологии).
//! От чего зависит: engine.rs (история inner.transits), core/types.
//! Настройки: нет своих (порог n — profit.transit_auto_after площадки-источника, читает transit.rs).

use std::collections::HashMap;

use rust_decimal::Decimal;

use crate::core::types::{ChainId, Direction, VenueId};

use super::engine::{Core, lk};

/// Откуда взято время в пути.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Режим manual: всегда из настроек (profit.transit_manual_s[A] + profit.transit_credit_s[B]).
    ManualMode,
    /// Режим manual_auto, статистики монеты и пары мало: из настроек.
    Manual,
    /// Статистика монеты по паре площадок: n последних переводов.
    Coin(usize),
    /// Статистика пары площадок по всем монетам: число записанных переводов, число монет.
    Pair(usize, usize),
}

/// Готовая сумма пары площадок: сумма секунд, число переводов, число монет с переводами.
#[derive(Debug, Default, Clone, Copy)]
struct Agg {
    sum: Decimal,
    count: usize,
    coins: usize,
}

/// Ключ суммы: (сеть перевода — Asset.chain, None — сеть по умолчанию; откуда).
type Key = (Option<ChainId>, VenueId);

/// Суммы пар площадок (поле inner.transit_pairs): (сеть, откуда) → [(куда, сумма)] — поиск по ссылке, без выделений.
/// Ведётся вместе с историей inner.transits (transit.rs load, record; порядок замков: transits, затем пары).
#[derive(Debug, Default)]
pub struct Pairs(std::sync::Mutex<HashMap<Key, Vec<(VenueId, Agg)>>>);

/// Что делает: ряд монеты пары площадок изменился — поправить сумму пары этой сети. Принимает: ядро, (сеть монеты —
/// Asset.chain, откуда, куда), (сумма, число) ряда до и после. Ошибок нет.
pub fn pair_update(
    core: &Core,
    (chain, from, to): (Option<&ChainId>, &VenueId, &VenueId),
    before: (Decimal, usize),
    after: (Decimal, usize),
) {
    let mut g = lk(&core.inner.transit_pairs.0);
    let list = g.entry((chain.cloned(), from.clone())).or_default();
    let i = match list.iter().position(|(x, _)| x == to) {
        Some(i) => i,
        None => {
            list.push((to.clone(), Agg::default()));
            list.len() - 1
        }
    };
    let a = &mut list[i].1;
    a.sum += after.0 - before.0;
    a.count = (a.count + after.1).saturating_sub(before.1);
    match (before.1 == 0, after.1 == 0) {
        (true, false) => a.coins += 1,
        (false, true) => a.coins = a.coins.saturating_sub(1),
        _ => {}
    }
}

/// Что делает: среднее записанных переводов пары площадок направления по всем монетам ТОЙ ЖЕ сети (Asset.chain
/// монеты направления), если их не меньше n (готовая сумма, без прохода по истории). Принимает: ядро, направление,
/// n (profit.transit_auto_after; 0 — статистики нет). Возвращает: (среднее, переводов, монет); None — меньше n. Ошибок нет.
pub fn pair_mean(core: &Core, d: &Direction, n: usize) -> Option<(Decimal, usize, usize)> {
    if n == 0 {
        return None;
    }
    let g = lk(&core.inner.transit_pairs.0);
    let list = g.get(&(d.asset.chain.clone(), d.buy_on.clone()))?;
    let (_, a) = list.iter().find(|(x, _)| x == &d.sell_on)?;
    if a.count < n || a.count == 0 {
        return None;
    }
    Some((a.sum / Decimal::from(a.count), a.count, a.coins))
}

/// Что делает: строка хронологии сделки «Время в пути …: N с — источник». Принимает: направление, секунды,
/// источник. Ошибок нет.
pub fn text(d: &Direction, secs: Decimal, src: &Source) -> String {
    let pair = format!("{} → {}", d.buy_on.0, d.sell_on.0);
    let why = match src {
        Source::ManualMode => format!(
            "режим manual: из настроек (profit.transit_manual_s[{}] + profit.transit_credit_s[{}])",
            d.buy_on.0, d.sell_on.0
        ),
        Source::Manual => format!(
            "из настроек (profit.transit_manual_s[{}] + profit.transit_credit_s[{}]): статистики монеты и пары мало",
            d.buy_on.0, d.sell_on.0
        ),
        Source::Coin(n) => format!(
            "статистика монеты {} по паре {pair}, {n} переводов",
            d.asset.ticker
        ),
        Source::Pair(n, c) => format!(
            "статистика пары {pair}{} по всем монетам, {n} переводов ({c} монет); у {} своих меньше",
            net(d),
            d.asset.ticker
        ),
    };
    format!(
        "Время в пути {pair}: {} с — {why} (решение 464)",
        secs.round_dp(1)
    )
}

/// Сеть перевода для текста: другая сеть — « (сеть id)», сеть по умолчанию — пусто (текст как до решения 487).
/// Ошибок нет.
fn net(d: &Direction) -> String {
    d.asset
        .chain
        .as_ref()
        .map_or_else(String::new, |c| format!(" (сеть {})", c.0))
}

/// Что делает: строка хронологии сделки о времени в пути ПЛАНА (ревью t238: T — из плана, не пересчёт): источник —
/// по текущей истории; если сейчас получается другое число (история изменилась после расчёта), так и сказано.
/// Принимает: ядро, направление, T плана, с. Возвращает: текст. Ошибок нет.
pub fn note(core: &Core, d: &Direction, plan_s: Decimal) -> String {
    let (a, b, t) = (&d.buy_on.0, &d.sell_on.0, plan_s.round_dp(1));
    match super::transit::seconds_from(core, d) {
        Ok((s, src)) if s == plan_s => text(d, plan_s, &src),
        // История переводов изменилась после расчёта плана — число плана и то, что получается сейчас.
        Ok((s, src)) => {
            let now = text(d, s, &src);
            format!("Время в пути {a} → {b}: {t} с по плану; сейчас: {now}")
        }
        Err(e) => format!("Время в пути {a} → {b}: {t} с по плану; источник: {e}"),
    }
}
