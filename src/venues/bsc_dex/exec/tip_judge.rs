//! Файл: venues/bsc_dex/exec/tip_judge.rs
//! Что делает: решение 437 (3) — разбор проигрыша сделки DEX ↔ DEX (режим profit_share): «по чаевым» или «по
//! скорости». Только по событию проигрыша (наша транзакция откатилась или отказана условием −32003), без постоянного
//! опроса: логи пулов маршрута в нужном блоке (eth_getLogs), транзакции, изменившие наш пул раньше нас (в блоке
//! включения — transactionIndex меньше нашего; при отказе условия пулов — первая в окне блоков, см. ниже), их
//! чаевые = effectiveGasPrice квитанции − baseFee блока (nitro arbos/tx_processor.go:1037-1042: при сборе чаевых
//! effectiveGasPrice = min(maxFee, база + чаевые); /home/claude/src-ext/reports/arb-tips.md, Ф8, раздел 2 п. 1–2).
//! Откат: перехватчик — с наибольшими чаевыми среди изменивших пул раньше нас в блоке включения; выше наших → «по
//! чаевым» (доля растёт); не выше или перехватчика нет → «по скорости» (порядок PGA: внутри раунда — по чаевым, раньше
//! пришедший раундом раньше уже в блоке — pga/heap.go:296-301, pga_tx_orderer.go:84-108; чаевыми не лечится).
//! Решение 438 (ревью 437, п. 2): отказ −32003 по условию пулов значит «пул маршрута изменился после блока плана» —
//! перехватчик = ПЕРВАЯ по (блок, transactionIndex) транзакция, изменившая пул маршрута в блоках (блок плана, голова
//! своего узла на момент ответа]; её чаевые против наших. Никого в окне нет — итога нет (None): доля не меняется,
//! только журнал (вызывающий, exec/tip_cycle.rs).
//! Причину отката квитанция не даёт (minOut или deadline — contracts/ArbExecutor.sol:154,165; debug_* у узла закрыт),
//! поэтому разбирается любой откат транзакции цикла (docs/TASKS.md, «Решение 437»).
//! Ревью 438 (D): «изменил пул» — лог события, пишущего слоты условия этого пула (topic0 — exec/pool_cond_topics.rs);
//! у пулов с корнем хранилища или без условия — любой лог пула.
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
    /// Блоки поиска включительно: блок включения (откат) — (b, b); отказ условия пулов — (блок плана + 1, голова).
    pub blocks: (u64, u64),
    /// Наше место в блоке (откат); None — нас в сети нет (отказ условия): перехватчик — первый изменивший пул.
    pub index: Option<u64>,
    /// Наши чаевые, wei за газ.
    pub tip: u128,
    /// Пулы маршрута.
    pub pools: Vec<PoolRef>,
    /// Ревью 438 (D): по пулам (как pools) topic0 событий, пишущих слоты условия (exec/pool_cond_topics.rs); None —
    /// любое событие пула.
    pub topics: Vec<Option<Vec<B256>>>,
}

/// Лог eth_getLogs: только нужные поля.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawLog {
    address: Address,
    topics: Vec<B256>,
    block_number: Option<U64>,
    transaction_hash: Option<B256>,
    transaction_index: Option<U64>,
}

/// Квитанция перехватчика: только цена газа.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PaidPrice {
    effective_gas_price: U256,
}

/// Разбор проигрыша. Принимает: окружение, что разбираем. Возвращает: TipLoss (итог, перехватчик и его чаевые);
/// None — отказ условия, а изменившего пул в окне нет (итог не определяется однозначно).
/// Ошибки: узел (логи, квитанция, блок), BadResponse.
pub async fn judge(ctx: &Ctx, p: &Probe) -> GatewayResult<Option<TipLoss>> {
    let rivals = rivals(ctx, p).await?;
    // Откат — все раньше нас в блоке включения (наибольшие чаевые); отказ условия — только первый в окне.
    let take = if p.index.is_some() { rivals.len() } else { 1 };
    let mut best: Option<(u64, B256, u128)> = None;
    for (block, h) in rivals.into_iter().take(take) {
        let r: Option<PaidPrice> = rpc::call(ctx, "eth_getTransactionReceipt", json!([h])).await?;
        let Some(r) = r else { continue };
        let base = gas_base::fetch(ctx, block).await?;
        let paid = r.effective_gas_price.saturating_sub(base);
        let tip = u128::try_from(paid).unwrap_or(u128::MAX);
        if best.is_none_or(|(_, _, t)| tip > t) {
            best = Some((block, h, tip));
        }
    }
    if p.index.is_none() && best.is_none() {
        return Ok(None);
    }
    let outcome = match best {
        Some((_, _, t)) if t > p.tip => TipOutcome::LostTip,
        _ => TipOutcome::LostSpeed,
    };
    Ok(Some(TipLoss {
        at: Utc::now(),
        ours: if p.ours.is_zero() {
            "нет в сети (отказ условия пулов −32003)".to_string()
        } else {
            format!("{:#x}", p.ours)
        },
        block: best.map_or(p.blocks.1, |(b, _, _)| b),
        outcome,
        rival: best.map(|(_, h, _)| format!("{h:#x}")),
        rival_tip_gwei: best.and_then(|(_, _, t)| tip_mode::to_gwei(t)),
        our_tip_gwei: tip_mode::to_gwei(p.tip).unwrap_or_default(),
    }))
}

/// Транзакции, изменившие пул маршрута в блоках поиска раньше нас (без нашей), по (блок, transactionIndex), без
/// повторов. Возвращает: (блок, хеш). Ошибки: узел.
async fn rivals(ctx: &Ctx, p: &Probe) -> GatewayResult<Vec<(u64, B256)>> {
    let mut addrs: Vec<Address> = p.pools.iter().map(PoolRef::address).collect();
    addrs.sort();
    addrs.dedup();
    let (from, to) = (block_tag(p.blocks.0), block_tag(p.blocks.1));
    let filter = json!([{ "fromBlock": from, "toBlock": to, "address": addrs }]);
    let logs: Vec<RawLog> = rpc::call(ctx, "eth_getLogs", filter).await?;
    let mut found: Vec<(u64, u64, B256)> = Vec::new();
    for l in logs {
        let (Some(b), Some(h), Some(i)) = (l.block_number, l.transaction_hash, l.transaction_index)
        else {
            continue;
        };
        let (b, i) = (b.to::<u64>(), i.to::<u64>());
        let before = p.index.is_none_or(|ours| i < ours);
        if h != p.ours && before && touches(&l, p) && !found.iter().any(|f| f.2 == h) {
            found.push((b, i, h));
        }
    }
    found.sort_by_key(|f| (f.0, f.1));
    Ok(found.into_iter().map(|(b, _, h)| (b, h)).collect())
}
/// Лог относится к пулу маршрута: V4 — менеджер и topics[1] = PoolId; прочие — адрес контракта; ревью 438 (D) — и
/// topic0 из событий, пишущих слоты условия этого пула (None — любое). Ошибок нет.
fn touches(l: &RawLog, p: &Probe) -> bool {
    p.pools.iter().zip(&p.topics).any(|(r, t)| {
        let at = match r {
            PoolRef::Singleton { manager, id } => {
                l.address == *manager && l.topics.get(1) == Some(id)
            }
            other => l.address == other.address(),
        };
        let kind = t
            .as_ref()
            .is_none_or(|t| l.topics.first().is_some_and(|t0| t.contains(t0)));
        at && kind
    })
}
