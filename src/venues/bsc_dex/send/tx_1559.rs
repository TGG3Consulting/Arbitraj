//! Файл: venues/bsc_dex/send/tx_1559.rs
//! Что делает: решение 433 (шаг 5 двух сетей) — транзакция EIP-1559 (тип 2) для сети с моделью газа eip1559_l1
//! (Arbitrum One, send/gas_mode.rs): поля maxFeePerGas и maxPriorityFeePerGas (чаевые — ставка PGA секвенсора),
//! chainId — dex.evm_chain_id (решение 416), пустой список доступа. Цена газа, которую заплатит транзакция, — по правилу
//! EIP-1559: min(maxFeePerGas, baseFee + maxPriorityFeePerGas); потолок maxFeePerGas = baseFee × (1 + запас/100) + чаевые.
//! Подпись — wallet/signer.rs sign (уже общий для legacy и EIP-1559: TxEnvelope из Signed<TxEip1559>).
//! Кто вызывает: пока тесты; отправка, отмена (тип 2 с повышением обеих цен), перевод — задача 433-д (docs/TASKS.md).
//! От чего зависит: ctx.rs (ошибки), alloy consensus.
//! Настройки: нет (chainId, цены и запас передаёт вызывающий; их настройки — задача 433-д).
//!
//! EIP-1559 — https://eips.ethereum.org/EIPS/eip-1559 (раздел «Specification»: TransactionType 2, поля chain_id, nonce,
//! max_priority_fee_per_gas, max_fee_per_gas, gas_limit, destination, amount, data, access_list; priority_fee_per_gas =
//! min(max_priority_fee_per_gas, max_fee_per_gas − base_fee_per_gas), effective_gas_price = priority_fee_per_gas +
//! base_fee_per_gas); на Arbitrum One — https://docs.arbitrum.io/how-arbitrum-works/gas-fees («Gas and fees»).

use alloy::consensus::TxEip1559;
use alloy_primitives::{Bytes, TxKind, U256};
use rust_decimal::Decimal;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;

/// Цены газа транзакции типа 2 (wei за единицу газа).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fees {
    /// maxFeePerGas — потолок цены.
    pub max_fee: u128,
    /// maxPriorityFeePerGas — чаевые (ставка PGA).
    pub tip: u128,
}

/// Потолок цены: baseFee × (1 + запас/100) + чаевые, вверх до целого wei. Принимает: базовую цену (wei), запас (%),
/// чаевые (wei). Возвращает: цены транзакции. Ошибки: текст — запас отрицательный или число не помещается в u128.
pub fn fees(base_fee: U256, margin_pct: Decimal, tip: u128) -> Result<Fees, String> {
    if margin_pct.is_sign_negative() {
        return Err(format!("запас цены газа {margin_pct} % отрицательный"));
    }
    let base = u128::try_from(base_fee).map_err(|_| format!("baseFee {base_fee} больше u128"))?;
    let base = i128::try_from(base)
        .ok()
        .and_then(|b| Decimal::try_from_i128_with_scale(b, 0).ok())
        .ok_or_else(|| format!("baseFee {base} не помещается в десятичное число"))?;
    let capped = base
        .checked_mul(Decimal::ONE + margin_pct / Decimal::ONE_HUNDRED)
        .ok_or_else(|| format!("baseFee {base} × запас {margin_pct} % не помещается"))?
        .ceil();
    let capped =
        u128::try_from(capped).map_err(|_| format!("потолок цены {capped} не помещается"))?;
    let max_fee = capped
        .checked_add(tip)
        .ok_or_else(|| format!("потолок цены {capped} + чаевые {tip} не помещаются"))?;
    Ok(Fees { max_fee, tip })
}

/// Цена газа, которую заплатит транзакция (EIP-1559): min(maxFeePerGas, baseFee + чаевые). Принимает: базовую цену
/// блока, цены транзакции. Возвращает: цену или None — потолок ниже базовой цены (транзакция в блок не войдёт).
pub fn effective_price(base_fee: U256, f: Fees) -> Option<U256> {
    let max_fee = U256::from(f.max_fee);
    if max_fee < base_fee {
        return None;
    }
    Some(max_fee.min(base_fee.saturating_add(U256::from(f.tip))))
}

/// Транзакция типа 2. Принимает: окружение (ошибки), chainId (dex.evm_chain_id, прочитанный до отправки), nonce, цены,
/// лимит газа, адрес вызова или TxKind::Create, сумму нативной монеты (wei), данные. Возвращает: транзакцию.
/// Ошибки: BadResponse — чаевые выше потолка цены (EIP-1559: «max_fee_per_gas >= max_priority_fee_per_gas»).
#[allow(clippy::too_many_arguments)]
pub fn eip1559(
    ctx: &Ctx,
    chain: u64,
    nonce: u64,
    f: Fees,
    gas: u64,
    to: impl Into<TxKind>,
    value: U256,
    input: Bytes,
) -> GatewayResult<TxEip1559> {
    build(chain, nonce, f, gas, to.into(), value, input)
        .map_err(|e| ctx.err(ErrorCode::BadResponse, e))
}

