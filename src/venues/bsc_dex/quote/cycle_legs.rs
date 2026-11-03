//! Файл: venues/bsc_dex/quote/cycle_legs.rs
//! Что делает: ноги цикла DEX ↔ DEX (решение 124) по сигналу ядра DexCycle: пулы маршрута ищутся в реестре по ссылке
//! (pools/registry.rs), допуск каждого пула — тот же, что у поиска циклов (quote/select.rs, сторона хопа —
//! quote/graph_legs.rs hop_side), decimals — из кэша токенов или узла. Два режима:
//! - по состоянию в памяти (now) — для расчёта цикла на каждую точку подбора размера (быстро, без узла);
//! - СВЕЖЕЕ состояние (fresh) — перед сделкой и сухой сделкой: пулы цикла перечитываются ОДНИМ Multicall3 на голове
//!   (pools/load_batch.rs, как предторговая верификация графа — решение 83, quote/verify.rs) с пределами verify.*;
//!   состояние кладётся на КЛОНЫ записей (реестр не трогаем). «Не торгуем вслепую»: не перечитался пул — сделки нет.
//!
//! Нативный BNB (адрес 0 у пулов V4/Infinity с нативной валютой) внутри цикла в v1 не исполняется — отказ.
//! Кто вызывает: quote/cycle_cost.rs, exec/cycle_trade.rs, exec/cycle_dry.rs.
//! От чего зависит: quote/{route_calc, select, graph_legs, verify}.rs, pools/{registry, load, load_batch}.rs, tokens.rs.
//! Настройки: verify.max_staleness_blocks, verify.timeout_ms (через verify::limits) [bsc_dex].

use std::time::Duration;

use alloy_primitives::Address;

use crate::core::types::DexCycle;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::pools::load;
use super::super::pools::load_batch::{self, Want};
use super::super::pools::registry::Entry;
use super::Quoting;
use super::graph_legs::hop_side;
use super::route_calc::Leg;
use super::{select, verify};

/// Ноги цикла по состоянию пулов в памяти. Ошибки: NoRoute (пул не в реестре, не допущен, нативный BNB, разрыв цикла),
/// NotConfigured (адрес USDT), узел (decimals).
pub async fn now(q: &Quoting, c: &DexCycle) -> GatewayResult<Vec<Leg>> {
    let entries = entries(q, c)?;
    legs(q, c, entries).await
}

/// Ноги цикла на СВЕЖЕМ состоянии пулов (одним Multicall3 на голове). Возвращает: ноги и блок перечтения.
/// Ошибки: StaleData (нет головы, пул не перечитан, таймаут verify.timeout_ms, отставание больше
/// verify.max_staleness_blocks), NoRoute, NotConfigured (verify.*), узел.
pub async fn fresh(q: &Quoting, c: &DexCycle) -> GatewayResult<(Vec<Leg>, u64)> {
    let ctx = &q.ctx;
    let (max_stale, _drift, timeout_ms) = verify::limits(ctx)?;
    let entries = entries(q, c)?;
    let block = q
        .heads
        .latest()
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::StaleData,
                "нет свежего блока для перечтения цикла",
            )
        })?
        .number;
    let wants: Vec<Want> = entries
        .iter()
        .map(|e| Want {
            family: e.family,
            reference: &e.pool.reference,
            spacing: e.pool.tick_spacing,
            step: e.pool.bin_step,
            usdt: e.usdt,
            token0: e.pool.token0,
            token1: e.pool.token1,
        })
        .collect();
    let fetch = load_batch::states(ctx, &q.tokens, &wants, block);
    let loaded = tokio::time::timeout(Duration::from_millis(timeout_ms), fetch)
        .await
        .map_err(|_| {
            ctx.err(
                ErrorCode::StaleData,
                format!(
                    "перечтение пулов цикла не уложилось в verify.timeout_ms ({timeout_ms} мс)"
                ),
            )
        })?;
    let mut renewed = Vec::with_capacity(entries.len());
    for (e, l) in entries.iter().zip(loaded) {
        let st = l.map_err(|m| {
            let why = format!("пул цикла не перечитан: {}", m.message);
            ctx.err(ErrorCode::StaleData, why)
        })?;
        renewed.push(load::renewed(e, block, st));
    }
    let head = q.heads.latest().map_or(block, |h| h.number);
    if head.saturating_sub(block) > max_stale {
        let why = format!(
            "состояние цикла устарело: блок {block} отстал от головы {head} больше verify.max_staleness_blocks ({max_stale})"
        );
        return Err(ctx.err(ErrorCode::StaleData, why));
    }
    Ok((legs(q, c, renewed).await?, block))
}

/// Записи реестра по хопам цикла, по порядку; цикл замкнут (base → … → base), без нативного BNB.
/// Ошибки: NoRoute.
fn entries(q: &Quoting, c: &DexCycle) -> GatewayResult<Vec<Entry>> {
    let ctx = &q.ctx;
    let no = |w: String| ctx.err(ErrorCode::NoRoute, w);
    let (Some(first), Some(last)) = (c.hops.first(), c.hops.last()) else {
        return Err(no("цикл без шагов".into()));
    };
    if first.token_in != c.base || last.token_out != c.base {
        return Err(no(
            "цикл не начинается и не заканчивается базовой монетой".into()
        ));
    }
    let mut prev = c.base;
    let mut out = Vec::with_capacity(c.hops.len());
    for h in &c.hops {
        if h.token_in != prev {
            return Err(no(format!(
                "шаг {} цикла не продолжает предыдущий",
                h.index
            )));
        }
        if h.token_in == Address::ZERO || h.token_out == Address::ZERO {
            return Err(no(
                "нативный BNB внутри цикла в v1 не исполняется (базовая — WBNB, ERC-20)".into(),
            ));
        }
        prev = h.token_out;
        out.push(
            q.reg
                .get(&h.pool)
                .ok_or_else(|| no(format!("пул шага {} больше не в реестре", h.index)))?,
        );
    }
    Ok(out)
}

/// Ноги из записей: допуск пула по стороне хопа и decimals. Ошибки: NoRoute (пул не допущен), узел (decimals).
async fn legs(q: &Quoting, c: &DexCycle, entries: Vec<Entry>) -> GatewayResult<Vec<Leg>> {
    let ctx = &q.ctx;
    let usdt = q.tokens.usdt(ctx)?;
    let usdt_dec = q.tokens.decimals(ctx, usdt).await?;
    let mut out = Vec::with_capacity(entries.len());
    for (h, entry) in c.hops.iter().zip(entries) {
        let side = hop_side(&entry, h.token_in)
            .ok_or_else(|| ctx.err(ErrorCode::NoRoute, "пул шага не соединяет токены шага"))?;
        let usable = select::usable(q, &entry, side, usdt_dec).map_err(|w| {
            let why = format!(
                "шаг {} ({} {}): {w}",
                h.index, entry.family.title, entry.ticker
            );
            ctx.err(ErrorCode::NoRoute, why)
        })?;
        out.push(Leg {
            dec_in: q.tokens.decimals(ctx, h.token_in).await?,
            dec_out: q.tokens.decimals(ctx, h.token_out).await?,
            entry,
            usable,
            token_in: h.token_in,
            token_out: h.token_out,
        });
    }
    Ok(out)
}
