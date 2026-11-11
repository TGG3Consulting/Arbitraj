//! Файл: core/deal_gas.rs
//! Что делает: решение 236, дополнение (техлид 09.11 по поручению Тиграна: «итог каждой сделки точный и честный, со
//! ВСЕМИ затратами») — газ, который раньше не попадал в итог сделки, идёт в её затраты (spent) один раз:
//! - газ разрешений (approve) перед обменом DEX (ChainTx::approve_gas_bnb у успеха, GatewayError::approve_gas_bnb у
//!   ошибки — ставит адаптер, venues/bsc_dex/exec/swap_send.rs, venues/kyberswap/trade.rs);
//! - чаевые канала отправки (ChainTx::tip_bnb — адаптер ставит их, только когда обмен в блоке или откатился в блоке)
//!   и газ отдельной транзакции их перевода (ChainTx::tip_gas_bnb — по её квитанции; решение 236, дополнение п. 6);
//! - газ откатившегося / отменённого / неисполненного обмена (газ транзакции, газ отмены, занявшей nonce) — раньше
//!   только статистика газа монеты (gas_stats.rs);
//! - газ неудачного вывода из кошелька DEX (откат перевода, отмена, занявшая nonce) — раньше нигде (transfers.rs).
//!
//! BNB → USDT — живой ценой BNB, как остальной газ (fills::usdt → convert.rs); нет цены — в итог не входит, строка в
//! хронологию и журнал (как fills.rs). Строка хронологии — что и сколько.
//! Открытая сделка (у вызывающего своя копия) — charge; закрытая (возврат USDT после закрытия) — charge_closed: итог
//! сделки, итог дня (daily::settle с −затратой), итог монеты (gas_stats), журнал — как deal_return_cost::after.
//! Кто вызывает: trade_ops.rs (собирает), transfers.rs (собирает), executor.rs, sell.rs, relay.rs, manual_relay.rs
//! (charge), usdt_relay_retry.rs (charge_closed), cycle_deal.rs (лист approve и чаевых цикла).
//! От чего зависит: deal.rs, fills.rs (usdt), convert.rs (GAS_TICKER), ledger.rs, daily.rs, daily_commit.rs,
//! gas_stats.rs, core/types (ChainTx, TradeResult, Cost), deal_book.rs (статьи затрат и начисления дня, решение 257).
//! Настройки: нет (venue.quote_asset — через fills::usdt).

use rust_decimal::Decimal;

use crate::core::types::{ChainTx, Cost, CostKind, TradeResult, TxOutcome, VenueId};
use crate::gateway::errors::GatewayError;
use crate::storage::journal::Level;

use super::deal::Deal;
use super::engine::{Core, lk};

/// Одна статья газа вне итога обмена / вывода: что это и сколько BNB.
#[derive(Debug, Clone, PartialEq)]
pub struct Wasted {
    /// Для хронологии: «approve», «чаевые канала», «откат обмена», «отмена обмена» и т. п.
    pub what: String,
    /// Вид затраты (Gas, ApproveGas, ChannelTip, TransferGas) — все платятся из резерва BNB (convert::is_gas).
    pub kind: CostKind,
    pub bnb: Decimal,
}

/// Что делает: статья, если сумма есть и не ноль. Принимает: что, вид, сумму. Возвращает: 0 или 1 статью. Ошибок нет.
fn item(what: &str, kind: CostKind, bnb: Option<Decimal>) -> Option<Wasted> {
    bnb.filter(|b| !b.is_zero()).map(|bnb| Wasted {
        what: what.to_string(),
        kind,
        bnb,
    })
}

