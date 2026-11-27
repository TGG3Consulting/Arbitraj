//! Оплата WETH на нативном рынке o1. Поручение и квитанция закрепляют физический PoolId.
use super::super::{
    ctx::Ctx,
    pools::{flux_native, launch_market, registry::Entry},
    send::track::Receipt,
};
use super::{fill, signed_call};
use crate::{
    core::types::PoolRef,
    gateway::errors::{ErrorCode, GatewayResult},
};
use alloy::{
    rpc::types::Log,
    sol,
    sol_types::{SolCall, SolEvent},
};
use alloy_primitives::{Address, B256, Bytes, U256};
use serde::{Deserialize, Serialize};
use serde_json::Value;

sol! {interface IWallet {
    function execute(bytes32 id,address tokenIn,address tokenOut,uint256 qty,uint256 minimum,uint256 deadline) external returns(uint256);
    event Filled(address indexed payer,address indexed recipient,address indexed tokenIn,address tokenOut,uint256 paid,uint256 received,bytes32 poolId);
}}

pub(in crate::venues::bsc_dex) fn enabled(e: &Entry) -> bool {
    e.family.variant.native_market_module()
}
pub(super) fn missing(ctx: &Ctx) -> crate::gateway::errors::GatewayError {
    ctx.err(
        ErrorCode::NoRoute,
        "Требуется проверенный модуль расчёта в WETH с подписанным PoolId",
    )
}
pub(in crate::venues::bsc_dex) fn key(ctx: &Ctx, e: &Entry) -> GatewayResult<B256> {
    match e.pool.reference {
        PoolRef::Singleton { id, .. } if enabled(e) => Ok(id),
        _ => Err(missing(ctx)),
    }
}
pub(in crate::venues::bsc_dex) async fn at(ctx: &Ctx, e: &Entry, tag: &Value) -> GatewayResult<()> {
    let PoolRef::Singleton { manager, id } = e.pool.reference else {
        return Err(missing(ctx));
    };
    if e.family.variant.flux_native() {
        return flux_native::module(ctx,e,tag).await;
    }
    launch_market::check(
        ctx,
        e.family,
        manager,
        id,
        (e.pool.token0, e.pool.token1),
        tag,
    )
    .await
}
pub(in crate::venues::bsc_dex) fn data(
    id: B256,
    input: Address,
    output: Address,
    qty: U256,
    minimum: U256,
    deadline: U256,
) -> Bytes {
    IWallet::executeCall {
        id,
        tokenIn: input,
        tokenOut: output,
        qty,
        minimum,
        deadline,
    }
    .abi_encode()
    .into()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proof {
    pub id: B256,
    pub module: Address,
    pub payer: Address,
    pub input: Address,
    pub output: Address,
    pub amount: U256,
    pub minimum: U256,
    pub deadline: U256,
}
impl Proof {
    pub(super) fn data(&self) -> Bytes {
        data(
            self.id,
            self.input,
            self.output,
            self.amount,
            self.minimum,
            self.deadline,
        )
    }
    pub(super) async fn included(&self, ctx: &Ctx, hash: B256, r: &Receipt) -> GatewayResult<()> {
        signed_call::included(
            ctx,
            hash,
            r,
            self.payer,
            self.module,
            U256::ZERO,
            self.data(),
        )
        .await
    }
    pub(in crate::venues::bsc_dex) fn measured(
        &self,
        logs: &[super::super::check::simulate::SimLog],
    ) -> Option<(U256, U256)> {
        self.amounts(
            &signed_call::logs(logs)?,
            self.payer,
            self.input,
            self.output,
        )
    }
    pub(super) fn amounts(
        &self,
        logs: &[Log],
        wallet: Address,
        input: Address,
        output: Address,
    ) -> Option<(U256, U256)> {
        if (wallet, input, output) != (self.payer, self.input, self.output)
            || self.minimum.is_zero()
            || input == output
        {
            return None;
        }
        let mut events = logs.iter().filter(|l| {
            l.address() == self.module && l.topic0() == Some(&IWallet::Filled::SIGNATURE_HASH)
        });
        let event = IWallet::Filled::decode_log_data(&events.next()?.inner.data).ok()?;
        if events.next().is_some()
            || (
                event.payer,
                event.recipient,
                event.tokenIn,
                event.tokenOut,
                event.poolId,
            ) != (wallet, wallet, input, output, self.id)
            || event.paid != self.amount
            || event.received < self.minimum
        {
            return None;
        }
        let moved = |token, from, to| fill::moved_logs(logs, token, from, to);
        let spent =
            moved(input, Some(wallet), None).checked_sub(moved(input, None, Some(wallet)))?;
        let net =
            moved(output, None, Some(wallet)).checked_sub(moved(output, Some(wallet), None))?;
        ((spent, net) == (event.paid, event.received)).then_some((spent, net))
    }
}
