//! Файл: pools/virtuals_tax.rs. Воспроизводит раздельный стартовый налог Unicorn из FRouterV2.
//! Кто вызывает: virtuals::market. Читает конкретную высоту, startTime, taxStartTime и isProjectXLaunch.
use super::{
    super::{
        ctx::Ctx,
        node::{multicall, rpc},
    },
    nami_batch as batch,
};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, Bytes, U256};
use serde_json::{Value, json};
sol! {interface ITax {
    function startTime() view returns(uint256);
    function taxStartTime() view returns(uint256);
    function isProjectXLaunch(address token) view returns(bool);
    function antiSniperBuyTaxStartValue() view returns(uint256);
}}
pub(super) async fn current(
    ctx: &Ctx,
    factory: Address,
    pool: Address,
    front: Address,
    token: Address,
    buy: u8,
    at: &Value,
) -> GatewayResult<u8> {
    let wrong = || {
        ctx.err(
            ErrorCode::BadResponse,
            "Unicorn: не подтверждены время или параметры изменения стартового налога",
        )
    };
    let multi = ctx.addr("dex.multicall3")?;
    let optional = multicall::encode(&[
        (pool, ITax::taxStartTimeCall {}.abi_encode().into()),
        (
            front,
            ITax::isProjectXLaunchCall { token }.abi_encode().into(),
        ),
    ]);
    let [start, initial, extras] = batch::read(
        ctx,
        [
            batch::view(at, pool, ITax::startTimeCall {}),
            batch::view(at, factory, ITax::antiSniperBuyTaxStartValueCall {}),
            ("eth_call", json!([{"to":multi,"data":optional},at])),
        ],
    )
    .await?;
    let start = batch::decode(&start, ITax::startTimeCall {})?;
    let initial = batch::decode(&initial, ITax::antiSniperBuyTaxStartValueCall {})?;
    let mut extras = multicall::decode(&extras, 2)?.into_iter();
    let extra = extras
        .next()
        .ok_or_else(wrong)?
        .ok()
        .map(|v: Bytes| ITax::taxStartTimeCall::abi_decode_returns_validate(&v))
        .transpose()
        .map_err(|_| wrong())?
        .unwrap_or_default();
    let project = extras
        .next()
        .ok_or_else(wrong)?
        .ok()
        .map(|v: Bytes| ITax::isProjectXLaunchCall::abi_decode_returns_validate(&v))
        .transpose()
        .map_err(|_| wrong())?
        .unwrap_or(false);
    let block: Value =
        rpc::call(ctx, "eth_getBlockByHash", json!([at["blockHash"], false])).await?;
    if block["hash"] != at["blockHash"] {
        return Err(wrong());
    }
    let timestamp = block["timestamp"]
        .as_str()
        .and_then(|v| v.strip_prefix("0x"))
        .and_then(|v| u64::from_str_radix(v, 16).ok())
        .ok_or_else(wrong)?;
    let elapsed = U256::from(timestamp).saturating_sub(if extra.is_zero() { start } else { extra });
    let tax = initial.saturating_sub(if project {
        elapsed
    } else {
        elapsed / U256::from(60)
    });
    Ok(u8::try_from(tax.min(U256::from(99u8.saturating_sub(buy)))).unwrap_or(99))
}
