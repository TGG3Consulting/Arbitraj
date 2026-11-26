//! Локальный расчёт направления и средств для котировки Fermi. Данные уровней и известные decimals берутся из кэша адаптера.
use super::super::{exec, gas_ticker, num, parts::Parts};
use super::markets::Books;
use crate::core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side};
use alloy::primitives::{Address, I256};
use chrono::Utc;
use rust_decimal::Decimal;
use std::{sync::Arc, time::Duration};
pub(super) struct Candidate {
    pub book: Arc<Books>,
    pub quote: Quote,
    pub who: Address,
    pub input: Address,
    pub output: Address,
    pub amount: I256,
    pub decs: (u8, u8),
}
pub(super) fn candidate(p: &Parts, asset: &Asset, side: Side, qty: Decimal) -> Option<Candidate> {
    if qty <= Decimal::ZERO || exec::native::is_native(&p.ctx, asset) {
        return None;
    }
    let b = p.slot_markets.current(p)?;
    let coin = asset.contract?;
    let cash = b.config.cash;
    let buy = side == Side::Buy;
    let (input, output) = if buy { (cash, coin) } else { (coin, cash) };
    let levels = b.sides.get(&(coin, buy))?;
    let gross = levels.price(qty, buy)?;
    let decs = (
        p.tokens.decimals_known(coin)?,
        p.tokens.decimals_known(cash)?,
    );
    let raw = num::to_units(qty, decs.0)?;
    if raw.is_zero() || raw.bit_len() > 255 || num::to_decimal(raw, decs.0) != Some(qty) {
        return None;
    }
    let amount = I256::from_raw(raw);
    let head = p.heads.fresh(&p.ctx).ok()?;
    if head.number != b.head.number.to::<u64>() {
        return None;
    }
    p.lag.check(&p.ctx).ok()?;
    super::super::quote::direct::taxes(&p.quoting(), asset).ok()?;
    let age = Duration::from_millis(b.config.interval);
    let funds = p
        .slot_markets
        .funds
        .get(p, 1, input, b.config.executor, age)?;
    let required = num::to_units(if buy { gross } else { qty }, funds.decimals)?;
    if funds.spend < required || funds.native < num::to_units(b.config.gas, 18)? {
        return None;
    }
    let quote = Quote {
        venue: p.ctx.venue(),
        asset: asset.clone(),
        side,
        qty,
        fillable_qty: qty,
        avg_price: gross.checked_div(qty)?,
        costs: vec![Cost::new(
            CostKind::Gas,
            b.config.gas,
            gas_ticker::read(&p.ctx).ok()?,
            false,
        )],
        route: Some(DexRoute {
            mode: RouteMode::SlotIndicative,
            legs: vec![],
            block: head.number,
            gas_units: 0,
            executable: false,
            exec_note: Some("Для Fermi требуется симуляция кошелька в открытом слоте".into()),
        }),
        signal_route: None,
        at: Utc::now(),
        pick: Some(format!(
            "Fermi {}, слот {}",
            b.config.builder.name(),
            b.frame.slot
        )),
    };
    Some(Candidate {
        book: b,
        quote,
        who: funds.who,
        input,
        output,
        amount: if buy { -amount } else { amount },
        decs,
    })
}
