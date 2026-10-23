//! Файл: core/partial.rs
//! Что делает: частичная покупка (решение 48) — обычным полным расчётом со всеми затратами сравниваются
//! два итога сделки: «довезти и продать, как задумано» (costs.rs на фактический объём, с фактической
//! ценой покупки вместо котировки) и «продать обратно на площадке покупки» (trade_ops::sell_value).
//! Выбирается больший итог (оба в минусе — меньший убыток). Режим застрявшей монеты здесь не включается.
//! Кто вызывает: core/executor.rs.
//! От чего зависит: dirctx.rs, costs.rs, trade_ops.rs, deal.rs.
//! Настройки: через costs.rs и dirctx.rs.
//!
//! Осторожность расчёта: затраты покупки в монете из котировки уже вычтены из факта (deal.qty),
//! а расчёт «довезти» вычитает их ещё раз — итог «довезти» слегка занижен, но не завышен.

use rust_decimal::Decimal;

use super::costs::Calc;
use super::deal::Deal;
use super::engine::Core;

/// Что делать с купленным частично.
#[derive(Debug)]
pub enum Choice {
    /// Довезти до B и продать — ожидаемый итог сделки, USDT.
    Deliver(Decimal),
    /// Продать на A — ожидаемый итог сделки, USDT.
    SellBack(Decimal),
    /// Ни один путь не считается — причины обоих.
    Neither(String),
}

/// Итог «довезти»: выручка расчёта минус затраты расчёта без затрат покупки (их заменяет факт),
/// минус фактически вложенное и потраченное. Принимает: расчёт, сделку.
fn deliver_result(c: &Calc, deal: &Deal) -> Decimal {
    let buy_costs: Decimal = c
        .costs
        .iter()
        .filter(|l| l.stage == "A" && !l.in_price && !l.in_qty)
        .map(|l| l.usdt)
        .sum();
    c.proceeds - (c.costs_usdt - buy_costs) - deal.invested - deal.spent
}

/// Выбор пути. Принимает: ядро, сделку после частичной покупки. Возвращает: Choice. Ошибок нет.
pub async fn choose(core: &Core, deal: &Deal) -> Choice {
    let deliver = match super::dirctx::DirCtx::build(core, &deal.direction) {
        Ok(cx) => super::costs::evaluate(&core.inner.deps, &cx, deal.qty)
            .await
            .map(|c| deliver_result(&c, deal)),
        Err(e) => Err(e),
    };
    let back = super::trade_ops::sell_value(
        core,
        &deal.direction.buy_on,
        &deal.direction.asset,
        deal.qty,
    )
    .await
    .map(|(v, _)| deal.result_if(v));
    match (deliver, back) {
        (Ok(d), Ok(b)) if d >= b => Choice::Deliver(d),
        (Ok(_), Ok(b)) => Choice::SellBack(b),
        (Ok(d), Err(_)) => Choice::Deliver(d),
        (Err(_), Ok(b)) => Choice::SellBack(b),
        (Err(d), Err(b)) => Choice::Neither(format!("довезти: {d}; продать на A: {b}")),
    }
}
