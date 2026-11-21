//! Файл: venues/bsc_dex/best_of/mod.rs
//! Что делает: режим «лучший из двух» для DEX-стороны сделки (решение 123, слова Тиграна: «мы спрашиваем КС всегда, но
//! если наш маршрут лучше то делаем посвоему»). Финальная котировка и сделка спрашивают и KyberSwap, и свой сборщик;
//! исполняет лучший для стороны (газ учтён у обоих). Выбор — чистая функция choose.rs; затраты вне цены — extra.rs;
//! финальная котировка — quote.rs; сделка — trade.rs.
//! Кто вызывает: venue.rs (quote, trade).
//! От чего зависит: choose.rs, extra.rs, quote.rs, trade.rs, ctx.rs.
//! Настройки: exec.dex_best_of [bsc_dex] (да/нет, необязательная) — только вместе с exec.via_kyber = да.

pub mod choose;
pub mod extra;
pub mod quote;
pub mod trade;

use super::ctx::Ctx;

/// Включён ли режим: exec.via_kyber = да и exec.dex_best_of = да. Не задано или ошибка чтения — нет (как без режима).
/// Ошибок нет.
pub fn enabled(ctx: &Ctx) -> bool {
    ctx.flag("exec.via_kyber").unwrap_or(false) && ctx.flag("exec.dex_best_of").unwrap_or(false)
}
