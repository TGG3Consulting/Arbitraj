//! Файл: venues/bsc_dex/quote/steps.rs
//! Что делает: затраты одного шага пути (решения 35, 52) — у каждого обмена своя строка с шагом маршрута (Cost::with_step):
//! газ обмена в этом пуле (BNB), комиссия пула (в токене входа шага), налог монеты на её шаге (покупка — шаг, где
//! монета на выходе; продажа — где на входе), влияние крючка (в токене выхода шага). Комиссия, налог и крючок уже
//! в цене (in_price = true — для прозрачности), газ — отдельно. Валюта каждой затраты — тикер, который ядро может
//! пересчитать: USDT (venue.quote_asset), тикер монеты, тикер промежуточной (биржи или «BNB» для WBNB).
//! Кто вызывает: quote/costs.rs.
//! От чего зависит: quote/path.rs, quote/pool_quote.rs (комиссия V4), pools/mids.rs, num.rs, core/types (Cost, RouteStep).
//! Настройки: dex.v2_fee_pct [вид пула V2].
//!
//! Шаг маршрута и валюты затрат — стык core/types/cost.rs (RouteStep, Cost::with_step) и ответ ядра в PR #25.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{Cost, CostKind, PoolKind, PoolState, RouteStep, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::mids::Mids;
use super::super::pools::registry::Entry;
use super::super::wallet::balances::BNB;
use super::path::{Hop, Path};
use super::pool_quote::swap_fee;

/// Тикеры токенов пути для затрат.
pub struct Names<'a> {
    pub usdt: (Address, &'a str),
    pub coin: (Address, &'a str),
    pub mids: &'a Mids,
}

impl Names<'_> {
    /// Тикер токена. Ошибки: BadResponse — токен не USDT, не монета и не промежуточная.
    pub fn of(&self, ctx: &Ctx, a: Address) -> GatewayResult<String> {
        if a == self.usdt.0 {
            Ok(self.usdt.1.to_string())
        } else if a == self.coin.0 {
            Ok(self.coin.1.to_string())
        } else {
            self.mids.ticker(a).ok_or_else(|| {
                ctx.err(
                    ErrorCode::BadResponse,
                    format!("тикер токена {a} неизвестен — затрату не пересчитать"),
                )
            })
        }
    }

    /// Шаг маршрута «вход → выход» с номером (с 1). Ошибки: как у of.
    pub fn step(
        &self,
        ctx: &Ctx,
        index: u32,
        from: Address,
        to: Address,
    ) -> GatewayResult<RouteStep> {
        Ok(RouteStep {
            index,
            from: self.of(ctx, from)?,
            to: self.of(ctx, to)?,
            from_token: Some(from),
            to_token: Some(to),
        })
    }
}

/// Затраты всех шагов пути. Принимает: путь, сторону, налоги монеты, цену газа (wei), тикеры.
/// Ошибки: BadResponse (числа, тикер), NotConfigured (dex.v2_fee_pct).
pub fn path_costs(
    ctx: &Ctx,
    p: &Path,
    side: Side,
    t: &TokenTaxes,
    gas_wei: U256,
    names: &Names<'_>,
) -> GatewayResult<Vec<Cost>> {
    let pct = |v: Decimal, x: Decimal| v * x / Decimal::ONE_HUNDRED;
    let coin = names.coin.0;
    let mut out = Vec::new();
    for (i, h) in p.hops.iter().enumerate() {
        let step = names.step(ctx, i as u32 + 1, h.token_in, h.token_out)?;
        let (from, to) = (step.from.clone(), step.to.clone());
        let add = |out: &mut Vec<Cost>, kind, amount, asset: &str, in_price| {
            out.push(Cost::new(kind, amount, asset, in_price).with_step(step.clone()));
        };
        add(
            &mut out,
            CostKind::Gas,
            gas_bnb(ctx, gas_wei, h)?,
            BNB,
            false,
        );
        add(
            &mut out,
            CostKind::PoolFee,
            pct(h.pool_in, fee_pct(ctx, &h.entry, h.token_in)?),
            &from,
            true,
        );
        if side == Side::Buy && h.token_out == coin {
            add(
                &mut out,
                CostKind::BuyTax,
                pct(p.coins, t.buy_pct),
                &from_coin(names),
                true,
            );
        }
        if side == Side::Sell && h.token_in == coin {
            add(
                &mut out,
                CostKind::SellTax,
                pct(p.coins, t.sell_pct),
                &from_coin(names),
                true,
            );
        }
        add(
            &mut out,
            CostKind::HookImpact,
            pct(h.pool_out, h.usable.hook_pct),
            &to,
            true,
        );
    }
    Ok(out)
}

/// Тикер монеты. Ошибок нет.
fn from_coin(names: &Names<'_>) -> String {
    names.coin.1.to_string()
}

/// Газ обмена в пуле шага, BNB. Ошибки: BadResponse.
fn gas_bnb(ctx: &Ctx, gas_wei: U256, h: &Hop) -> GatewayResult<Decimal> {
    num::to_decimal(
        gas_wei.saturating_mul(U256::from(h.usable.gas)),
        num::BNB_DECIMALS,
    )
    .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "газ не помещается в Decimal"))
}

/// Полная комиссия пула в процентах для направления «токен входа → другой». Ошибки: NotConfigured (V2), BadResponse.
pub fn fee_pct(ctx: &Ctx, e: &Entry, token_in: Address) -> GatewayResult<Decimal> {
    let pips = |p: u32| Decimal::from(p) / Decimal::from(10_000u32);
    let zero_for_one = token_in == e.pool.token0;
    match (&e.pool.state, e.pool.kind) {
        (_, PoolKind::V2) => ctx.dex_dec("dex.v2_fee_pct", e.family.id),
        (_, PoolKind::V3) => e
            .pool
            .fee_pips
            .map(pips)
            .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "у пула V3 нет fee")),
        (
            PoolState::Cl {
                protocol_fee,
                lp_fee,
                ..
            }
            | PoolState::Bin {
                protocol_fee,
                lp_fee,
                ..
            },
            _,
        ) => Ok(pips(swap_fee(*protocol_fee, *lp_fee, zero_for_one))),
        _ => Err(ctx.err(ErrorCode::BadResponse, "состояние пула не того вида")),
    }
}
