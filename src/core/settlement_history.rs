//! Устанавливает единицу конкретной операции CEX по торговому исполнению, отчёту
//! площадки и квитанции перевода. Вывод и депозит подтверждаются по отдельности.
use rust_decimal::Decimal;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use crate::core::deal::Deal;
use crate::core::engine::Core;
use crate::core::types::{Asset, CostKind, DexAccountId, TransferStatus, VenueId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Basis {
    Raw,
    Display,
    Conflict,
}

pub(super) type Modes = HashMap<(VenueId, bool), Basis>;

#[cfg(test)]
#[path = "settlement_history_tests.rs"]
mod tests;

#[derive(Clone, PartialEq)]
pub(super) struct Observation {
    cex: VenueId,
    dex: VenueId,
    wallet: DexAccountId,
    /// true — вывод с CEX и приход в кошелёк.
    withdrawal: bool,
    hash: String,
    raw: Decimal,
    exchange: Decimal,
}

/// Используем полное прямое исполнение. Возврат USDT, перегон и записи с одним
/// только снимком баланса не доказывают, сколько монет биржа зачислила по хешу.
pub(super) fn observed(d: &Deal, dex: &VenueId) -> Option<Observation> {
    let a = &d.direction.asset;
    let withdrawal = d.direction.sell_on == *dex;
    if a.contract.is_none() || d.relays != 0 || (!withdrawal && d.direction.buy_on != *dex) {
        return None;
    }
    let buy = d.buy.as_ref()?;
    if buy.request.asset != *a
        || buy.request.venue != d.direction.buy_on
        || buy.filled_qty <= Decimal::ZERO
    {
        return None;
    }
    let s = d
        .transfers
        .iter()
        .find(|t| t.status == TransferStatus::Sent && t.venue == d.direction.buy_on)?;
    let tx = s.tx_hash.as_ref()?;
    if tx.is_empty() || s.venue_ref.is_empty() {
        return None;
    }
    let mut credits = d.transfers.iter().filter(|t| {
        t.status == TransferStatus::Credited
            && t.venue == d.direction.sell_on
            && t.tx_hash
                .as_ref()
                .is_some_and(|h| h.eq_ignore_ascii_case(tx))
    });
    let r = credits.next()?;
    if credits.next().is_some() || r.amount <= Decimal::ZERO {
        return None;
    }
    let (cex, raw, exchange) = if withdrawal {
        if r.asset != *a {
            return None;
        }
        let fee = s.fee.as_ref()?;
        if fee.kind != CostKind::WithdrawFee
            || fee.in_price
            || fee.paid_in.is_some()
            || fee.asset != s.asset.ticker
        {
            return None;
        }
        let buy_fee: Decimal = buy
            .fees
            .iter()
            .filter(|f| f.asset == a.ticker || f.asset == s.asset.ticker)
            .map(|f| f.amount)
            .sum();
        let held = buy.filled_qty.checked_sub(buy_fee)?;
        if held.trunc_with_scale(s.amount.scale()) != s.amount {
            return None;
        }
        (&s.venue, r.amount, s.amount.checked_sub(fee.amount)?)
    } else {
        if r.venue_ref.is_empty()
            || s.asset != *a
            || d.sells.is_empty()
            || d.sells
                .iter()
                .any(|f| f.request.venue != r.venue || f.request.asset != *a)
        {
            return None;
        }
        let sold: Decimal = d.sells.iter().map(|f| f.filled_qty).sum();
        if sold != r.amount {
            return None;
        }
        (&r.venue, s.amount, r.amount)
    };
    if raw <= Decimal::ZERO || exchange <= Decimal::ZERO {
        return None;
    }
    Some(Observation {
        cex: cex.clone(),
        dex: dex.clone(),
        wallet: d.dex_account.clone()?,
        withdrawal,
        hash: tx.to_ascii_lowercase(),
        raw,
        exchange,
    })
}

type ReceiptKey = (VenueId, DexAccountId, bool, String, Decimal);
pub(super) type Receipts = Mutex<HashMap<ReceiptKey, (Decimal, Decimal)>>;

pub(super) async fn verified(
    core: &Core,
    a: &Asset,
    scale: Decimal,
    rows: &[Observation],
    receipts: &Receipts,
) -> (Modes, bool) {
    let mut modes = Modes::new();
    let mut unknown = HashSet::new();
    for row in rows {
        let key = (
            row.dex.clone(),
            row.wallet.clone(),
            row.withdrawal,
            row.hash.clone(),
            row.raw,
        );
        let stored = super::engine::lk(receipts).get(&key).copied();
        let proof = if let Some(proof) = stored {
            Some(proof)
        } else {
            let read = async {
                let dex = core
                    .inner
                    .deps
                    .gw
                    .get_as(&row.dex, Some(&row.wallet))
                    .await?;
                dex.transfer_display_unit(a, &row.hash, row.raw, row.withdrawal)
                    .await
            }
            .await;
            match read {
                Ok(Some(proof)) => {
                    super::engine::lk(receipts).insert(key, proof);
                    Some(proof)
                }
                Ok(None) => None,
                Err(_) => {
                    unknown.insert((row.cex.clone(), row.withdrawal));
                    None
                }
            }
        };
        let Some((ui, historical)) = proof else {
            continue;
        };
        if historical != scale || ui == row.raw {
            continue;
        }
        let mode = if row.exchange == ui {
            Basis::Display
        } else if row.exchange == row.raw {
            Basis::Raw
        } else {
            continue;
        };
        modes
            .entry((row.cex.clone(), row.withdrawal))
            .and_modify(|old| {
                if *old != mode {
                    *old = Basis::Conflict;
                }
            })
            .or_insert(mode);
    }
    for key in &unknown {
        modes.remove(key);
    }
    (modes, !unknown.is_empty())
}
