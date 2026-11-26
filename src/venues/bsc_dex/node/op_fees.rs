//! Base GasPriceOracle: стоимость публикации неподписанной транзакции и комиссия оператора на одном блоке.
//! Используют котировки, отправка и отмена. https://docs.optimism.io/op-stack/transactions/fees
//! ABI: https://github.com/ethereum-optimism/optimism/blob/develop/packages/contracts-bedrock/src/L2/GasPriceOracle.sol

use super::super::ctx::Ctx;
use super::call;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use alloy::sol;
use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256, address};

sol! {
    interface IGasPriceOracle {
        function getL1Fee(bytes unsignedTx) external view returns (uint256);
        function getL1FeeUpperBound(uint256 unsignedTxSize) external view returns (uint256);
        function getOperatorFee(uint256 gasUsed) external view returns (uint256);
    }
}

/// Настройки Base фиксируются перед первой отправкой и сохраняются в Watch для отмен с тем же nonce.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Oracle {
    address: Address,
    max_bytes: u64,
}

/// Снимок на блок. GasPriceOracle до Jovian делит произведение на 1e6, после Jovian умножает на 100.
/// Два ответа оракула определяют одинаковую рациональную ставку в обоих случаях.
#[derive(Clone, Copy)]
pub struct Tariff {
    pub oracle: Oracle,
    l1: U256,
    constant: U256,
    per_million: U256,
}

impl Tariff {
    /// Газ нескольких транзакций суммируется. Однократные комиссии учитываются на каждую.
    /// При дробной ставке floor суммы даёт верхнюю оценку суммы отдельных округлений.
    pub fn extra(&self, gas: u64, transactions: usize) -> U256 {
        let fixed = self
            .l1
            .saturating_add(self.constant)
            .saturating_mul(U256::from(transactions));
        fixed.saturating_add(
            self.per_million.saturating_mul(U256::from(gas)) / U256::from(1_000_000),
        )
    }
}

