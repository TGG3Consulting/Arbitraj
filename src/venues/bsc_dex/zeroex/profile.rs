use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{ctx::Ctx, keyed_price, node::rpc},
};
use alloy_primitives::{Address, Bytes, b256, keccak256};
use serde_json::{Value, json};

// Код содержит четыре неизменяемые роли из записанного CREATE. Карточка и тест повторяют тот же хеш.
const RUNTIME: alloy_primitives::B256 =
    b256!("c5547e965ffc209cdec17ed01f88def2bba6ef8a21f9ce287f9f25faf2cae9f9");

pub(in crate::venues::bsc_dex) fn module(ctx: &Ctx) -> GatewayResult<Address> {
    Some(ctx.addr("zeroex.wallet")?)
        .filter(|a| !a.is_zero() && ctx.chain_id() == Some(1))
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NotConfigured,
                "Разместите модуль 0x в выбранной сети Ethereum",
            )
        })
}
pub(in crate::venues::bsc_dex) async fn verify(
    ctx: &Ctx,
    at: &Value,
    address: Address,
    settler: Address,
) -> GatewayResult<()> {
    if module(ctx)? != address {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Изменился модуль 0x выбранного кошелька",
        ));
    }
    let read = rpc::call::<Bytes>(ctx, "eth_getCode", json!([address, at]));
    let (code, ()) = tokio::try_join!(read, keyed_price::verify_settler(ctx, settler, at))?;
    if keccak256(code) != RUNTIME {
        return Err(ctx.err(
            ErrorCode::NoRoute,
            "Код модуля 0x отличается от размещения на Ethereum",
        ));
    }
    Ok(())
}
