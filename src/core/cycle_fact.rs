//! Файл: core/cycle_fact.rs
//! Что делает: учёт факта сделки цикла DEX ↔ DEX (вынесено из cycle_deal.rs — размер файла, решение 257): исполненный
//! цикл — вложено, выручка, газ, approve и чаевые курсами адаптера, Closed; газ транзакции ошибки (и отмены, чаевые,
//! approve) — в затраты. Затраты — статьями по виду (deal_book.rs, решение 257).
//! Кто вызывает: core/cycle_deal.rs.
//! От чего зависит: deal.rs, deal_book.rs, deal_gas.rs (of_filled), gas_stats.rs, ledger.rs, convert.rs, core/types,
//! gateway/errors.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::{CostKind, CycleTradeResult};
use crate::gateway::errors::GatewayError;

use super::deal::{Deal, Step};
use super::engine::Core;

/// Цикл исполнен: учёт по факту (вложено, выручка, газ — курсами адаптера), сделка Closed. Принимает: ядро, сделку,
/// итог. Ошибок нет.
pub(super) async fn filled(core: &Core, deal: &mut Deal, r: CycleTradeResult) {
    if let Some(tx) = &r.trade.chain {
        super::gas_stats::record_tx(core, &deal.direction.asset, tx).await;
    }
    deal.invested = r.input * r.base_usdt;
    deal.proceeds = r.output * r.base_usdt;
    // Решение 219: комиссия — список; у цикла DEX — газ в BNB.
    let gas = r.trade.fee_in(super::convert::GAS_TICKER);
    // Решение 236, дополнение: approve перед циклом, чаевые канала и газ их перевода (п. 6; факт транзакции) — тем же
    // курсом BNB адаптера.
    let extra_list = super::deal_gas::of_filled(&r.trade);
    let extra: Decimal = extra_list.iter().map(|w| w.bnb).sum();
    // Решение 257: статьи затрат по виду (газ цикла, approve, чаевые) — тем же курсом.
    super::deal_book::spend(deal, CostKind::Gas, gas * r.bnb_usdt);
    for w in &extra_list {
        super::deal_book::spend(deal, w.kind, w.bnb * r.bnb_usdt);
    }
    let by = r.trade.executor.clone().unwrap_or_default();
    let extra_t = if extra.is_zero() {
        String::new()
    } else {
        format!(", approve, чаевые канала и газ их перевода {extra} BNB")
    };
    deal.note(format!(
        "Цикл исполнен: вход {} → выход {} {} (газ {} BNB{extra_t}); исполнил: {by}",
        r.input, r.output, deal.direction.asset.ticker, gas
    ));
    deal.buy = Some(r.trade);
    super::ledger::finish(core, deal, Step::Closed, "цикл DEX ↔ DEX исполнен").await;
}

/// Газ транзакции ошибки (и отмены, занявшей nonce) — в затраты по курсу BNB плана и в статистику газа. Решение 236,
/// дополнение: и чаевые канала с газом их перевода (факт транзакции, п. 6), и газ approve до ошибки (GatewayError::approve_gas_bnb). Ошибок нет.
pub(super) async fn spend(core: &Core, deal: &mut Deal, bnb_usdt: Decimal, e: &GatewayError) {
    if let Some(tx) = &e.tx {
        super::gas_stats::record_tx(core, &deal.direction.asset, tx).await;
        let gas = tx.gas_bnb.unwrap_or(Decimal::ZERO) + tx.cancel_gas_bnb.unwrap_or(Decimal::ZERO);
        super::deal_book::spend(deal, CostKind::Gas, gas * bnb_usdt);
        deal.note(format!("Газ транзакции цикла: {gas} BNB"));
        if let Some(t) = tx.tip_bnb.filter(|t| !t.is_zero()) {
            super::deal_book::spend(deal, CostKind::ChannelTip, t * bnb_usdt);
            deal.note(format!("Чаевые канала цикла: {t} BNB"));
        }
        // Решение 236, дополнение п. 6: газ отдельной транзакции перевода чаевых (её квитанция, адаптер).
        if let Some(t) = tx.tip_gas_bnb.filter(|t| !t.is_zero()) {
            super::deal_book::spend(deal, CostKind::ChannelTip, t * bnb_usdt);
            deal.note(format!("Газ перевода чаевых цикла: {t} BNB"));
        }
    }
    if let Some(a) = e.approve_gas().filter(|a| !a.is_zero()) {
        super::deal_book::spend(deal, CostKind::ApproveGas, a * bnb_usdt);
        deal.note(format!("Газ approve перед циклом: {a} BNB"));
    }
}
