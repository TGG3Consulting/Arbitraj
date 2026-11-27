//! Проверка ролей фабрики и FeeRouter Motoswap и получение котировки для заданного маршрута.
//! Загрузчик пулов и котировщик используют адреса и хеши из настроек площадки и ответы контрактов на выбранном блоке.
//! https://docs.motoswap.org/dev/addresses и https://docs.motoswap.org/dev/guides/swap-via-fee-router
#[path = "motoswap_state.rs"]
mod pool_state;
pub(crate) use pool_state::state;

use super::{
    abi_check::IPoolImmutables as Pair,
    families::Family,
    nami_batch as batch, solidly_classic_factory as code,
};
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::ctx::Ctx,
};
use alloy_primitives::{Address, B256, U256, keccak256};
use serde_json::Value;

alloy::sol! {interface IMoto {
    function pathVersion() external view returns(uint256);
    function upgradesRenounced() external view returns(bool);
    function router() external view returns(address);
    function WETH() external view returns(address);
    function quoteRegistry() external view returns(address);
    function registry() external view returns(address);
    function creatorRegistry() external view returns(address);
    function creatorVault() external view returns(address);
    function collector() external view returns(address);
    function beacon() external view returns(address);
    function implementation() external view returns(address);
    function pairCodeHash() external view returns(bytes32);
    function getAmountsOut(uint256 amountIn,address[] path) external view returns(uint256[]);
    function swapFeeBps() external view returns(uint256);
    function protocolFeeBps() external view returns(uint256);
    function creatorFeeBps() external view returns(uint256);
    function swapAllowed(address who) external view returns(bool);
    function quoteRank(address token) external view returns(uint256);
    function isQuote(address token) external view returns(bool);
    function activeCreator(address token) external view returns(address);
    function feeOnOutput(address[] path) external view returns(bool);
    function balanceOf(address owner) external view returns(uint256);
}}
#[derive(Clone, Copy, Eq, PartialEq)]
struct Role {
    address: Address,
    hash: B256,
    implementation: Option<(Address, B256)>,
}
fn bad(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NotApplicable,
        "Motoswap: не подтверждены исходные роли, реестр пары или тариф",
    )
}
fn role(ctx: &Ctx, rows: &[String], name: &str, proxy: bool) -> GatewayResult<Role> {
    let mut selected = rows
        .iter()
        .map(|s| s.split_whitespace().collect::<Vec<_>>())
        .filter(|parts| parts.first() == Some(&name));
    let row = selected.next().ok_or_else(|| bad(ctx))?;
    if selected.next().is_some() || row.len() != if proxy { 5 } else { 3 } {
        return Err(bad(ctx));
    }
    let addr = |s: &str| {
        s.parse::<Address>()
            .ok()
            .filter(|a| !a.is_zero())
            .ok_or_else(|| bad(ctx))
    };
    let hash = |s: &str| {
        s.parse::<B256>()
            .ok()
            .filter(|h| !h.is_zero() && *h != keccak256([]))
            .ok_or_else(|| bad(ctx))
    };
    Ok(Role {
        address: addr(row[1])?,
        hash: hash(row[2])?,
        implementation: if proxy {
            Some((addr(row[3])?, hash(row[4])?))
        } else {
            None
        },
    })
}
impl Role {
    async fn check(self, ctx: &Ctx, at: &Value) -> GatewayResult<()> {
        code::bind(ctx, self.address, self.hash, self.implementation, at).await
    }
}
#[path = "motoswap_frame.rs"]
mod frame;

pub(crate) fn routing(
    ctx: &Ctx,
    id: &str,
) -> GatewayResult<(
    Address,
    Address,
    crate::venues::bsc_dex::exec::carbon_binding::Code,
)> {
    let rows = ctx.dex_list("dex.motoswap_roles", id)?;
    let fee = role(ctx, &rows, "fee_router", true)?;
    let core = role(ctx, &rows, "core", false)?;
    let (implementation, runtime) = fee.implementation.ok_or_else(|| bad(ctx))?;
    Ok((
        fee.address,
        core.address,
        crate::venues::bsc_dex::exec::carbon_binding::Code {
            shell: fee.hash,
            implementation,
            runtime,
        },
    ))
}
pub(crate) async fn frozen(ctx: &Ctx, router: Address, at: &Value) -> GatewayResult<()> {
    let [raw] = batch::read(
        ctx,
        [batch::view(at, router, IMoto::upgradesRenouncedCall {})],
    )
    .await?;
    if batch::decode(&raw, IMoto::upgradesRenouncedCall {})? {
        Ok(())
    } else {
        Err(bad(ctx))
    }
}

pub(crate) async fn official(
    ctx: &Ctx,
    e: &super::registry::Entry,
    input: Address,
    n: U256,
    at: &Value,
) -> GatewayResult<U256> {
    let (a, b) = (e.pool.token0, e.pool.token1);
    let i = if input == a {
        0
    } else if input == b {
        1
    } else {
        return Err(bad(ctx));
    };
    let s = Box::pin(state(ctx, e.family, e.pool.reference.address(), (a, b), at)).await?;
    let projected = s.swap(i == 0, true, n).ok_or_else(|| bad(ctx))?;
    let pair_input = if s.fee_on_output[i] {
        n
    } else {
        s.net(n).ok_or_else(|| bad(ctx))?
    };
    let effective = s.balances[i]
        .checked_add(pair_input)
        .and_then(|v| v.checked_sub(s.reserves[i]))
        .ok_or_else(|| bad(ctx))?;
    let (_, core, _) = routing(ctx, e.family.id)?;
    let out = crate::venues::bsc_dex::node::call::canonical(
        ctx,
        at,
        core,
        IMoto::getAmountsOutCall {
            amountIn: effective,
            path: vec![input, [b, a][i]],
        },
    )
    .await?;
    let [spent, gross] = out.as_slice() else {
        return Err(bad(ctx));
    };
    let net = if s.fee_on_output[i] {
        s.net(*gross).ok_or_else(|| bad(ctx))?
    } else {
        *gross
    };
    if *spent != effective || net != projected.amount_out {
        return Err(bad(ctx));
    }
    Ok(net)
}
