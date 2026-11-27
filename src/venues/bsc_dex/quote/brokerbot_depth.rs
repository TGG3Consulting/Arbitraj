//! Глубина акции в USDT ограничена исполнимой частью двух обменов валюты.
use super::{depth_band::Band, pool_quote, rate};
use crate::venues::bsc_dex::{
    ctx::Ctx,
    num,
    pools::{depth_key::Bridge, registry::Entry},
};
use alloy_primitives::{Address, U256};
use std::sync::Arc;

fn convertible(
    ctx: &Ctx,
    pools: &[Arc<Entry>],
    input: Address,
    amount: U256,
) -> Option<(U256, bool)> {
    pools
        .iter()
        .filter(|e| e.valid)
        .filter_map(|e| {
            let f = pool_quote::swap(ctx, e, input, true, amount).ok()?;
            if f.complete && f.amount_in == amount {
                return Some((f.amount_out, true));
            }
            if f.amount_in.is_zero() || f.amount_in >= amount {
                return None;
            }
            let part = pool_quote::swap(ctx, e, input, true, f.amount_in).ok()?;
            (part.complete && part.amount_in == f.amount_in && !part.amount_out.is_zero())
                .then_some((part.amount_out, false))
        })
        .max()
}
pub(super) fn band(
    ctx: &Ctx,
    cash: Address,
    dec: u8,
    units: (U256, U256, bool),
    rates: &[Arc<Entry>],
    bridge: &Bridge,
) -> Result<Band, String> {
    let value = |a| {
        if a == U256::ZERO {
            return Ok((rust_decimal::Decimal::ZERO, true));
        }
        let direct = rate::best_in(ctx, rates, cash, cash, a)
            .ok()
            .map(|out| (out, true));
        let via = convertible(ctx, &bridge.to_wrapped, cash, a).and_then(|(mid, all)| {
            let (out, last) = convertible(ctx, &bridge.to_usdt, bridge.wrapped, mid)?;
            Some((out, all && last))
        });
        let (raw, full) = direct
            .into_iter()
            .chain(via)
            .max()
            .ok_or_else(|| "Aktionariat: нет измеримого пути CHF → USDT".to_owned())?;
        let usd = num::to_decimal(raw, dec)
            .ok_or_else(|| "Глубина Aktionariat не помещается в USDT".to_owned())?;
        Ok::<_, String>((usd, full))
    };
    let (buy, b) = value(units.0)?;
    let (sell, s) = value(units.1)?;
    Ok(Band {
        buy,
        sell,
        full: units.2 && b && s,
    })
}
