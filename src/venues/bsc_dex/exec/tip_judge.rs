//! Файл: venues/bsc_dex/exec/tip_judge.rs
//! Что делает: решение 437 (3) — разбор проигрыша сделки DEX ↔ DEX (режим profit_share): «по чаевым» или «по
//! скорости». Только по событию проигрыша (наша транзакция откатилась или отказана условием −32003), без постоянного
//! опроса: логи пулов маршрута в нужном блоке (eth_getLogs), транзакции, изменившие наш пул раньше нас (в блоке
//! включения — transactionIndex меньше нашего; при отказе условия — в ближайшем блоке после отправки, все), их
//! чаевые = effectiveGasPrice квитанции − baseFee блока (nitro arbos/tx_processor.go:1037-1042: при сборе чаевых
//! effectiveGasPrice = min(maxFee, база + чаевые); /home/claude/src-ext/reports/arb-tips.md, Ф8, раздел 2 п. 1–2).
//! Перехватчик — с наибольшими чаевыми; выше наших → «по чаевым» (доля растёт); не выше или перехватчика нет →
//! «по скорости» (порядок PGA: внутри раунда — по чаевым, раньше пришедший раундом раньше уже в блоке — pga/heap.go:
//! 296-301, pga_tx_orderer.go:84-108; чаевыми не лечится).
//! Причину отката квитанция не даёт (minOut или deadline — contracts/ArbExecutor.sol:154,165; debug_* у узла закрыт),
//! поэтому разбирается любой откат транзакции цикла (docs/TASKS.md, «Решение 437»).
//! Пул V4 (Singleton) — лог менеджера с topics[1] = PoolId (события менеджера с bytes32 indexed id первым —
//! pools/abi_v4.rs:25-47, сверка PoolId с topics[1] — pools/discover_v4.rs); пара общего контракта — любой лог
//! контракта.
//! Кто вызывает: exec/tip_cycle.rs (фоновая задача после итога сделки).
//! От чего зависит: node/rpc.rs, node/call.rs (block_tag), quote/gas_base.rs (baseFee блока), send/tip_mode.rs, ctx.rs.
//! Настройки: нет.
//!
//! eth_getLogs (address — адрес или массив; fromBlock/toBlock), квитанция (effectiveGasPrice, transactionIndex) —
//! execution-apis 16caee27 src/eth/filter.yaml, src/eth/transaction.yaml#L127-L165, src/schemas/receipt.yaml.

use alloy_primitives::{Address, B256, U64, U256};
use chrono::Utc;
use serde::Deserialize;
use serde_json::json;

use crate::core::types::{PoolRef, TipLoss, TipOutcome};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::node::rpc;
use super::super::quote::gas_base;
use super::super::send::tip_mode;

/// Что разбираем.
#[derive(Debug, Clone)]
pub struct Probe {
    /// Наша транзакция; B256::ZERO — её нет в сети (отказ условия: хеш в ошибке отправки не передаётся).
    pub ours: B256,
    /// Блок поиска: включения (откат) или ближайший после отправки (отказ условия).
    pub block: u64,
    /// Наше место в блоке (откат); None — нас в блоке нет (отказ условия): берутся все изменившие наш пул.
    pub index: Option<u64>,
    /// Наши чаевые, wei за газ.
    pub tip: u128,
    /// Пулы маршрута.
    pub pools: Vec<PoolRef>,
}

/// Лог eth_getLogs: только нужные поля.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLog {
    address: Address,
    topics: Vec<B256>,
    transaction_hash: Option<B256>,
    transaction_index: Option<U64>,
}

/// Квитанция перехватчика: только цена газа.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaidPrice {
    effective_gas_price: U256,
}

/// Разбор проигрыша. Принимает: окружение, что разбираем. Возвращает: TipLoss (итог, перехватчик и его чаевые).
/// Ошибки: узел (логи, квитанция, блок), BadResponse.
pub async fn judge(ctx: &Ctx, p: &Probe) -> GatewayResult<TipLoss> {
    let rivals = rivals(ctx, p).await?;
    let mut best: Option<(B256, u128)> = None;
    if !rivals.is_empty() {
        let base = gas_base::fetch(ctx, p.block).await?;
        for h in rivals {
            let r: Option<PaidPrice> =
                rpc::call(ctx, "eth_getTransactionReceipt", json!([h])).await?;
            let Some(r) = r else { continue };
            let paid = r.effective_gas_price.saturating_sub(base);
            let tip = u128::try_from(paid).unwrap_or(u128::MAX);
            if best.is_none_or(|(_, t)| tip > t) {
                best = Some((h, tip));
            }
        }
    }
    let outcome = match best {
        Some((_, t)) if t > p.tip => TipOutcome::LostTip,
        _ => TipOutcome::LostSpeed,
    };
    Ok(TipLoss {
        at: Utc::now(),
        ours: if p.ours.is_zero() {
            "нет в сети (отказ условия −32003)".to_string()
        } else {
            format!("{:#x}", p.ours)
        },
        block: p.block,
        outcome,
        rival: best.map(|(h, _)| format!("{h:#x}")),
        rival_tip_gwei: best.and_then(|(_, t)| tip_mode::to_gwei(t)),
        our_tip_gwei: tip_mode::to_gwei(p.tip).unwrap_or_default(),
    })
}

/// Транзакции блока, изменившие пул маршрута раньше нас (без нашей), по порядку, без повторов. Ошибки: узел.
async fn rivals(ctx: &Ctx, p: &Probe) -> GatewayResult<Vec<B256>> {
    let mut addrs: Vec<Address> = p.pools.iter().map(PoolRef::address).collect();
    addrs.sort();
    addrs.dedup();
    let tag = block_tag(p.block);
    let filter = json!([{ "fromBlock": tag, "toBlock": tag, "address": addrs }]);
    let logs: Vec<RawLog> = rpc::call(ctx, "eth_getLogs", filter).await?;
    let mut out: Vec<B256> = Vec::new();
    for l in logs {
        let (Some(h), Some(i)) = (l.transaction_hash, l.transaction_index) else {
            continue;
        };
        let before = p.index.is_none_or(|ours| i.to::<u64>() < ours);
        if h != p.ours && before && touches(&l, &p.pools) && !out.contains(&h) {
            out.push(h);
        }
    }
    Ok(out)
}

/// Лог относится к пулу маршрута: V4 — менеджер и topics[1] = PoolId; прочие — адрес контракта. Ошибок нет.
fn touches(l: &RawLog, pools: &[PoolRef]) -> bool {
    pools.iter().any(|r| match r {
        PoolRef::Singleton { manager, id } => {
            l.address == *manager && l.topics.get(1) == Some(id)
        }
        other => l.address == other.address(),
    })
}
