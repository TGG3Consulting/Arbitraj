//! Файл: venues/bsc_dex/exec/solidly_gate.rs
//! Что делает: решение 273 — ограничения обмена пар Solidly, которые проверяет сам контракт пары (usdfi BasePair:
//! `require(!factory.isPaused())`, `require(tx.gasprice <= factory.maxGasPrice())`), на стороне плана и отправки:
//! - planned — по состоянию пулов в памяти (gate из load_solidly.rs / solidly_refresh.rs) и цене газа плана: причина,
//!   почему маршрут/цикл неисполним (котировка, исполнимость);
//! - fresh_cap — перед отправкой: ограничения заново с фабрики на последнем блоке (только DEX с заданными методами);
//!   пауза — сделки нет; возвращает наименьший предел цены газа — его проверяет exec/txs.rs send_capped по цене самой
//!   транзакции (legacy gasPrice; чаевые канала — отдельным переводом, на gasPrice свопа не влияют); свежие
//!   ограничения, отличные от памяти, — в реестр всем пулам DEX (ревью 273, п. 3).
//!
//! Решение 470: у Aero Slipstream V3 — pools/aero_cl.rs fee (крючок, MEV-налог); у Aero Basic перед отправкой — pools/aero_mev.rs fresh (пауза фабрики, правило MEV-налога).
//! Кто вызывает: quote/cycle_cost.rs, quote/mod.rs (planned), exec/executor/trade.rs, exec/cycle_exec.rs (fresh_cap).
//! От чего зависит: math/solidly_fee.rs (blocked), pools/{registry, solidly_fee_read}.rs, quote/mod.rs (Quoting),
//! core/types (PoolState, SolidlyGate).
//! Настройки: через solidly_fee_read.rs — dex.factory, dex.solidly_pause_method, dex.solidly_max_gas_method [DEX].

use alloy_primitives::U256;

use crate::core::types::{PoolRef, PoolState, SolidlyGate};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::ctx::Ctx;
use super::super::math::solidly_fee;
use super::super::pools::aero_cl;
use super::super::pools::aero_mev;
use super::super::pools::exec_kind::{self, ExecKind};
use super::super::pools::families::Proto;
use super::super::pools::family_variant::Variant;
use super::super::pools::registry::{Entry, Registry};
use super::super::pools::solidly_fee_read;
use super::super::quote::Quoting;

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
pub fn planned<'a>(
    ctx: &Ctx,
    entries: impl IntoIterator<Item = &'a Entry>,
    price: U256,
) -> GatewayResult<()> {
    match reason(entries, price) {
        Some(w) => Err(ctx.err(ErrorCode::NoRoute, w)),
        None => Ok(()),
    }
}

/// Перед отправкой: ограничения фабрик пулов Solidly шагов заново (последний блок), только у DEX с заданными методами.
/// Ревью 273 (п. 3): свежие ограничения отличаются от памяти — записываются в реестр всем пулам этой DEX (remember),
/// чтобы до следующего прохода сверки котировка не выбирала пул на паузе / с пределом ниже плановой цены газа.
/// Принимает: котировку (окружение и реестр), пулы шагов. Возвращает: наименьший предел цены газа (None — пределов нет).
/// Ошибки: NoRoute — фабрика на паузе; узел, BadResponse, NotConfigured (dex.factory), хранилище настроек (has_gate).
pub async fn fresh_cap(q: &Quoting, entries: &[&Entry]) -> GatewayResult<Option<U256>> {
    let ctx = &q.ctx;
    let mut seen: Vec<&str> = Vec::new();
    let mut cap: Option<U256> = None;
    for e in entries.iter().copied() {
        let f = e.family;
        // Решение 470: пул Aero Slipstream V3 — крючок и правило MEV-налога заново (pools/aero_cl.rs).
        if f.variant == Variant::SlipstreamAero
            && let PoolRef::Contract(pool) = e.pool.reference
        {
            aero_cl::fee(ctx, f, pool, "latest").await?;
            continue;
        }
        if !matches!(e.pool.state, PoolState::Solidly { .. }) || seen.contains(&f.id) {
            continue;
        }
        seen.push(f.id);
        // Решение 470: Aero Basic — пауза фабрики и правило MEV-налога (pools/aero_mev.rs) заново перед отправкой;
        // T470-06: у исполнителя v5 (шаг AERO_BASIC) налог исполняется — проверяется только пауза.
        if f.proto == Proto::AeroBasic {
            let taxed_ok = exec_kind::for_wallet(ctx, f) == Some(ExecKind::AeroBasic);
            aero_mev::fresh(ctx, ctx.dex_addr("dex.factory", f.id)?, taxed_ok).await?;
            continue;
        }
        if !solidly_fee_read::has_gate(ctx, f)? {
            continue;
        }
        let factory = ctx.dex_addr("dex.factory", f.id)?;
        let g = solidly_fee_read::gate(ctx, f, factory, "latest").await?;
        remember(&q.reg, entries, f.id, g);
        if let Some(w) = solidly_fee::blocked(&g, None) {
            return Err(ctx.err(
                ErrorCode::NoRoute,
                format!("{}: {w} — своп не отправлен", f.id),
            ));
        }
        if let Some(m) = g.max_gas_price.map(U256::from) {
            cap = Some(cap.map_or(m, |c| c.min(m)));
        }
    }
    Ok(cap)
}

/// Свежие ограничения фабрики DEX `id` — в реестр, если у пулов шагов этой DEX в памяти другие: всем пулам Solidly
/// этой DEX (фабрика одна на DEX). Совпали — реестр не трогается (проход реестра только при изменении). Ошибок нет.
fn remember(reg: &Registry, entries: &[&Entry], id: &str, g: SolidlyGate) {
    let stale = entries.iter().any(|e| {
        e.family.id == id && matches!(&e.pool.state, PoolState::Solidly { gate, .. } if *gate != g)
    });
    if !stale {
        return;
    }
    for r in reg.refs() {
        reg.update(&r, |x| {
            if x.family.id == id
                && let PoolState::Solidly { gate, .. } = &mut x.pool.state
            {
                *gate = g;
            }
        });
    }
}
