//! Допуск BaseStonk по зарегистрированному PoolKey, исходному launcher и конфигурации налога.
use super::{
    families::Family,
    key::{PoolKey, UniswapPoolKeyData},
    launch_abi::bound,
    launch_keys::{Role, invalid},
    nami_batch as batch,
};
use crate::{gateway::errors::GatewayResult, venues::bsc_dex::ctx::Ctx};
use alloy_primitives::{Address, B256, U256, aliases::I24};
use serde_json::Value;

alloy::sol! {
    interface IStonk {
        struct Key { address currency0; address currency1; uint24 fee; int24 tickSpacing; address hooks; }
        function launcher() external view returns(address);
        function hook() external view returns(address);
        function poolManager() external view returns(address);
        function POOL_FEE() external view returns(uint24);
        function configOf(bytes32 id) external view returns(uint16);
        function sniperExempt(address sender) external view returns(bool);
        function taxFor(Key calldata key,bool zeroForOne) external view returns(uint16);
    }
}
pub(super) const SPACING: i32 = 60;
pub(super) const TIERS: [u32; 2] = [0, 3000];

pub(super) fn key(pair: (Address, Address), hook: Address, tier: u32) -> PoolKey {
    let (currency0, currency1) = super::discover::sorted(pair.0, pair.1);
    PoolKey::Uniswap(UniswapPoolKeyData {
        currency0,
        currency1,
        fee: tier,
        tick_spacing: I24::try_from(SPACING).expect("int24"),
        hooks: hook,
    })
}

pub(super) fn restore(roles: &[Role], pair: (Address, Address), id: B256) -> Option<PoolKey> {
    roles
        .iter()
        .filter(|r| r.kind == "hook")
        .flat_map(|r| TIERS.map(|fee| key(pair, r.address, fee)))
        .find(|k| k.id() == id)
}

pub(super) struct Terms {
    pub token_is_zero: bool,
    pub sniper: bool,
    pub buy: u16,
}
pub(super) fn config(raw: &[u8]) -> Option<Terms> {
    if !raw.len().is_multiple_of(32) {
        return None;
    }
    let w = raw
        .chunks_exact(32)
        .map(U256::from_be_slice)
        .collect::<Vec<_>>();
    let bps = |n: U256| u16::try_from(n).ok().filter(|x| *x <= 10_000);
    let boolean = |n: U256| match u8::try_from(n).ok()? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    match w.as_slice() {
        [tax, burn, lp, set]
            if boolean(*set) == Some(true)
                && u32::from(bps(*burn)?) + u32::from(bps(*lp)?) <= 10_000 =>
        {
            Some(Terms {
                token_is_zero: true,
                sniper: false,
                buy: bps(*tax)?,
            })
        }
        [buy, sell, burn, lp, zero, set, ends, window, start]
            if boolean(*set) == Some(true)
                && bps(*sell).is_some()
                && u32::from(bps(*burn)?) + u32::from(bps(*lp)?) <= 10_000 =>
        {
            Some(Terms {
                token_is_zero: boolean(*zero)?,
                buy: bps(*buy)?,
                sniper: !ends.is_zero() && !window.is_zero() && bps(*start)? > bps(*buy)?,
            })
        }
        _ => None,
    }
}

pub(super) async fn read(
    ctx: &Ctx,
    f: &Family,
    key: &PoolKey,
    roles: &[Role],
    at: &Value,
) -> GatewayResult<(Address, bool, bool)> {
    let PoolKey::Uniswap(k) = key else {
        return Err(invalid(ctx));
    };
    let get = IStonk::launcherCall {};
    let [l, raw] = batch::read(
        ctx,
        [
            batch::view(at, k.hooks, get.clone()),
            batch::view(at, k.hooks, IStonk::configOfCall { id: key.id() }),
        ],
    )
    .await?;
    let launcher = batch::decode(&l, get)?;
    let terms = config(&raw).ok_or_else(|| invalid(ctx))?;
    let hook = IStonk::hookCall {};
    let mgr = IStonk::poolManagerCall {};
    let fee = IStonk::POOL_FEECall {};
    let ((), [h, m, t]) = tokio::try_join!(
        bound(ctx, roles, "factory", launcher, at),
        batch::read(
            ctx,
            [
                batch::view(at, launcher, hook.clone()),
                batch::view(at, launcher, mgr.clone()),
                batch::view(at, launcher, fee.clone())
            ]
        )
    )?;
    if batch::decode(&h, hook)? != k.hooks
        || batch::decode(&m, mgr)? != ctx.dex_addr("dex.pool_manager", f.id)?
        || batch::decode(&t, fee)?.to::<u32>() != k.fee
    {
        return Err(invalid(ctx));
    }
    let token = if terms.token_is_zero {
        k.currency0
    } else {
        k.currency1
    };
    let buy = if terms.sniper {
        let q = ctx.dex_addr("dex.quoter", f.id)?;
        // Хук видит SimpleSwapRouter при прямой оплате, даже если внешний модуль отсутствует в V5.
        let sender = match crate::venues::bsc_dex::exec::executor::address(ctx)? {
            Some(executor) => executor,
            None if ctx.dex_addr("dex.router", f.id).is_ok() => original(ctx, f)?.0,
            None => q,
        };
        let q_ex = IStonk::sniperExemptCall { sender: q };
        let ex = IStonk::sniperExemptCall { sender };
        let rate = IStonk::taxForCall {
            key: IStonk::Key {
                currency0: k.currency0,
                currency1: k.currency1,
                fee: alloy_primitives::aliases::U24::from(k.fee),
                tickSpacing: k.tick_spacing,
                hooks: k.hooks,
            },
            zeroForOne: !terms.token_is_zero,
        };
        let [a, b, c] = batch::read(
            ctx,
            [
                batch::view(at, k.hooks, q_ex.clone()),
                batch::view(at, k.hooks, ex.clone()),
                batch::view(at, k.hooks, rate.clone()),
            ],
        )
        .await?;
        batch::decode(&a, q_ex)? == batch::decode(&b, ex)? || batch::decode(&c, rate)? == terms.buy
    } else {
        true
    };
    Ok((token, buy, true))
}

pub(in crate::venues::bsc_dex) fn original(
    ctx: &Ctx,
    f: &Family,
) -> GatewayResult<(Address, B256)> {
    let mut roles = super::launch_keys::roles(ctx, f)?
        .into_iter()
        .filter(|r| r.kind == "adapter");
    let r = roles.next().ok_or_else(|| invalid(ctx))?;
    if roles.next().is_some() {
        return Err(invalid(ctx));
    }
    Ok((r.address, r.hash))
}
