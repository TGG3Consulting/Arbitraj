//! computeQuote даёт ориентир. Состояние счёта LP и доступность кредитного хранилища
//! подтверждаются полной симуляцией точного поручения перед допуском к отправке.
use super::{
    euler_swap::{self, IEuler},
    families::Family,
    integral_quotes::levels,
    ladder_counter, load_metric,
    registry::{Entry, Registry},
};
use crate::{
    core::types::PoolState,
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, node::call, tokens::Tokens},
};
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::Value;

fn quote(input: Address, output: Address, amount: U256) -> IEuler::computeQuoteCall {
    IEuler::computeQuoteCall {
        input,
        output,
        amount,
        exactIn: true,
    }
}

pub(crate) fn request(
    ctx: &Ctx,
    entry: &Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let p = &entry.pool;
    let output = if input == p.token0 {
        p.token1
    } else if input == p.token1 {
        p.token0
    } else {
        return Err(ctx.err(ErrorCode::NoRoute, "EulerSwap: неизвестная валюта входа"));
    };
    Ok((
        p.reference.address(),
        quote(input, output, amount).abi_encode().into(),
    ))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn ladder(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    counter: Address,
    at: &Value,
) -> GatewayResult<PoolState> {
    let backing = euler_swap::market(ctx, f, pool, pair, at).await?;
    let coin = if counter == pair.0 { pair.1 } else { pair.0 };
    let (mut bought, mut sold) = (Vec::new(), Vec::new());
    if backing.installed {
        let (mut buys, mut sells, bl, sl) = tokio::try_join!(
            ladder_counter::amounts(ctx, tokens, reg, f.id, counter),
            ladder_counter::amounts(ctx, tokens, reg, f.id, coin),
            call::canonical(
                ctx,
                at,
                pool,
                IEuler::getLimitsCall {
                    input: counter,
                    output: coin
                }
            ),
            call::canonical(
                ctx,
                at,
                pool,
                IEuler::getLimitsCall {
                    input: coin,
                    output: counter
                }
            )
        )?;
        let cap = |sizes: &mut Vec<U256>, maximum: U256| {
            if let Some(&first) = sizes.first() {
                sizes.push(first.min(maximum));
            }
            sizes.retain(|x| !x.is_zero() && *x <= maximum);
            sizes.sort_unstable();
            sizes.dedup();
        };
        cap(&mut buys, bl.inputMax);
        if !bl.outputMax.is_zero() {
            bought = levels(ctx, pool, (counter, coin), &buys, at, quote).await?;
        }
        sells.extend(bought.iter().map(|p| p.amount_out));
        cap(&mut sells, sl.inputMax);
        if !sl.outputMax.is_zero() {
            sold = levels(ctx, pool, (coin, counter), &sells, at, quote).await?;
        }
    }
    let mut state = load_metric::state(
        pair,
        coin,
        pair.0,
        (!bought.is_empty() || !sold.is_empty()).then_some(0),
        bought,
        sold,
    );
    if let PoolState::OraclePmm(d) = &mut state {
        d.shared_accounts = backing.sites;
    }
    Ok(state)
}
