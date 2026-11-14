//! Файл: venues/bsc_dex/exec/solidly_gate.rs
//! Что делает: решение 273 — ограничения обмена пар Solidly, которые проверяет сам контракт пары (usdfi BasePair:
//! `require(!factory.isPaused())`, `require(tx.gasprice <= factory.maxGasPrice())`), на стороне плана и отправки:
//! - planned — по состоянию пулов в памяти (gate из load_solidly.rs / solidly_refresh.rs) и цене газа плана: причина,
//!   почему маршрут/цикл неисполним (котировка, исполнимость);
//! - fresh_cap — перед отправкой: ограничения заново с фабрики на последнем блоке (только DEX с заданными методами);
//!   пауза — сделки нет; возвращает наименьший предел цены газа — его проверяет exec/txs.rs send_capped по цене самой
//!   транзакции (legacy gasPrice; чаевые канала — отдельным переводом, на gasPrice свопа не влияют).
//!
//! Кто вызывает: quote/cycle_cost.rs, quote/mod.rs (planned), exec/executor/trade.rs, exec/cycle_exec.rs (fresh_cap).
//! От чего зависит: math/solidly_fee.rs (blocked), pools/{registry, solidly_fee_read}.rs, core/types (PoolState).
//! Настройки: через solidly_fee_read.rs — dex.factory, dex.solidly_pause_method, dex.solidly_max_gas_method [DEX].

use alloy_primitives::U256;

use crate::core::types::PoolState;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::solidly_fee;
use super::super::pools::registry::Entry;
use super::super::pools::solidly_fee_read;

/// Причина, почему обмен через один из пулов откатится при цене газа плана. Принимает: пулы шагов, цену газа.
/// Возвращает: «DEX пул: причина» первого запрещающего пула или None. Ошибок нет.
pub fn reason<'a>(entries: impl IntoIterator<Item = &'a Entry>, price: U256) -> Option<String> {
    entries.into_iter().find_map(|e| match &e.pool.state {
        PoolState::Solidly { gate, .. } => solidly_fee::blocked(gate, Some(price))
            .map(|w| format!("{} {:?}: {w}", e.family.id, e.pool.reference)),
        _ => None,
    })
}

/// То же, что reason, ошибкой. Ошибки: NoRoute — с точной причиной.
pub fn planned<'a>(ctx: &Ctx, entries: impl IntoIterator<Item = &'a Entry>, price: U256) -> GatewayResult<()> {
    match reason(entries, price) {
        Some(w) => Err(ctx.err(ErrorCode::NoRoute, w)),
        None => Ok(()),
    }
}

/// Перед отправкой: ограничения фабрик пулов Solidly шагов заново (последний блок), только у DEX с заданными методами.
/// Принимает: окружение, пулы шагов. Возвращает: наименьший предел цены газа (None — пределов нет).
/// Ошибки: NoRoute — фабрика на паузе; узел, BadResponse, NotConfigured (dex.factory).
pub async fn fresh_cap(ctx: &Ctx, entries: &[&Entry]) -> GatewayResult<Option<U256>> {
    let mut seen: Vec<&str> = Vec::new();
    let mut cap: Option<U256> = None;
    for e in entries.iter().copied() {
        let f = e.family;
        if !matches!(e.pool.state, PoolState::Solidly { .. })
            || seen.contains(&f.id)
            || !solidly_fee_read::has_gate(ctx, f)
        {
            continue;
        }
        seen.push(f.id);
        let factory = ctx.dex_addr("dex.factory", f.id)?;
        let g = solidly_fee_read::gate(ctx, f, factory, "latest").await?;
        if let Some(w) = solidly_fee::blocked(&g, None) {
            return Err(ctx.err(ErrorCode::NoRoute, format!("{}: {w} — своп не отправлен", f.id)));
        }
        if let Some(m) = g.max_gas_price.map(U256::from) {
            cap = Some(cap.map_or(m, |c| c.min(m)));
        }
    }
    Ok(cap)
}
