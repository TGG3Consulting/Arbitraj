//! Файл: venues/bsc_dex/check/simulate.rs
//! Что делает: eth_simulateV1 — последовательность вызовов в одном блоке поверх состояния заданного блока, без
//! транзакций в сети и без подписи (validation = false): так меряются налоги и газ пробным обменом (решение 30).
//! Кто вызывает: check/probe.rs, exec/precheck_sim.rs.
//! От чего зависит: node/rpc.rs, ctx.rs.
//! Настройки: узел — bsc.node_http_url (метод должен поддерживаться провайдером; не поддерживается — ошибка узла
//! как есть, проверка монеты не проходит).
//!
//! Метод, поля и ответ — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/execute.yaml#L137-L260 ,
//! схемы — src/schemas/execute.yaml (EthSimulatePayload, BlockStateCalls, CallResult: status, returnData, gasUsed, error).
//! В узле BSC — BlockChainAPI.SimulateV1: https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/internal/ethapi/api.go#L1142 .

use alloy_primitives::{Address, Bytes, U64};
use serde::Deserialize;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::node::rpc;

/// Один вызов последовательности.
#[derive(Debug, Clone)]
pub struct Call {
    pub from: Address,
    pub to: Address,
    pub input: Bytes,
}

/// Итог вызова.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallResult {
    /// 0x1 — успех, 0x0 — откат.
    pub status: U64,
    pub return_data: Bytes,
    pub gas_used: U64,
    #[serde(default)]
    pub error: Option<CallError>,
    /// События вызова (execute.yaml CallResult.logs) — по ним видно, какой контракт сделал обмен (check/emitter.rs).
    #[serde(default)]
    pub logs: Vec<SimLog>,
}

/// Событие вызова: контракт и темы (остальные поля не нужны).
#[derive(Debug, Clone, Deserialize)]
pub struct SimLog {
    pub address: Address,
    #[serde(default)]
    pub topics: Vec<alloy_primitives::B256>,
}

/// Ошибка вызова (откат).
#[derive(Debug, Clone, Deserialize)]
pub struct CallError {
    pub code: i64,
    pub message: String,
}

#[derive(Deserialize)]
struct BlockResult {
    calls: Vec<CallResult>,
}

impl CallResult {
    /// Успешен ли вызов. Ошибок нет.
    pub fn ok(&self) -> bool {
        self.status == U64::from(1u8)
    }

    /// Причина отката для журнала. Ошибок нет.
    pub fn why(&self) -> String {
        match &self.error {
            Some(e) => format!("код {}: {}", e.code, e.message),
            None => "откат без текста".into(),
        }
    }
}

/// Выполняет вызовы по порядку в одном симулированном блоке поверх блока `base` (номер, а не «latest»: срок
/// транзакции считан по времени этого же блока, решение 61). Время симулированного блока узел ставит сам: у BSC —
/// время базового + timestampIncrement = 12 с (bsc 947ba1cb internal/ethapi/simulate.go#L56-L57, #L577-L583), поэтому
/// dex.deadline_s должен быть больше 12 с — иначе откат TransactionDeadlinePassed («не проверено», повтор).
/// Принимает: вызовы, номер базового блока. Возвращает: итог каждого вызова в том же порядке.
/// Ошибки: узел (метод не поддерживается — его текст), BadResponse (число итогов не совпало).
pub async fn run(ctx: &Ctx, calls: &[Call], base: u64) -> GatewayResult<Vec<CallResult>> {
    let list: Vec<_> = calls
        .iter()
        .map(|c| json!({ "from": c.from, "to": c.to, "input": c.input }))
        .collect();
    let payload = json!({ "blockStateCalls": [{ "calls": list }], "validation": false });
    let blocks: Vec<BlockResult> =
        rpc::call(ctx, "eth_simulateV1", json!([payload, block_tag(base)])).await?;
    let out = blocks
        .into_iter()
        .next()
        .map(|b| b.calls)
        .unwrap_or_default();
    if out.len() != calls.len() {
        return Err(ctx.err(
            ErrorCode::BadResponse,
            format!(
                "eth_simulateV1: вызовов {}, итогов {}",
                calls.len(),
                out.len()
            ),
        ));
    }
    Ok(out)
}
