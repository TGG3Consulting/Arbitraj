//! Файл: venues/bsc_dex/node/arb_gas.rs
//! Что делает: решение 433 (шаг 5 двух сетей) — газ сети Arbitrum с частью L1: вызов NodeInterface
//! gasEstimateComponents(to, contractCreation, data) через eth_call и разбор его четырёх значений — gasEstimate
//! (весь газ: L2 + L1 в единицах газа L2; равен eth_estimateGas — записи 04 и 05 своего узла), gasEstimateForL1
//! (часть L1 — плата за данные пакета на L1), baseFee (базовая цена газа L2, wei) и l1BaseFeeEstimate (цена L1, wei).
//! Стоимость газа по правилу EIP-1559: газ × min(maxFeePerGas, baseFee + maxPriorityFeePerGas) — send/tx_1559.rs.
//! Адрес NodeInterface — настройка площадки dex.node_interface (не код). Работает только при dex.gas_mode = eip1559_l1
//! (send/gas_mode.rs); у BSC (legacy) не вызывается.
//! Кто вызывает: connection_gas.rs («Проверить подключение», часть «Газ сети»); отправка и котировка — задача 433-д
//! (docs/TASKS.md, «Решение 433»).
//! От чего зависит: node/rpc.rs, node/call.rs (разбор по ABI), ctx.rs, alloy sol!.
//! Настройки: dex.node_interface [площадка DEX].
//!
//! Официально: NodeInterface — виртуальный контракт узла Nitro по адресу 0x…C8, gasEstimateComponents —
//! https://docs.arbitrum.io/build-decentralized-apps/nodeinterface/reference («Use eth_call to call»); объявление с
//! возвращаемыми значениями — OffchainLabs/nitro-contracts, src/node-interface/NodeInterface.sol («gasEstimate an
//! estimate of the total amount of gas needed for this tx», «gasEstimateForL1 … the l1 component», «baseFee the l2 base
//! fee»); плата = базовая цена L2 × (газ L2 + плата L1 в газе L2), чаевые = min(maxPriorityFeePerGas, maxFeePerGas −
//! baseFee) — https://docs.arbitrum.io/how-arbitrum-works/gas-fees («Gas and fees»; выписка —
//! docs/notes/arbitrum/docs-check-2026-11-20.md). Сигнатура сверена с записью своего узла: селектор 0xc94e6eeb и тело —
//! tests/data/arbitrum_node_2026-11-19/05_nodeinterface_gasEstimateComponents.json (тест ниже).

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes, U256};
use serde_json::json;

use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::call;
use super::rpc;

alloy::sol! {
    /// NodeInterface Nitro (docs.arbitrum.io, NodeInterface reference): оценка газа по частям.
    interface INodeInterface {
        function gasEstimateComponents(address to, bool contractCreation, bytes data)
            external
            payable
            returns (uint64 gasEstimate, uint64 gasEstimateForL1, uint256 baseFee, uint256 l1BaseFeeEstimate);
    }
}

/// Части оценки газа Arbitrum (ответ gasEstimateComponents).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Components {
    /// Весь газ транзакции в единицах газа L2 (L2 + L1).
    pub gas_estimate: u64,
    /// Из него — часть L1 (данные пакета на L1), в единицах газа L2.
    pub gas_for_l1: u64,
    /// Базовая цена газа L2, wei.
    pub base_fee: U256,
    /// Оценка цены L1 (wei за единицу) — для журнала и разбора.
    pub l1_base_fee: U256,
}

impl Components {
    /// Газ исполнения на L2 без части L1. Ошибок нет (часть L1 больше всего — 0, узел так не отвечает).
    pub fn l2_gas(&self) -> u64 {
        self.gas_estimate.saturating_sub(self.gas_for_l1)
    }
}

/// Тело вызова gasEstimateComponents. Принимает: адрес вызова (для развёртывания — любой, create = true), признак
/// развёртывания, данные транзакции. Возвращает: байты eth_call. Ошибок нет.
pub fn calldata(to: Address, create: bool, data: Bytes) -> Bytes {
    Bytes::from(
        INodeInterface::gasEstimateComponentsCall {
            to,
            contractCreation: create,
            data,
        }
        .abi_encode(),
    )
}

/// Разбор ответа gasEstimateComponents. Принимает: байты result eth_call. Возвращает: части газа.
/// Ошибки: BadResponse — ответ не по ABI.
pub fn decode(out: &[u8]) -> GatewayResult<Components> {
    let r = call::decode::<INodeInterface::gasEstimateComponentsCall>(out)?;
    Ok(Components {
        gas_estimate: r.gasEstimate,
        gas_for_l1: r.gasEstimateForL1,
        base_fee: r.baseFee,
        l1_base_fee: r.l1BaseFeeEstimate,
    })
}

/// Части газа транзакции у своего узла: eth_call к NodeInterface (dex.node_interface) от отправителя на блоке latest.
/// Принимает: окружение, отправителя, адрес вызова, признак развёртывания, данные. Возвращает: части газа.
/// Ошибки: NotConfigured — dex.node_interface не задана; откат (текст узла); связь с узлом; BadResponse.
pub async fn components(
    ctx: &Ctx,
    from: Address,
    to: Address,
    create: bool,
    data: Bytes,
) -> GatewayResult<Components> {
    let ni = ctx.addr("dex.node_interface")?;
    // Поле «data» — как в записи своего узла (05_*.json), блок latest.
    let req = json!([{ "from": from, "to": ni, "data": calldata(to, create, data) }, "latest"]);
    let out: Bytes = rpc::call(ctx, "eth_call", req).await?;
    decode(&out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, hex};

    /// Запись своего узла Arbitrum One (Nitro v3.12.4), 19.11.2026.
    const REC: &str = include_str!(
        "../../../../tests/data/arbitrum_node_2026-11-19/05_nodeinterface_gasEstimateComponents.json"
    );

    fn rec() -> serde_json::Value {
        serde_json::from_str(REC).unwrap()
    }

    /// Селектор и тело запроса совпадают с записанными байт в байт.
    #[test]
    fn calldata_matches_recorded_request() {
        let r = rec();
        assert_eq!(
            hex::encode_prefixed(INodeInterface::gasEstimateComponentsCall::SELECTOR),
            r["selector"].as_str().unwrap()
        );
        let to = address!("0x0000000000000000000000000000000000000002");
        let got = hex::encode_prefixed(calldata(to, false, Bytes::new()));
        assert_eq!(got, r["request"]["params"][0]["data"].as_str().unwrap());
    }

    /// Разбор записанного ответа: 0x5765 газа (= eth_estimateGas, запись 04), из них 0x3d3 — L1.
    #[test]
    fn decode_recorded_response() {
        let r = rec();
        let out = hex::decode(r["response"]["result"].as_str().unwrap()).unwrap();
        let c = decode(&out).unwrap();
        assert_eq!(c.gas_estimate, 0x5765);
        assert_eq!(c.gas_for_l1, 0x3d3);
        assert_eq!(c.base_fee, U256::from(0x0134_6150u64));
        assert_eq!(c.l1_base_fee, U256::from(0x0076_7008u64));
        assert_eq!(c.l2_gas(), 0x5765 - 0x3d3);
        let est: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../tests/data/arbitrum_node_2026-11-19/04_eth_estimateGas.json"
        ))
        .unwrap();
        assert_eq!(
            u64::from_str_radix(est["result"].as_str().unwrap().trim_start_matches("0x"), 16)
                .unwrap(),
            c.gas_estimate
        );
    }
}
