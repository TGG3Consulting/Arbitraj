//! Файл: venues/bsc_dex/exec/pool_cond_row.rs
//! Что делает: решение 438 — итог чтения условия пулов (PoolConds: блок плана, knownAccounts, строка по каждому пулу
//! «в условии / без условия и почему», время чтения), разбор ответов eth_getStorageAt / eth_getProof и строка для
//! журнала send.pool_cond и карточки сделки (TradeResult.executor).
//! Кто вызывает: exec/pool_cond_read.rs (сборка), exec/cycle_exec.rs (журнал, отправка, карточка).
//! От чего зависит: exec/pool_cond.rs (How), ctx.rs.
//! Настройки: нет.
//!
//! Форматы ответов (nitro-geth 260bc5dcbd5a16925d7e26145577b2612f30f428): eth_getStorageAt — hexutil.Bytes 32 байта
//! (internal/ethapi/api.go:617-618); eth_getProof — AccountResult.storageHash (internal/ethapi/api.go:336-344).

use alloy_primitives::B256;
use serde::Deserialize;
use serde_json::Map;
use serde_json::value::RawValue;

use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::pool_cond::How;

/// Пул маршрута: подпись и что поставлено в условие (Ok) или почему без условия (Err).
#[derive(Debug, Clone)]
pub struct Row {
    pub pool: String,
    pub cond: Result<String, String>,
}

/// Условие пулов маршрута на блоке плана.
#[derive(Debug, Clone)]
pub struct PoolConds {
    /// Блок плана: на нём перечитаны пулы и прочитаны значения.
    pub block: u64,
    /// knownAccounts: адрес → корень хранилища или {слот: значение}; пусто — условий пулов нет.
    pub known: Map<String, serde_json::Value>,
    pub rows: Vec<Row>,
    /// Сколько шло чтение, мс.
    pub read_ms: u128,
}

/// Корень из eth_getProof.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Proof {
    storage_hash: B256,
}

/// Слово слота из ответа eth_getStorageAt (32 байта). Ошибки: BadResponse.
pub fn word(ctx: &Ctx, v: &RawValue) -> GatewayResult<B256> {
    serde_json::from_str(v.get()).map_err(|e| {
        let why = format!("eth_getStorageAt: не слово 32 байта: {e}");
        ctx.err(ErrorCode::BadResponse, why)
    })
}

/// Корень хранилища из ответа eth_getProof. Ошибки: BadResponse.
pub fn root(ctx: &Ctx, v: &RawValue) -> GatewayResult<B256> {
    let p: Proof = serde_json::from_str(v.get()).map_err(|e| {
        let why = format!("eth_getProof: нет storageHash: {e}");
        ctx.err(ErrorCode::BadResponse, why)
    })?;
    Ok(p.storage_hash)
}

/// Условие не прочитано: все пулы без условия с одной причиной (кроме Off — их причина своя). Ошибок нет.
pub fn failed(block: u64, rules: &[(String, How)], why: &str, read_ms: u128) -> PoolConds {
    let rows = rules
        .iter()
        .map(|(pool, h)| Row {
            pool: pool.clone(),
            cond: Err(match h {
                How::Off(w) => (*w).to_string(),
                _ => why.to_string(),
            }),
        })
        .collect();
    PoolConds {
        block,
        known: Map::new(),
        rows,
        read_ms,
    }
}

impl PoolConds {
    /// Строка для журнала и карточки сделки: блок, время чтения, пулы в условии и без него с причиной. Ошибок нет.
    pub fn summary(&self) -> String {
        let on: Vec<String> = self
            .rows
            .iter()
            .filter_map(|r| r.cond.as_ref().ok().map(|w| format!("{} — {w}", r.pool)))
            .collect();
        let off: Vec<String> = self
            .rows
            .iter()
            .filter_map(|r| r.cond.as_ref().err().map(|w| format!("{} — {w}", r.pool)))
            .collect();
        let mut s = format!(
            "условие пулов (блок плана {}, чтение {} мс): в условии {} из {}",
            self.block,
            self.read_ms,
            on.len(),
            self.rows.len()
        );
        if !on.is_empty() {
            s.push_str(&format!(" [{}]", on.join("; ")));
        }
        if !off.is_empty() {
            s.push_str(&format!("; без условия: [{}]", off.join("; ")));
        }
        s
    }
}

/// Строка журнала send.pool_cond (пулы в условии и без него, блок плана, время чтения) — в фоне: сделка не ждёт
/// записи (Тигран 16.11.2026). Принимает: окружение, id сделки, условие. Возвращает: строку условия (summary) — для
/// карточки сделки и текста ошибки. Ошибок нет.
pub fn log(ctx: &Ctx, deal: &str, conds: &PoolConds) -> String {
    let note = conds.summary();
    let (ctx, msg) = (ctx.clone(), format!("Сделка {deal}: {note}"));
    tokio::spawn(async move {
        ctx.log(Level::Info, "send.pool_cond", msg, None).await;
    });
    note
}
