//! Для старых семейств сохраняет prefetch по высоте; для фиксированного снимка передаёт EIP-1898 во все чтения тиков.
use super::super::{
    ctx::Ctx,
    node::{call, rpc},
};
use crate::gateway::errors::GatewayResult;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub(super) enum At<'a> {
    Height(&'a str),
    Canonical(&'a Value),
}
impl At<'_> {
    pub(super) async fn view<C: SolCall>(
        self,
        ctx: &Ctx,
        to: Address,
        method: &C,
    ) -> GatewayResult<C::Return> {
        match self {
            Self::Height(tag) => call::view(ctx, to, method, tag).await,
            Self::Canonical(hash) => {
                let bytes = Bytes::from(method.abi_encode());
                let raw: Bytes =
                    rpc::call(ctx, "eth_call", json!([{"to":to,"data":bytes},hash])).await?;
                call::decode::<C>(&raw)
            }
        }
    }
}
