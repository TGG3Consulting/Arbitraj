//! ABI подтверждённых размещений OasisSwap и опциональная котировка скидки фактического пользователя.
use super::nami_batch as batch;
use crate::{
    gateway::errors::{ErrorCode, GatewayResult},
    venues::bsc_dex::{
        ctx::Ctx,
        node::{multicall, rpc},
    },
};
use alloy::{sol, sol_types::SolCall};
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::{Value, json};
sol! {
    interface IOasis {
        function factory() external view returns(address);
        function WETH() external view returns(address);
        function getPair(address,address) external view returns(address);
        function pairCodeHash() external view returns(bytes32);
        function fee() external view returns(uint64);
        function MAX_REBATE() external view returns(uint64);
        function FEE_DIVISOR() external view returns(uint64);
        function oasisFeeProportion() external view returns(uint64);
        function feeCache0() external view returns(uint256);
        function feeCache1() external view returns(uint256);
        function getReserves() external view returns(uint112 r0,uint112 r1,uint32 stamp);
        function balanceOf(address account) external view returns(uint256);
        function isRebateApprovedRouter(address router) external view returns(bool);
        function rebateEstimator() external view returns(address);
        function getRebate(address user) external view returns(uint64);
        function calculateFee(uint64 rebate) external view returns(uint256);
    }
}

pub(super) fn hash(ctx: &Ctx, id: &str, key: &str) -> GatewayResult<B256> {
    ctx.dex_text(key, id)?
        .parse::<B256>()
        .ok()
        .filter(|h| *h != B256::ZERO && *h != keccak256([]))
        .ok_or_else(|| ctx.not_set(key, "Нужен keccak256 развёрнутого кода OasisSwap"))
}

/// Фабрика определяет возврат контроллеру сделки. Возможный EVM revert — только отказ опционального режима.
pub(super) async fn rebate(
    ctx: &Ctx,
    factory: Address,
    pool: Address,
    tag: &Value,
    owner: Address,
    base: u32,
    module: Option<Address>,
) -> GatewayResult<Option<u32>> {
    let ask = IOasis::getRebateCall { user: owner };
    let data = multicall::encode(&[(factory, ask.abi_encode().into())]);
    let target = ctx.addr("dex.multicall3")?;
    let bytes: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":target,"from":owner,"data":data},tag]),
    )
    .await?;
    let Some(Ok(first)) = multicall::decode(&bytes, 1)?.pop() else {
        return Ok(None);
    };
    let value = batch::decode(&first, ask)?;
    if value > 10000 {
        if let Some(adapter) = module {
            // Исторический принятый runtime вызывает calculateFee даже для 10001.
            let cap = IOasis::MAX_REBATECall {};
            let [reply] = batch::read(ctx, [batch::view(tag, adapter, cap.clone())]).await?;
            if batch::decode(&reply, cap)? != 10_000 {
                return Err(ctx.err(
                    ErrorCode::NotApplicable,
                    "OasisSwap: модуль не подтверждает предел скидки",
                ));
            }
        }
        // Нативный router с useRebate=false и принятый модуль со своим ограничением берут fee().
        return Ok(None);
    }
    let calc = IOasis::calculateFeeCall { rebate: value };
    let bytes: Bytes = rpc::call(
        ctx,
        "eth_call",
        json!([{"to":pool,"from":owner,"data":Bytes::from(calc.abi_encode())},tag]),
    )
    .await?;
    let expected = base
        * (10000
            - u32::try_from(value)
                .map_err(|_| ctx.err(ErrorCode::BadResponse, "OasisSwap: ставка вне диапазона"))?)
        / 10000;
    if batch::decode(&bytes, calc)? != U256::from(expected) {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            "OasisSwap: расчёт скидки расходится с парой",
        ));
    }
    Ok(Some(expected))
}
