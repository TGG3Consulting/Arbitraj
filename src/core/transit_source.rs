//! Файл: core/transit_source.rs
//! Что делает: решение 464 (t238, живой NUM 20.11: Gate → DEX — план 39 с из настроек 37 + 2, факт 58 с; по паре
//! gate → bsc_dex в истории 123 перевода других монет, среднее 49,1 с) — откуда взято время в пути T в режиме
//! «вручную + авто» (manual_auto, transit.rs): у монеты по паре площадок переводов не меньше
//! profit.transit_auto_after — среднее её последних n (как раньше); меньше, но у ПАРЫ площадок (все монеты, тот же
//! откуда → куда) записанных переводов не меньше n — среднее записанных переводов пары (тот же способ — среднее; у
//! каждой монеты в истории хранятся её последние n, времени перевода в записи нет, поэтому «последние n пары» не
//! выделить — берутся все записанные); иначе — значение из настроек. Источник числа — строкой в хронологию сделки.
//! Кто вызывает: core/transit.rs (seconds_from), core/executor.rs (note — строка хронологии).
//! От чего зависит: engine.rs (история inner.transits), core/types.
//! Настройки: нет своих (порог n — profit.transit_auto_after площадки-источника, читает transit.rs).

use rust_decimal::Decimal;

use crate::core::types::Direction;

use super::engine::{Core, lk};

/// Откуда взято время в пути.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Настройки: profit.transit_manual_s[A] + profit.transit_credit_s[B].
    Manual,
    /// Статистика монеты по паре площадок: n последних переводов.
    Coin(usize),
    /// Статистика пары площадок по всем монетам: число записанных переводов, число монет.
    Pair(usize, usize),
}

/// Что делает: среднее записанных переводов пары площадок направления по всем монетам, если их не меньше n. Принимает:
/// ядро, направление, n (profit.transit_auto_after; 0 — статистики нет). Возвращает: (среднее, переводов, монет);
/// None — меньше n. Ошибок нет.
pub fn pair_mean(core: &Core, d: &Direction, n: usize) -> Option<(Decimal, usize, usize)> {
    if n == 0 {
        return None;
    }
    let t = lk(&core.inner.transits);
    let (mut sum, mut count, mut coins) = (Decimal::ZERO, 0usize, 0usize);
    for ((_, from, to), v) in t.iter() {
        if from == &d.buy_on && to == &d.sell_on && !v.is_empty() {
            sum += v.iter().copied().sum::<Decimal>();
            count += v.len();
            coins += 1;
        }
    }
    (count >= n).then(|| (sum / Decimal::from(count), count, coins))
}

/// Что делает: строка хронологии сделки «Время в пути …: N с — источник». Принимает: направление, секунды,
/// источник. Ошибок нет.
pub fn text(d: &Direction, secs: Decimal, src: &Source) -> String {
    let pair = format!("{} → {}", d.buy_on.0, d.sell_on.0);
    let why = match src {
        Source::Manual => format!(
            "из настроек (profit.transit_manual_s[{}] + profit.transit_credit_s[{}]): статистики монеты и пары мало",
            d.buy_on.0, d.sell_on.0
        ),
        Source::Coin(n) => format!(
            "статистика монеты {} по паре {pair}, {n} переводов",
            d.asset.ticker
        ),
        Source::Pair(n, c) => format!(
            "статистика пары {pair} по всем монетам, {n} переводов ({c} монет); у {} своих меньше",
            d.asset.ticker
        ),
    };
    format!(
        "Время в пути {pair}: {} с — {why} (решение 464)",
        secs.round_dp(1)
    )
}

/// Что делает: строка источника времени в пути направления сейчас — для хронологии сделки (executor.rs). Принимает:
/// ядро, направление. Возвращает: текст; None — время не посчитано (ошибка настроек — её уже вернул расчёт). Ошибок
/// нет.
pub fn note(core: &Core, d: &Direction) -> Option<String> {
    let (secs, src) = super::transit::seconds_from(core, d).ok()?;
    Some(text(d, secs, &src))
}
