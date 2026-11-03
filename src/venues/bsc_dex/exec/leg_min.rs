//! Файл: venues/bsc_dex/exec/leg_min.rs
//! Что делает: свежий минимум выхода доли из предторговой верификации (решения 83, 84) — последнему шагу доли, НЕ НИЖЕ
//! уже стоящего минимума сделки (exec/order.rs: наибольшее из «по req.worst_price» и «ожидаемое × (1 − допуск)»).
//! Так граница сделки ядра (худшая цена) доходит до транзакции и при свежем минимуме (ревью 122, пункт 2): бандл,
//! прямой и умный путь, исполнитель (сумма минимумов долей — minOut execute) и сухая сделка берут минимум отсюда.
//! Кто вызывает: exec/{trade_direct, smart_legs, bundle_trade, dry_build}.rs, exec/executor/trade.rs
//! (через verify_gate::apply_leg_min).
//! От чего зависит: exec/chain.rs (Step), num.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::chain::Step;

/// Свежий минимум выхода доли (решение 84) последнему шагу — он отдаёт выход доли кошельку. Минимум = наибольшее из
/// свежего и уже стоящего (exec/order.rs: по req.worst_price и по допуску) — граница сделки ядра не теряется (ревью
/// 122). Принимает: шаги доли, минимум в выходном активе, decimals выхода. Ошибки: NoRoute (доля без шагов),
/// BadResponse (число), QuoteMismatch (минимум 0 — обмен без минимума не отправляется).
pub fn apply_leg_min(
    ctx: &Ctx,
    steps: &mut [Step],
    min: Decimal,
    out_dec: u8,
) -> GatewayResult<()> {
    let last = steps
        .last_mut()
        .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "доля маршрута без шагов"))?;
    let units = num::to_units(min, out_dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::BadResponse,
            "свежий минимум выхода не переводится в единицы токена",
        )
    })?;
    if units.is_zero() {
        return Err(ctx.err(
            ErrorCode::QuoteMismatch,
            "свежий минимум выхода 0 — обмен без минимума не отправляется",
        ));
    }
    last.order.min_out = last.order.min_out.max(units);
    Ok(())
}
