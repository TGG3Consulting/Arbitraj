//! Файл: pools/virtuals.rs. Читает виртуальные резервы и фактическую выплатную ликвидность публичного Bonding.
//! Кто вызывает: verify, load и предоплатная сверка. Настройки: dex.factory, dex.quoter, dex.bonding_roles.
use super::{families::Family, nami_batch as batch, registry::Entry, virtuals_roles};
use crate::{
    core::types::{PoolState, VirtualsState},
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::Value;

sol! {interface IBond {
    function tokenA() view returns(address);
    function tokenB() view returns(address);
    function router() view returns(address);
    function factory() view returns(address);
    function assetToken() view returns(address);
    function taxVault() view returns(address);
    function taxManager() view returns(address);
    function hasRole(bytes32 role,address account) view returns(bool);
    function getPair(address a,address b) view returns(address);
    function tokenInfo(address token) view returns(bytes);
    function balanceOf(address owner) view returns(uint256);
    function kLast() view returns(uint256);
    function getReserves() view returns(uint256 a,uint256 b);
    function buyTax() view returns(uint256);
    function sellTax() view returns(uint256);
    function gradThreshold() view returns(uint256);
    function hasAntiSniperTax(address pair) view returns(bool);
    function bondingV4() view returns(address);
    function getAmountsOut(address token,address denomination,uint256 amount) view returns(uint256);
}}
macro_rules! views {
    ($ctx:expr,$at:expr; $( $v:ident : $to:expr => $call:expr ),+ $(,)?) => {
        let [$($v),+]=batch::read($ctx,[$(batch::view($at,$to,$call)),+]).await?;
        $(let $v=batch::decode(&$v,$call)?;)+
    }
}

fn open(raw: &Bytes, token: Address, pool: Address, unicorn: bool) -> bool {
    let word = |i: usize| raw.get(i * 32..(i + 1) * 32);
    word(1) == Some(token.into_word().as_slice())
        && word(2) == Some(pool.into_word().as_slice())
        && word(11) == Some(B256::with_last_byte(1).as_slice())
        && word(12) == Some(B256::ZERO.as_slice())
        && (!unicorn || word(16) == Some(B256::with_last_byte(1).as_slice()))
}

pub(crate) async fn market(
    ctx: &Ctx,
    f: &Family,
    pool: Address,
    pair: (Address, Address),
    at: &Value,
) -> GatewayResult<VirtualsState> {
    let generation = ctx.settings.generation();
    let roles = virtuals_roles::at(ctx, f, at).await?;
    let virtuals_roles::Roles {
        factory,
        router,
        front,
        unicorn,
    } = roles;
    let invalid = || {
        ctx.err(
            ErrorCode::NotApplicable,
            "Virtuals: регистрация, флаги запуска или адреса действующего рынка изменились",
        )
    };
    views!(ctx,at; agent:pool=>IBond::tokenACall{}, asset:pool=>IBond::tokenBCall{}, pair_router:pool=>IBond::routerCall{},
        payer:router=>IBond::assetTokenCall{}, owner:router=>IBond::factoryCall{}, issuer:front=>IBond::factoryCall{},
        spender:front=>IBond::routerCall{}, allowed:router=>IBond::hasRoleCall{role:keccak256("EXECUTOR_ROLE"),account:front});
    if agent.is_zero()
        || asset.is_zero()
        || agent == asset
        || super::discover::sorted(agent, asset) != pair
        || pair_router != router
        || payer != asset
        || owner != factory
        || issuer != factory
        || spender != router
        || !allowed
    {
        return Err(invalid());
    }
    let [info] = batch::read(
        ctx,
        [batch::view(
            at,
            front,
            IBond::tokenInfoCall { token: agent },
        )],
    )
    .await?;
    if !open(&info, agent, pool, unicorn) {
        return Err(invalid());
    }
    views!(ctx,at; mapping:factory=>IBond::getPairCall{a:agent,b:asset},reserves:pool=>IBond::getReservesCall{}, invariant:pool=>IBond::kLastCall{},
      balance_a:agent=>IBond::balanceOfCall{owner:pool},balance_b:asset=>IBond::balanceOfCall{owner:pool},
      buy:factory=>IBond::buyTaxCall{},sell:factory=>IBond::sellTaxCall{},graduation:front=>IBond::gradThresholdCall{});
    if mapping != pool || invariant.is_zero() {
        return Err(invalid());
    }
    let buy_tax = u8::try_from(buy).unwrap_or(100);
    let (launch_tax, graduation) = if unicorn {
        views!(ctx,at; anti:router=>IBond::hasAntiSniperTaxCall{pair:pool},bonding4:router=>IBond::bondingV4Call{});
        if bonding4 != front {
            return Err(invalid());
        }
        if anti {
            (
                super::virtuals_tax::current(ctx, factory, pool, front, agent, buy_tax, at).await?,
                None,
            )
        } else {
            (0, Some(graduation))
        }
    } else {
        (0, Some(graduation))
    };
    let external_swap = if unicorn {
        false
    } else {
        views!(ctx,at; receiver:factory=>IBond::taxVaultCall{}, manager:router=>IBond::taxManagerCall{});
        receiver == manager
    };
    if ctx.settings.generation() != generation {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Настройки Bonding изменились при чтении",
        ));
    }
    Ok(VirtualsState {
        agent,
        asset,
        reserves: [reserves.a, reserves.b],
        backing: [balance_a, balance_b],
        invariant,
        graduation,
        buy_tax,
        sell_tax: u8::try_from(sell).unwrap_or(100),
        launch_tax,
        external_swap,
    })
}

pub(crate) async fn fresh(ctx: &Ctx, e: &Entry, at: &Value) -> GatewayResult<()> {
    let result = market(
        ctx,
        e.family,
        e.pool.reference.address(),
        (e.pool.token0, e.pool.token1),
        at,
    )
    .await?;
    if !matches!(&e.pool.state,PoolState::Virtuals(s) if **s==result) {
        return Err(ctx.err(
            ErrorCode::StaleData,
            "Virtuals: статус рынка, доступный остаток, налог или резервы изменились",
        ));
    }
    Ok(())
}

pub(crate) fn quote(
    ctx: &Ctx,
    e: &Entry,
    input: Address,
    amount: U256,
) -> GatewayResult<(Address, Bytes)> {
    let bad = || {
        ctx.err(
            ErrorCode::NoRoute,
            "Virtuals: запрошенный чистый обмен недоступен",
        )
    };
    let PoolState::Virtuals(s) = &e.pool.state else {
        return Err(bad());
    };
    super::super::math::virtuals::quote(s, input, true, amount).ok_or_else(bad)?;
    let buy = input == s.asset;
    let credited = if buy {
        super::super::math::virtuals::principal(s, amount).ok_or_else(bad)?
    } else {
        amount
    };
    let q = IBond::getAmountsOutCall {
        token: s.agent,
        denomination: if buy { s.asset } else { Address::ZERO },
        amount: credited,
    };
    Ok((
        ctx.dex_addr("dex.quoter", e.family.id)?,
        q.abi_encode().into(),
    ))
}

pub(crate) fn decode(e: &Entry, raw: &[u8], input: Address) -> Option<U256> {
    let PoolState::Virtuals(s) = &e.pool.state else {
        return None;
    };
    let gross = IBond::getAmountsOutCall::abi_decode_returns_validate(raw).ok()?;
    if input == s.asset {
        return Some(gross);
    }
    (input == s.agent).then_some(())?;
    gross.checked_sub(gross.checked_mul(U256::from(s.sell_tax))? / U256::from(100))
}
