//! Сравнивает доступные книги и AMM в общей валюте. Торговый билет принадлежит одному кошельку.
use super::{
    book::{Books, MarketBook},
    wire::{self, Proof},
};
use crate::core::types::{Asset, Cost, CostKind, DexRoute, Quote, RouteMode, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::venues::bsc_dex::{
    exec, gas_ticker, hashflow, node::op_fees::Oracle, num, parts::Parts, send,
};
use alloy_primitives::{Address, B256, U256, keccak256};
use chrono::Utc;
use rust_decimal::Decimal;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub(super) struct Ticket {
    pub book: Arc<Books>,
    pub order: Proof,
    pub decs: (u8, u8),
    pub gas: Decimal,
}
type Map = HashMap<(Address, B256), Ticket>;
#[derive(Default, Clone)]
pub(super) struct Tickets(Arc<Mutex<Map>>);
impl Tickets {
    fn save(&self, t: Ticket) -> Option<B256> {
        let id = keccak256(t.order.data()?);
        let payer = t.order.payer;
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.retain(|_, q| q.book.expires > Utc::now().timestamp_millis());
        m.insert((payer, id), t);
        Some(id)
    }
    pub(super) fn take(&self, payer: Address, id: B256) -> Option<Ticket> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(payer, id))
    }
}
pub(in crate::venues::bsc_dex) fn active(p: &Parts) -> bool {
    p.ctx.chain_id() == Some(8453)
        && p.ctx.flag("hanji.trading").unwrap_or(false)
        && p.hanji.current(p).is_some()
}
fn candidate(
    p: &Parts,
    book: &Arc<Books>,
    m: &MarketBook,
    asset: &Asset,
    side: Side,
    qty: Decimal,
) -> Option<(Quote, Ticket)> {
    let raw = num::to_units(qty, m.decimals.0)?;
    if raw.is_zero() || num::to_decimal(raw, m.decimals.0)? != qty {
        return None;
    }
    let buy = side == Side::Buy;
    let coin = asset.contract?;
    let (input, output) = if buy {
        (book.config.cash, coin)
    } else {
        (coin, book.config.cash)
    };
    let (direction, levels) = m.direction(input);
    let f = m.units.exact_quantity(levels, direction, raw, buy)?;
    let amount = if buy { f.spent } else { f.received };
    let cash = num::to_decimal(amount, m.decimals.1)?;
    let mut budget = f.spent;
    let mut commission = (f.fee_y / m.units.y).try_into().ok()?;
    if buy && output == m.x {
        // Market may temporarily pull the whole bid at the limit before refunding better fills.
        let value = (raw / m.units.x).checked_mul(U256::from(f.last_price))?;
        let fee = value
            .checked_mul(U256::from(m.units.fee_rate))?
            .div_ceil(U256::from(super::RATE_SCALE));
        budget = value.checked_add(fee)?.checked_mul(m.units.y)?;
        commission = fee.try_into().ok()?;
    }
    let who = p
        .hanji
        .funds
        .get(p, 8453, input, book.config.executor, book.config.age)?;
    if who.spend < budget || who.native < num::to_units(book.config.gas, 18)? {
        return None;
    }
    let mut seed = Vec::from(book.head.hash.as_slice());
    seed.extend_from_slice(who.who.as_slice());
    seed.extend_from_slice(m.market.as_slice());
    seed.extend_from_slice(input.as_slice());
    seed.extend_from_slice(&raw.to_be_bytes::<32>());
    let order = Proof {
        executor: book.config.executor,
        payer: who.who,
        market: m.market,
        input,
        output,
        budget,
        max_unspent: if buy {
            budget.checked_sub(U256::from(1))?
        } else {
            U256::ZERO
        },
        min_received: f.received,
        max_received: if buy { raw } else { U256::MAX },
        price: f.last_price,
        max_commission: commission,
        deadline: book
            .head
            .timestamp
            .to::<u64>()
            .checked_add(book.config.deadline)?,
        leg: keccak256(seed),
    };
    let q = Quote {
        venue: p.ctx.venue(),
        asset: asset.clone(),
        side,
        qty,
        fillable_qty: qty,
        avg_price: cash.checked_div(qty)?,
        costs: vec![Cost::new(
            CostKind::Gas,
            book.config.gas,
            gas_ticker::read(&p.ctx).ok()?,
            false,
        )],
        route: Some(DexRoute {
            mode: RouteMode::OrderbookIndicative,
            legs: vec![],
            block: book.head.number.to(),
            gas_units: 0,
            executable: false,
            exec_note: Some("Требуется проверка Hanji от кошелька".into()),
        }),
        at: Utc::now(),
        signal_route: None,
        pick: Some(format!("Hanji {:#x}", m.market)),
    };
    Some((
        q,
        Ticket {
            book: book.clone(),
            order,
            decs: m.decimals,
            gas: book.config.gas,
        },
    ))
}
async fn firm(p: &Parts, mut q: Quote, mut t: Ticket) -> Option<(Quote, Ticket)> {
    let h = &t.book.head;
    let ctx = &p.ctx;
    let (spent, received, _, used) = wire::simulate(ctx, h, &t.order).await.ok()?;
    let env = p.env();
    let channels = send::select(ctx, &p.stats).ok()?;
    if send::tip(&channels).is_some()
        || channels
            .iter()
            .any(|r| r.method != crate::core::types::SendMethod::Single)
    {
        return None;
    }
    let fees = exec::tx_fee::price(&env, &channels, None).await.ok()?;
    let gas =
        send::tx::with_margin(ctx, used.into(), ctx.dec("send.gas_limit_margin_pct").ok()?).ok()?;
    let l1 = Oracle::read(ctx)
        .ok()?
        .planned(ctx, h.number.to(), gas)
        .await
        .ok()?;
    let wei = fees.cap().checked_mul(U256::from(gas))?.checked_add(l1)?;
    t.gas = num::to_decimal(wei, 18)?;
    if t.gas > t.book.config.gas || super::book::header(ctx).await.ok()?.hash != h.hash {
        return None;
    }
    let buy = q.side == Side::Buy;
    let gross = num::to_decimal(if buy { spent } else { received }, t.decs.1)?;
    q.avg_price = gross.checked_div(q.qty)?;
    q.costs[0].amount = t.gas;
    if let Some(r) = q.route.as_mut() {
        r.executable = true;
        r.gas_units = used;
        r.exec_note = None;
    }
    Some((q, t))
}
pub(in crate::venues::bsc_dex) async fn consider(
    p: &Parts,
    asset: &Asset,
    side: Side,
    qty: Decimal,
    baseline: GatewayResult<Quote>,
) -> GatewayResult<Quote> {
    if !active(p)
        || baseline
            .as_ref()
            .is_err_and(|e| e.code != ErrorCode::NoRoute)
        || qty <= Decimal::ZERO
        || exec::native::is_native(&p.ctx, asset)
        || p.lag.check(&p.ctx).is_err()
        || crate::venues::bsc_dex::quote::direct::taxes(&p.quoting(), asset).is_err()
    {
        return baseline;
    }
    let Some(b) = p.hanji.current(p) else {
        return baseline;
    };
    let Some(markets) = asset.contract.and_then(|a| b.by_coin.get(&a)) else {
        return baseline;
    };
    let final_stage = crate::gateway::quote_stage::is_final();
    let mut score = None;
    let mut best = None;
    if let Ok(q) = &baseline
        && q.qty == qty
        && q.fillable_qty == qty
        && (!final_stage || q.route.as_ref().is_none_or(|r| r.executable))
    {
        let Some(cost) = hashflow::routing::total(p, q).await else {
            return baseline;
        };
        score = Some(cost);
    }
    let options = markets
        .iter()
        .filter_map(|m| candidate(p, &b, m, asset, side, qty));
    let tasks = options.map(|(q, t)| async move {
        if final_stage {
            firm(p, q, t).await
        } else {
            Some((q, t))
        }
    });
    let Ok(all) =
        tokio::time::timeout(b.config.timeout, futures_util::future::join_all(tasks)).await
    else {
        return baseline;
    };
    for (q, t) in all.into_iter().flatten() {
        let Some(net) = hashflow::routing::total(p, &q).await else {
            continue;
        };
        if score.is_none_or(|old| {
            if side == Side::Buy {
                net < old
            } else {
                net > old
            }
        }) {
            score = Some(net);
            best = Some((q, t));
        }
    }
    let Some((mut q, t)) = best else {
        return baseline;
    };
    if !p.hanji.current(p).is_some_and(|x| Arc::ptr_eq(&x, &b)) {
        return baseline;
    }
    if final_stage {
        let Some(id) = p.hanji.tickets.save(t) else {
            return baseline;
        };
        if let Some(r) = q.route.as_mut() {
            r.mode = RouteMode::Orderbook(id);
        }
    }
    Ok(q)
}
pub(in crate::venues::bsc_dex) fn breaks(
    p: &Parts,
    asset: &Asset,
    side: Side,
    max: Decimal,
) -> Vec<Decimal> {
    let Some(b) = p.hanji.current(p) else {
        return vec![];
    };
    let mut result = Vec::new();
    if let Some(markets) = asset.contract.and_then(|a| b.by_coin.get(&a)) {
        for m in markets {
            let buy = side == Side::Buy;
            let coin = asset.contract.expect("проверено выше");
            let (dir, levels) = m.direction(if buy { b.config.cash } else { coin });
            let mut lots = U256::ZERO;
            for (i, lvl) in levels.iter().enumerate() {
                lots = lots.saturating_add(U256::from(lvl.shares));
                let Some(x) = lots.checked_mul(m.units.x) else {
                    break;
                };
                let qty = if coin == m.x {
                    x
                } else {
                    let Some(f) =
                        m.units
                            .exact_quantity(&levels[..=i], dir, x, dir == super::Side::BuyX)
                    else {
                        break;
                    };
                    if buy { f.received } else { f.spent }
                };
                if let Some(q) = num::to_decimal(qty, m.decimals.0).filter(|q| *q <= max) {
                    result.push(q);
                }
            }
            if coin == m.x
                && let Some(raw) = num::to_units(max, m.decimals.0)
                && !m.units.x.is_zero()
                && let Some(q) = num::to_decimal(raw / m.units.x * m.units.x, m.decimals.0)
                    .filter(|q| *q > Decimal::ZERO)
            {
                result.push(q);
            }
        }
    }
    result.sort_unstable();
    result.dedup();
    result
}
