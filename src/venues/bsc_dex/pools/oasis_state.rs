//! Состояние OasisSwap и изменение чистых резервов после доли протокольной комиссии.
use super::{families::Family, load::Loaded};
use crate::{core::types::PoolState, gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy_primitives::{Address, U256};
pub(crate) struct OasisMarket {
    pub reserve0: U256,
    pub reserve1: U256,
    pub rate: u32,
    pub rebated: u32,
    pub share: u32,
    pub native: Address,
    pub adapter: Option<Address>,
    pub native_rebate: bool,
    pub rebate_available: bool,
    pub(super) repeatable: bool,
}
impl OasisMarket {
    pub fn state(&self) -> PoolState {
        PoolState::Oasis {
            reserve0: self.reserve0,
            reserve1: self.reserve1,
            share: self.share,
            repeatable: self.repeatable,
        }
    }
}

pub(super) async fn load(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    block: &str,
) -> GatewayResult<Loaded> {
    let tag = super::elfomo::block(ctx, block).await?;
    let data = if f.executor_kind.is_some() {
        match super::oasis::at(ctx, f, pool, pair, &tag, None, true).await {
            Ok(v) => v,
            Err(module_error) => {
                match super::oasis::at(ctx, f, pool, pair, &tag, None, false).await {
                    Ok(v) => v,
                    Err(_) => return Err(module_error),
                }
            }
        }
    } else {
        super::oasis::at(ctx, f, pool, pair, &tag, None, false).await?
    };
    Ok(Loaded {
        state: data.state(),
        fee_pips: Some(data.rate * 100),
        ticks: None,
        bins: None,
    })
}

pub fn after(
    r0: U256,
    r1: U256,
    share: u32,
    pips: u32,
    zfo: bool,
    dx: U256,
    dy: U256,
) -> Option<PoolState> {
    if pips >= 1_000_000 || !pips.is_multiple_of(100) || share > 10_000 {
        return None;
    }
    let cached = dx
        .checked_mul(U256::from(pips / 100))?
        .checked_mul(U256::from(share))?
        / U256::from(100_000_000u64);
    let (rin, rout) = if zfo { (r0, r1) } else { (r1, r0) };
    let next_in = rin.checked_add(dx)?.checked_sub(cached)?;
    let next_out = rout.checked_sub(dy)?;
    if next_in.bit_len() > 112 {
        return None;
    }
    let (reserve0, reserve1) = if zfo {
        (next_in, next_out)
    } else {
        (next_out, next_in)
    };
    Some(PoolState::Oasis {
        reserve0,
        reserve1,
        share,
        repeatable: true,
    })
}
