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
    // При покупке CHF выдаёт мост. На краю диапазона встречная продажа
    // может вернуть лишь пыль даже при полностью обеспеченной покупке.
    let (needed, covered) = buy_depth(ctx, rates, bridge, cash, units.0)?;
    let buy = num::to_decimal(needed, dec)
        .ok_or_else(|| "Глубина покупки не помещается в USDT".to_owned())?;
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
    let (sell, s) = value(units.1)?;
    Ok(Band {
        buy,
        sell,
        full: units.2 && covered && s,
    })
}

fn buy_depth(
    ctx: &Ctx,
    direct: &[Arc<Entry>],
    bridge: &Bridge,
    cash: Address,
    want: U256,
) -> Result<(U256, bool), String> {
    if want.is_zero() {
        return Ok((U256::ZERO, true));
    }
    let mut target = want;
    while target > U256::ZERO {
        let price = |pools: &[Arc<Entry>], out: Address, qty: U256| {
            pools
                .iter()
                .filter(|e| e.valid)
                .filter_map(|e| {
                    let input = if out == e.pool.token0 {
                        e.pool.token1
                    } else if out == e.pool.token1 {
                        e.pool.token0
                    } else {
                        return None;
                    };
                    let s = pool_quote::swap(ctx, e, input, false, qty).ok()?;
                    (s.complete && s.amount_out == qty && !s.amount_in.is_zero())
                        .then_some(s.amount_in)
                })
                .min()
        };
        let straight = price(direct, cash, target);
        let via = price(&bridge.to_wrapped, cash, target)
            .and_then(|mid| price(&bridge.to_usdt, bridge.wrapped, mid));
        if let Some(paid) = straight.into_iter().chain(via).min() {
            return Ok((paid, target == want));
        }
        if direct.is_empty() && (bridge.to_wrapped.is_empty() || bridge.to_usdt.is_empty()) {
            break;
        }
        target >>= 1;
    }
    Err("Aktionariat: расчётная валюта не покупает CHF по прочитанным пулам".to_owned())
}
