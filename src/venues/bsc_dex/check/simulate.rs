//! Файл: venues/bsc_dex/check/simulate.rs
//! Что делает: eth_simulateV1 — последовательность вызовов в одном блоке поверх состояния заданного блока, без
//! транзакций в сети и без подписи (validation = false): так меряются налоги и газ пробным обменом (решение 30).
//! Подмена состояния (stateOverrides, run_with) — сухая сделка через неразвёрнутый исполнитель (решение 122, часть X).
//! Кто вызывает: check/probe.rs, exec/precheck_sim.rs, exec/dry_sim.rs, exec/executor/sim.rs.
//! От чего зависит: node/rpc.rs, ctx.rs, check/sim_base.rs (Base — номер и время базового блока).
//! Настройки: узел — bsc.node_http_url (метод должен поддерживаться провайдером; не поддерживается — ошибка узла
//! как есть, проверка монеты не проходит).
//!
//! Метод, поля и ответ — https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/execute.yaml#L137-L260 ,
//! схемы — src/schemas/execute.yaml (EthSimulatePayload, BlockStateCalls, CallResult: status, returnData, gasUsed, error).
//! В узле BSC — BlockChainAPI.SimulateV1: https://github.com/bnb-chain/bsc/blob/947ba1cb1ef05d4a5f71b738113a3d8b4c16f6b7/internal/ethapi/api.go#L1142 .

use alloy_primitives::{Address, B256, Bytes, U64, U256};
use serde::Deserialize;
use serde_json::json;

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::node::rpc;

pub use super::sim_base::Base;

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

/// Выполняет вызовы по порядку в одном симулированном блоке поверх базового блока (Base или номер).
/// Принимает: вызовы, базовый блок. Возвращает: итог каждого вызова в том же порядке.
/// Ошибки: узел (метод не поддерживается — его текст), BadResponse (число итогов не совпало).
pub async fn run(
    ctx: &Ctx,
    calls: &[Call],
    base: impl Into<Base>,
) -> GatewayResult<Vec<CallResult>> {
    run_valued(ctx, calls, &[], base).await
}

/// То же с суммой BNB у вызовов (поле value объекта вызова — GenericTransaction, execution-apis
/// src/schemas/transaction.yaml): нативная монета (решение 62) — WRAP_ETH с value, перевод BNB.
/// Принимает: вызовы, суммы по номеру вызова (wei; нет в списке — без value), номер базового блока. Ошибки: как у run.
pub async fn run_valued(
    ctx: &Ctx,
    calls: &[Call],
    values: &[(usize, U256)],
    base: impl Into<Base>,
) -> GatewayResult<Vec<CallResult>> {
    run_with(ctx, calls, values, &[], base).await
}

/// Подмена состояния счёта в симуляции (execute.yaml AccountOverride: code и state — полная замена хранилища):
/// так «Сухая сделка DEX» проверяет ещё не развёрнутый исполнитель ArbExecutor на живой сети (решение 122, часть X).
#[derive(Debug, Clone)]
pub struct Override {
    pub address: Address,
    pub code: Bytes,
    /// Слоты хранилища (ключ → значение); остальные слоты — нули.
    pub state: Vec<(B256, B256)>,
}

/// То же с подменой состояния (blockStateCalls[].stateOverrides — execution-apis 16caee27 src/schemas/execute.yaml
/// L40-L99). Принимает: вызовы, суммы BNB, подмены, номер базового блока. Ошибки: как у run.
pub async fn run_with(
    ctx: &Ctx,
    calls: &[Call],
    values: &[(usize, U256)],
    overrides: &[Override],
    base: impl Into<Base>,
) -> GatewayResult<Vec<CallResult>> {
    let base = base.into();
    let list: Vec<_> = calls
        .iter()
        .enumerate()
        .map(|(i, c)| match values.iter().find(|(n, _)| *n == i) {
            Some((_, v)) => json!({ "from": c.from, "to": c.to, "input": c.input, "value": v }),
            None => json!({ "from": c.from, "to": c.to, "input": c.input }),
        })
        .collect();
    let mut block = json!({ "calls": list });
    if !overrides.is_empty() {
        let map: serde_json::Map<String, serde_json::Value> = overrides
            .iter()
            .map(|o| {
                let state: serde_json::Map<String, serde_json::Value> = o
                    .state
                    .iter()
                    .map(|(k, v)| (format!("{k:#x}"), json!(v)))
                    .collect();
                (
                    format!("{:#x}", o.address),
                    json!({ "code": o.code, "state": state }),
                )
            })
            .collect();
        block["stateOverrides"] = serde_json::Value::Object(map);
    }
    if let Some(t) = base.time {
        block["blockOverrides"] = json!({ "time": U64::from(t) });
    }
    let payload = json!({ "blockStateCalls": [block], "validation": false });
    let blocks: Vec<BlockResult> = rpc::call(
        ctx,
        "eth_simulateV1",
        json!([payload, block_tag(base.number)]),
    )
    .await?;
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