impl Oracle {
    /// Одна выборка на блок для всех размеров маршрута. Стоимость неподписанного envelope ограничена настройкой.
    pub async fn tariff(&self, ctx: &Ctx, number: u64) -> GatewayResult<Tariff> {
        use IGasPriceOracle::{getL1FeeUpperBoundCall, getOperatorFeeCall};
        let block = format!("{number:#x}");
        let size = getL1FeeUpperBoundCall {
            unsignedTxSize: U256::from(self.max_bytes),
        };
        let zero = getOperatorFeeCall {
            gasUsed: U256::ZERO,
        };
        let million = getOperatorFeeCall {
            gasUsed: U256::from(1_000_000),
        };
        let (l1, constant, fee) = tokio::try_join!(
            call::view(ctx, self.address, &size, &block),
            call::view(ctx, self.address, &zero, &block),
            call::view(ctx, self.address, &million, &block)
        )?;
        let per_million = fee.checked_sub(constant).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "отрицательная ставка operator fee Base",
            )
        })?;
        Ok(Tariff {
            oracle: *self,
            l1,
            constant,
            per_million,
        })
    }

    /// Адрес predeploy и предел длины неподписанной транзакции из настроек площадки.
    pub fn read(ctx: &Ctx) -> GatewayResult<Self> {
        let address = ctx.addr("dex.op_gas_price_oracle")?;
        if address != address!("420000000000000000000000000000000000000f") {
            return Err(ctx.not_set("dex.op_gas_price_oracle", "для Base требуется канонический GasPriceOracle 0x420000000000000000000000000000000000000F"));
        }
        let max_bytes = ctx.int("dex.op_quote_tx_bytes")?;
        if max_bytes == 0 {
            return Err(ctx.not_set(
                "dex.op_quote_tx_bytes",
                "задайте положительный предел длины транзакции",
            ));
        }
        Ok(Self { address, max_bytes })
    }

    /// План одной транзакции до сборки. У лимита getL1FeeUpperBound сеть сама учитывает подпись.
    pub async fn planned(&self, ctx: &Ctx, block: u64, gas: u64) -> GatewayResult<U256> {
        let size = IGasPriceOracle::getL1FeeUpperBoundCall {
            unsignedTxSize: U256::from(self.max_bytes),
        };
        self.sum(ctx, block, gas, &size).await
    }

    /// Точный неподписанный type-2 envelope; текущий тариф и лимит газа определяют резерв в ETH.
    pub async fn for_tx(
        &self,
        ctx: &Ctx,
        block: u64,
        gas: u64,
        raw: Bytes,
        creation: bool,
    ) -> GatewayResult<U256> {
        // Код конструктора известен при оценке деплоя. Его размер не определяет стоимость торговых маршрутов.
        let limit = if creation {
            raw.len() as u64
        } else {
            self.max_bytes
        };
        if raw.len() as u64 > limit {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                format!(
                    "Base: неподписанная транзакция {} байт превышает dex.op_quote_tx_bytes ({})",
                    raw.len(),
                    self.max_bytes
                ),
            ));
        }
        let fee = IGasPriceOracle::getL1FeeCall { unsignedTx: raw };
        let tag = format!("{block:#x}");
        let upper = IGasPriceOracle::getL1FeeUpperBoundCall {
            unsignedTxSize: U256::from(limit),
        };
        let op = IGasPriceOracle::getOperatorFeeCall {
            gasUsed: U256::from(gas),
        };
        let (l1, bound, operator) = tokio::try_join!(
            call::view(ctx, self.address, &fee, &tag),
            call::view(ctx, self.address, &upper, &tag),
            call::view(ctx, self.address, &op, &tag)
        )?;
        if l1 > bound {
            return Err(ctx.err(ErrorCode::NoRoute,format!("оценка публикации Base {l1} wei выше принятой при расчёте на этом блоке {bound} wei")));
        }
        l1.checked_add(operator)
            .ok_or_else(|| ctx.err(ErrorCode::BadResponse, "сумма комиссий Base больше U256"))
    }

    /// Обе комиссии читаются на одном явно заданном блоке. Ошибка любого ответа прерывает расчёт.
    async fn sum<C: SolCall<Return = U256>>(
        &self,
        ctx: &Ctx,
        block: u64,
        gas: u64,
        l1: &C,
    ) -> GatewayResult<U256> {
        let block = format!("{block:#x}");
        let operator = IGasPriceOracle::getOperatorFeeCall {
            gasUsed: U256::from(gas),
        };
        let (l1, op) = tokio::try_join!(
            call::view(ctx, self.address, l1, &block),
            call::view(ctx, self.address, &operator, &block)
        )?;
        l1.checked_add(op).ok_or_else(|| {
            ctx.err(
                ErrorCode::BadResponse,
                "сумма компонентов комиссии Base больше U256",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::send::tx_1559::{Fees, build};
    use alloy::consensus::SignableTransaction;
    use alloy_primitives::{TxKind, hex};
    use serde_json::Value;

    /// Восстановленный unsigned envelope реальной транзакции Base и все вызовы оракула на её блоке.
    #[test]
    fn recorded_base_unsigned_and_oracle() {
        let rec: Value = serde_json::from_str(include_str!(
            "../../../../tests/data/base_ethereum_gas_2026-11-26/base-oracle.json"
        ))
        .unwrap();
        let n = |k: &str| {
            u64::from_str_radix(rec[k].as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
        };
        let fees = Fees {
            tip: n("maxPriorityFeePerGas").into(),
            max_fee: n("maxFeePerGas").into(),
        };
        let tx = build(
            n("chainId"),
            n("nonce"),
            fees,
            n("gas"),
            TxKind::Call(rec["to"].as_str().unwrap().parse().unwrap()),
            U256::from(n("value")),
            rec["input"].as_str().unwrap().parse().unwrap(),
        )
        .unwrap();
        let unsigned: Bytes = tx.encoded_for_signing().into();
        assert_eq!(
            hex::encode_prefixed(&unsigned),
            rec["unsigned"].as_str().unwrap()
        );
        let rows = rec["calls"].as_array().unwrap();
        let args = [
            IGasPriceOracle::getL1FeeCall {
                unsignedTx: unsigned.clone(),
            }
            .abi_encode(),
            IGasPriceOracle::getL1FeeUpperBoundCall {
                unsignedTxSize: U256::from(unsigned.len()),
            }
            .abi_encode(),
            IGasPriceOracle::getOperatorFeeCall {
                gasUsed: U256::from(n("gasUsed")),
            }
            .abi_encode(),
            IGasPriceOracle::getOperatorFeeCall {
                gasUsed: U256::ZERO,
            }
            .abi_encode(),
            IGasPriceOracle::getOperatorFeeCall {
                gasUsed: U256::from(1_000_000u64),
            }
            .abi_encode(),
        ];
        for (row, arg) in rows.iter().zip(args) {
            assert_eq!(hex::encode_prefixed(arg), row["input"].as_str().unwrap());
        }
        let value = |i: usize| {
            U256::from_str_radix(
                rows[i]["result"].as_str().unwrap().trim_start_matches("0x"),
                16,
            )
            .unwrap()
        };
        let (l1, upper, operator) = (value(0), value(1), value(2));
        let oracle = Oracle {
            address: rec["oracle"].as_str().unwrap().parse().unwrap(),
            max_bytes: unsigned.len() as u64,
        };
        let tariff = Tariff {
            oracle,
            l1: upper,
            constant: value(3),
            per_million: value(4) - value(3),
        };
        assert_eq!(l1, U256::from(753750885u64));
        assert_eq!(tariff.extra(n("gasUsed"), 1), upper + operator);
        assert!(l1 < upper);
    }
}