/// Что делает: газ сверх газа самого обмена у ИСПОЛНЕННОГО обмена (газ обмена уже в TradeResult.fees → fills.rs):
/// approve, чаевые канала и газ транзакции их перевода. Принимает: итог. Возвращает: статьи. Ошибок нет.
pub fn of_filled(r: &TradeResult) -> Vec<Wasted> {
    let Some(c) = &r.chain else {
        return vec![];
    };
    [
        item("approve", CostKind::ApproveGas, c.approve_gas_bnb),
        item("чаевые канала", CostKind::ChannelTip, c.tip_bnb),
        item(TIP_GAS, CostKind::ChannelTip, c.tip_gas_bnb),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Статья «газ перевода чаевых» (решение 236, дополнение п. 6): чаевые идут отдельной транзакцией (nonce + 1 в том же
/// бандле), её газ в квитанции обмена не входит.
const TIP_GAS: &str = "газ перевода чаевых";

/// Что делает: весь газ транзакции, которая не дала исполнения (откат, отмена, занявшая nonce, обмен в блоке без
/// монет): газ самой транзакции, газ отмены, чаевые и газ их перевода, approve перед ней. Принимает: факт транзакции, что за операция
/// («обмена» / «перевода»), вид газа транзакции (Gas / TransferGas). Возвращает: статьи. Ошибок нет.
pub fn of_failed_tx(tx: &ChainTx, op: &str, kind: CostKind) -> Vec<Wasted> {
    let own = match tx.outcome {
        TxOutcome::Reverted => format!("откат {op}"),
        _ => format!("{op} без исполнения"),
    };
    [
        item(&own, kind, tx.gas_bnb),
        item(&format!("отмена {op}"), kind, tx.cancel_gas_bnb),
        item("чаевые канала", CostKind::ChannelTip, tx.tip_bnb),
        item(TIP_GAS, CostKind::ChannelTip, tx.tip_gas_bnb),
        item("approve", CostKind::ApproveGas, tx.approve_gas_bnb),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Что делает: газ разрешений, отправленных до ошибки обмена (GatewayError::approve_gas_bnb). Ошибок нет.
pub fn of_err_approve(e: &GatewayError) -> Vec<Wasted> {
    item("approve", CostKind::ApproveGas, e.approve_gas())
        .into_iter()
        .collect()
}

/// Что делает: сумма статей в USDT живой ценой BNB (fills::usdt; монета сделки не участвует — у сделки с BNB газ тоже по
/// живой цене BNB). Принимает: ядро, площадку (котируемая монета venue.quote_asset), статьи. Возвращает: (BNB, USDT).
/// Ошибки: текст — нет живой цены BNB или не задана venue.quote_asset.
async fn usd(core: &Core, venue: &VenueId, list: &[Wasted]) -> (Decimal, Result<Decimal, String>) {
    let bnb: Decimal = list.iter().map(|w| w.bnb).sum();
    let c = Cost::new(CostKind::Gas, bnb, super::convert::GAS_TICKER, false);
    (
        bnb,
        super::fills::usdt(core, venue, "", Decimal::ONE, &c).await,
    )
}

/// Что делает: строка хронологии «Газ вне обмена в затратах сделки: approve 0.0001 BNB, … = 0.06 USDT». Ошибок нет.
fn line(list: &[Wasted], bnb: Decimal, usd: &Result<Decimal, String>) -> String {
    let parts: Vec<String> = list
        .iter()
        .map(|w| format!("{} {} BNB", w.what, w.bnb))
        .collect();
    let tail = match usd {
        Ok(u) => format!("= {} USDT — в затратах сделки", u.round_dp(6)),
        Err(_) => {
            format!("= {bnb} BNB — не пересчитан в USDT (нет живой цены BNB), итог сделки без него")
        }
    };
    format!("Газ вне итога обмена: {} {tail}", parts.join(", "))
}

/// Что делает: вносит статьи в затраты открытой сделки (spent) и строку в хронологию; нет цены BNB — журнал
/// core.ledger. Статьи забираются (один раз). Принимает: ядро, сделку (копию вызывающего), площадку, статьи. Ошибок нет.
pub async fn charge(core: &Core, deal: &mut Deal, venue: &VenueId, list: &mut Vec<Wasted>) {
    let list = std::mem::take(list);
    if list.is_empty() {
        return;
    }
    let (bnb, u) = usd(core, venue, &list).await;
    deal.note(line(&list, bnb, &u));
    match u {
        // Решение 257: статьи затрат — по виду каждой (доля BNB).
        Ok(u) => super::deal_book::spend_bnb(deal, &items(&list), u),
        Err(e) => warn(core, venue, &deal.id, bnb, &e).await,
    }
}

/// Что делает: как charge, но у сделки в памяти ядра (закрытой — возврат USDT идёт после закрытия): затраты, итог
/// (result_usd − USDT), сохранение; итог дня (daily::settle с −USDT), итог монеты (gas_stats), журнал core.ledger.
/// Сделки нет (перегон вне сделки) — только журнал. Принимает: ядро, id, площадку, статьи. Ошибок нет.
pub async fn charge_closed(core: &Core, id: &str, venue: &VenueId, list: &mut Vec<Wasted>) {
    let list = std::mem::take(list);
    if list.is_empty() {
        return;
    }
    let (bnb, u) = usd(core, venue, &list).await;
    let text = line(&list, bnb, &u);
    let found = {
        let mut deals = lk(&core.inner.deals);
        deals.get_mut(id).map(|d| {
            d.note(text.clone());
            if let Ok(u) = &u {
                super::deal_book::spend_bnb(d, &items(&list), *u);
                if let Some(r) = d.result_usd.as_mut() {
                    *r -= *u;
                }
            }
            d.clone()
        })
    };
    let Some(d) = found else {
        let t = format!("{id}: {text} (сделки нет — перегон вне сделки)");
        log(core, Level::Info, venue, t).await;
        return;
    };
    super::ledger::save(core, &d).await;
    match u {
        Ok(u) if d.result_usd.is_some() && !u.is_zero() => {
            let mut x = d.clone();
            let before = x.day_book.len();
            let trip = super::daily::settle(core, &mut x, -u).await;
            super::deal_book::keep_day(core, &x, before).await;
            super::gas_stats::record_result(core, &d.direction.asset, -u).await;
            let t = format!(
                "Сделка {id}: газ {} BNB = {} USDT вычтен из итога, итог {} USDT",
                bnb,
                u.round_dp(6),
                d.result_usd.unwrap_or_default().round_dp(4)
            );
            log(core, Level::Info, venue, t).await;
            super::daily_commit::announce(core, &trip).await;
        }
        Ok(_) => {}
        Err(e) => warn(core, venue, id, bnb, &e).await,
    }
}

/// Статьи для учёта затрат по виду (решение 257): вид и BNB. Ошибок нет.
fn items(list: &[Wasted]) -> Vec<(CostKind, Decimal)> {
    list.iter().map(|w| (w.kind, w.bnb)).collect()
}

/// Журнал «газ не пересчитан в USDT». Ошибок нет.
async fn warn(core: &Core, venue: &VenueId, id: &str, bnb: Decimal, e: &str) {
    let t = format!("{id}: газ {bnb} BNB не переведён в USDT ({e}) — итог сделки без него");
    log(core, Level::Warn, venue, t).await;
}

/// Запись в журнал core.ledger. Ошибок нет.
async fn log(core: &Core, level: Level, venue: &VenueId, text: String) {
    core.inner
        .deps
        .log(level, "core.ledger", Some(&venue.0), text, None)
        .await;
}
