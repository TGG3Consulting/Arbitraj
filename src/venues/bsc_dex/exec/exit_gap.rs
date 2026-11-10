//! Файл: venues/bsc_dex/exec/exit_gap.rs
//! Что делает: решение 248 (кейс SMARS 10.11) — замер реального выхода продажи выхода после отката «мало на выходе»
//! (исполнитель — exec/executor/remeasure.rs, Universal Router — exec/ur_remeasure.rs) ядру: MarketEvent::ExitGap с
//! выходом модели (сумма ожидаемых долей), границей по замеру и модельным налогом монеты. Решение (закрыть монету
//! для входа или нет) — у ядра (core/exit_gap.rs, guard.exit_gap_pct).
//! Кто вызывает: exec/executor/remeasure.rs, exec/ur_remeasure.rs.
//! От чего зависит: send/stats.rs (канал событий ядру), gateway/events.rs, ctx.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{ExitGap, TradeRequest};
use crate::gateway::events::MarketEvent;

use super::super::ctx::Ctx;
use super::super::send::stats::Stats;

/// Шлёт ядру замер выхода. Принимает: окружение, канал, запрос, по долям (ожидаемый выход, модельный налог %),
/// границу по замеру (USDT). Ошибок нет (нет канала — ничего).
pub fn report(
    ctx: &Ctx,
    stats: &Stats,
    req: &TradeRequest,
    exp: &[(Decimal, Decimal)],
    real: Decimal,
) {
    let model: Decimal = exp.iter().map(|(e, _)| *e).sum();
    let tax = exp
        .iter()
        .map(|(_, t)| *t)
        .fold(Decimal::ZERO, Decimal::max);
    stats.emit(MarketEvent::ExitGap(ExitGap {
        venue: ctx.venue(),
        asset: req.asset.clone(),
        deal_id: req.deal_id.clone(),
        model_usdt: model,
        real_usdt: real,
        model_tax_pct: tax,
    }));
}
