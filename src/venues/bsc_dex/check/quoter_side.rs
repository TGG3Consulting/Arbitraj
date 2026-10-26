//! Файл: venues/bsc_dex/check/quoter_side.rs
//! Что делает: сверка с квотером по сторонам (решение 73) — части цикла check/quoter_cycle.rs: когда сторону без итога
//! пора сверить по сигналу «пул готов» (side_ready: она стала глубокой — не ждать полного прохода dex.quoter_check_s),
//! сменился ли итог стороны так, что стоит записать (changed), запись о смене (log_change).
//! Кто вызывает: check/quoter_cycle.rs.
//! От чего зависит: quote/select.rs (deep_side), check/state.rs, pools/listen.rs (Pools), pools/progress.rs (reason_key).
//! Настройки: через quote/select.rs — dex.min_pool_liquidity_usd, dex.depth_band_pct, dex.depth_hysteresis_pct.

use crate::core::types::{PoolRef, Side};
use crate::storage::journal::Level;

use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;
use super::super::pools::registry::Entry;
use super::super::quote::select;

/// Начало причины «не задана настройка» — такие причины пишутся одной строкой на причину (Signals::once).
const NOT_SET: &str = "Не задана настройка";

/// У пула есть сторона без итога сверки, которая сейчас глубокая (глубина — из кэша, pools/depth_book.rs): её пора
/// сверить, не дожидаясь полного прохода (решение 73). Знаки USDT ещё не известны — false. Ошибок нет.
pub fn side_ready(p: &Pools, r: &PoolRef) -> bool {
    let Some(e) = p.reg.get(r) else {
        return false;
    };
    let Some(dec) = p
        .tokens
        .usdt(&p.ctx)
        .ok()
        .and_then(|u| p.tokens.decimals_known(u))
    else {
        return false;
    };
    let q = p.checks.pool(r).quoter;
    [Side::Buy, Side::Sell].into_iter().any(|s| {
        q.side(s).is_none()
            && matches!(
                select::deep_side(&p.ctx, &p.reg, &p.tokens, &e, dec, select::Need::Side(s)),
                Ok(None)
            )
    })
}

/// Запись о смене итога стороны пула. Ошибок нет.
pub async fn log_change(p: &Pools, e: &Entry, side: Side, res: &Result<(), String>) {
    let what = if side == Side::Buy {
        "покупка"
    } else {
        "продажа"
    };
    let msg = match res {
        Ok(()) => format!(
            "{} {}: сверка с квотером ({what}) снова проходит — сторона в котировках",
            e.family.title, e.ticker
        ),
        Err(why) => format!(
            "{} {} ({what}): {why} — сторона исключена",
            e.family.title, e.ticker
        ),
    };
    let level = if res.is_ok() {
        Level::Info
    } else {
        Level::Warn
    };
    p.ctx.log(level, "dex.quoter", msg, None).await;
}

/// Сменилось ли состояние пула так, что это стоит записать (см. шапку). Ошибок нет.
pub fn changed(prev: Option<&Result<(), String>>, new: &Result<(), String>) -> bool {
    match (prev, new) {
        (_, Err(why)) if why.starts_with(NOT_SET) => false,
        (None, Ok(())) | (Some(Ok(())), Ok(())) => false,
        (Some(Err(a)), Err(b)) => reason_key(a) != reason_key(b),
        _ => true,
    }
}
