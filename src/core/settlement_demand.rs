//! Файл: core/settlement_demand.rs.
//! Что делает: перепроверяет спрос и цены основной сделки перед выдачей средств.
//! Кто вызывает: исполнитель поручения.
//! От чего зависит: котировки покупателя и продавца.
//! Настройки: глубина и границы сохранённого плана.
use super::{engine::Core, settlement_state::Plan, types::Side};
use rust_decimal::Decimal;
/// Свежая доступность обеих сторон монеты до финансирования, с ценой спроса в валюте учёта исходной биржи.
pub(super) async fn demand(
    core: &Core,
    plan: &Plan,
    target: Decimal,
    sell_qty: Decimal,
    bid: Decimal,
    required: Decimal,
) -> Result<(), String> {
    use super::costs_quote::{fetch, worst};
    let d = &plan.direction;
    let quotes = async {
        tokio::try_join!(
            fetch(core, d, false, target, None),
            fetch(core, d, true, sell_qty, None)
        )
    };
    let (a, b) = crate::gateway::quote_stage::settlement_if(
        true,
        crate::gateway::quote_stage::final_scope(quotes),
    )
    .await
    .map_err(|e| e.to_string())?;
    if !crate::gateway::quote_stage::settlement_quote_supported(&a, true) {
        return Err("Финансирование требует исполнимого маршрута целевой покупки".into());
    }
    if target <= Decimal::ZERO
        || sell_qty <= Decimal::ZERO
        || bid <= Decimal::ZERO
        || required <= Decimal::ZERO
        || a.fillable_qty < target
        || b.fillable_qty < sell_qty
    {
        return Err("Нет глубины обеих сторон основной сделки конверсии".into());
    }
    let ask = worst(
        core,
        &d.buy_on,
        &d.asset,
        Side::Buy,
        target,
        a.avg_price,
        None,
    )
    .map_err(|e| e.to_string())?;
    let sale = worst(
        core,
        &d.sell_on,
        &d.asset,
        Side::Sell,
        sell_qty,
        b.avg_price,
        None,
    )
    .map_err(|e| e.to_string())?;
    if sale < bid || target.checked_mul(ask).is_none_or(|x| x > required) {
        return Err(
            "Основная сделка конверсии вышла за подтверждённые границы спроса или средств".into(),
        );
    }
    Ok(())
}