/// Сборка без окружения (то же, что eip1559). Ошибка — текст: чаевые выше потолка цены.
pub fn build(
    chain: u64,
    nonce: u64,
    f: Fees,
    gas: u64,
    to: TxKind,
    value: U256,
    input: Bytes,
) -> Result<TxEip1559, String> {
    if f.tip > f.max_fee {
        return Err(format!(
            "Чаевые {} wei выше потолка цены {} wei",
            f.tip, f.max_fee
        ));
    }
    Ok(TxEip1559 {
        chain_id: chain,
        nonce,
        gas_limit: gas,
        max_fee_per_gas: f.max_fee,
        max_priority_fee_per_gas: f.tip,
        to,
        value,
        access_list: Default::default(),
        input,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::bsc_dex::node::arb_gas;
    use crate::venues::bsc_dex::wallet::signer::sign_with;
    use alloy::consensus::{Transaction, TxEnvelope, Typed2718};
    use alloy::eips::eip2718::Decodable2718;
    use alloy::signers::local::PrivateKeySigner;
    use alloy_primitives::{address, hex};

    /// Стоимость исполнения и резерв type-2 по двум реальным квитанциям Ethereum mainnet.
    #[test]
    fn ethereum_mainnet_plan_and_reserve() {
        let r: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/data/base_ethereum_gas_2026-11-26/ethereum-type2.json"
        ))
        .unwrap();
        for row in r["txs"].as_array().unwrap() {
            let n = |key: &str| row[key].as_u64().unwrap();
            let base = U256::from(n("base_fee_wei"));
            let f = Fees {
                max_fee: n("max_fee_per_gas_wei").into(),
                tip: n("max_priority_fee_per_gas_wei").into(),
            };
            let effective = effective_price(base, f).unwrap();
            assert_eq!(effective, U256::from(n("effective_gas_price_wei")));
            assert_eq!(
                effective * U256::from(n("gas_used")),
                U256::from(n("fee_wei"))
            );
            assert_eq!(
                U256::from(f.max_fee) * U256::from(n("gas_limit")),
                U256::from(n("cap_reserve_wei"))
            );
        }
    }

    /// Базовая цена и газ — из записи своего узла Arbitrum One (05_*.json), chainId — 01_eth_chainId.json (0xa4b1).
    fn recorded() -> (arb_gas::Components, u64) {
        let r: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/data/arbitrum_node_2026-11-19/05_nodeinterface_gasEstimateComponents.json"
        ))
        .unwrap();
        let out = hex::decode(r["response"]["result"].as_str().unwrap()).unwrap();
        let c: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/data/arbitrum_node_2026-11-19/01_eth_chainId.json"
        ))
        .unwrap();
        let chain = u64::from_str_radix(c["result"].as_str().unwrap().trim_start_matches("0x"), 16)
            .unwrap();
        (arb_gas::decode(&out).unwrap(), chain)
    }

    /// Потолок = baseFee × 1,25 + чаевые; цена — baseFee + чаевые (ниже потолка); потолок ниже базы — None.
    #[test]
    fn fees_and_effective_price() {
        let (c, _) = recorded();
        let f = fees(c.base_fee, Decimal::from(25), 1_000).unwrap();
        // 0x1346150 = 20 210 000 wei; × 1,25 = 25 262 500.
        assert_eq!(f.max_fee, 25_262_500 + 1_000);
        assert_eq!(
            effective_price(c.base_fee, f),
            Some(c.base_fee + U256::from(1_000u64))
        );
        let low = Fees { max_fee: 1, tip: 0 };
        assert_eq!(effective_price(c.base_fee, low), None);
        assert!(fees(c.base_fee, Decimal::from(-1), 0).is_err());
        assert!(
            build(
                1,
                0,
                Fees { max_fee: 1, tip: 2 },
                21_000,
                TxKind::Create,
                U256::ZERO,
                Bytes::new()
            )
            .is_err()
        );
    }

    /// Подписанная транзакция — тип 2, chainId сети из записи, поля читаются обратно из байт EIP-2718.
    #[test]
    fn signs_as_type_2() {
        let (c, chain) = recorded();
        assert_eq!(chain, 42161);
        let f = fees(c.base_fee, Decimal::from(25), 1_000).unwrap();
        let to = address!("0x0000000000000000000000000000000000000002");
        let tx = build(
            chain,
            7,
            f,
            c.gas_estimate,
            TxKind::Call(to),
            U256::ZERO,
            Bytes::new(),
        )
        .unwrap();
        let key: PrivateKeySigner =
            "0x4646464646464646464646464646464646464646464646464646464646464646"
                .parse()
                .unwrap();
        let signed = sign_with(&key, tx).unwrap();
        assert_eq!(signed.raw[0], 0x02);
        let mut buf: &[u8] = &signed.raw;
        let back = TxEnvelope::decode_2718(&mut buf).unwrap();
        assert_eq!(back.ty(), 2);
        assert_eq!(back.chain_id(), Some(42161));
        assert_eq!(back.nonce(), 7);
        assert_eq!(back.max_fee_per_gas(), f.max_fee);
        assert_eq!(back.max_priority_fee_per_gas(), Some(f.tip));
        assert_eq!(back.gas_limit(), 0x5765);
        assert_eq!(*back.tx_hash(), signed.hash);
    }
}
