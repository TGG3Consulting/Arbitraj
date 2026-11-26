//! Base GasPriceOracle: стоимость публикации неподписанной транзакции и комиссия оператора на одном блоке.
//! Используют котировки, отправка и отмена. https://docs.optimism.io/op-stack/transactions/fees
//! ABI: https://github.com/ethereum-optimism/optimism/blob/develop/packages/contracts-bedrock/src/L2/GasPriceOracle.sol

use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{address, Address, Bytes, U256};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use super::super::ctx::Ctx;
use super::call;

sol! {
    interface IGasPriceOracle {
        function getL1Fee(bytes unsignedTx) external view returns (uint256);
        function getL1FeeUpperBound(uint256 unsignedTxSize) external view returns (uint256);
        function getOperatorFee(uint256 gasUsed) external view returns (uint256);
    }
}

/// Настройки Base фиксируются перед первой отправкой и сохраняются в Watch для отмен с тем же nonce.
#[derive(Clone, Copy)]
pub struct Oracle {
    address: Address,
    max_bytes: u64,
}

impl Oracle {
    /// Адрес predeploy и предел длины неподписанной транзакции из настроек площадки.
    pub fn read(ctx: &Ctx) -> GatewayResult<Self> {
        let address = ctx.addr("dex.op_gas_price_oracle")?;
        if address != address!("420000000000000000000000000000000000000f") {
            return Err(ctx.not_set("dex.op_gas_price_oracle", "для Base требуется канонический GasPriceOracle 0x420000000000000000000000000000000000000F"));
        }
        let max_bytes = ctx.int("dex.op_quote_tx_bytes")?;
        if max_bytes == 0 {
            return Err(ctx.not_set("dex.op_quote_tx_bytes", "задайте положительный предел длины транзакции"));
        }
        Ok(Self { address, max_bytes })
    }

    /// План одной транзакции до сборки. У лимита getL1FeeUpperBound сеть сама учитывает подпись.
    pub async fn planned(&self, ctx: &Ctx, block: u64, gas: u64) -> GatewayResult<U256> {
        let size = IGasPriceOracle::getL1FeeUpperBoundCall { unsignedTxSize: self.max_bytes.into() };
        self.sum(ctx, block, gas, &size).await
    }

    /// Точный неподписанный type-2 envelope; текущий тариф и лимит газа определяют резерв в ETH.
    pub async fn for_tx(&self, ctx: &Ctx, block: u64, gas: u64, raw: Bytes) -> GatewayResult<U256> {
        if raw.len() as u64 > self.max_bytes {
            return Err(ctx.err(ErrorCode::NoRoute, format!("Base: неподписанная транзакция {} байт превышает dex.op_quote_tx_bytes ({})", raw.len(), self.max_bytes)));
        }
        let fee = IGasPriceOracle::getL1FeeCall { unsignedTx: raw };
        self.sum(ctx, block, gas, &fee).await
    }

    /// Обе комиссии читаются на одном явно заданном блоке. Ошибка любого ответа прерывает расчёт.
    async fn sum<C: SolCall<Return = U256>>(&self, ctx: &Ctx, block: u64, gas: u64, l1: &C) -> GatewayResult<U256> {
        let block = format!("{block:#x}");
        let operator = IGasPriceOracle::getOperatorFeeCall { gasUsed: gas.into() };
        let (l1, op) = tokio::try_join!(
            call::view(ctx, self.address, l1, &block),
            call::view(ctx, self.address, &operator, &block)
        )?;
        l1.checked_add(op).ok_or_else(|| ctx.err(ErrorCode::BadResponse,"сумма компонентов комиссии Base больше U256"))
    }
}

/// Расход в плане одной транзакции. Прочие площадки используют существующую модель.
pub async fn planned(ctx: &Ctx, gas_price: &super::super::quote::gas::GasPrice, units: u64) -> GatewayResult<U256> {
    if ctx.place.id != "base_dex" {
        return Ok(U256::ZERO);
    }
    let (block, _) = gas_price.known().ok_or_else(|| ctx.err(ErrorCode::StaleData, "нет блока цены газа Base"))?;
    Oracle::read(ctx)?.planned(ctx, block, units).await
}
