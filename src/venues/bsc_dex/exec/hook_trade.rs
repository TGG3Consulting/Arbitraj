//! Файл: venues/bsc_dex/exec/hook_trade.rs
//! Что делает: решение «335 (лесенка влияния)», ревью 2–3 — пути всех долей сделки с влиянием крючка B/C НА СУММЕ
//! СДЕЛКИ (факт пробы квотером), без выпадения пула из-за «не измерено»: цикл «расчёт → решение → проба» с узлом;
//! решение на каждом шаге — чистая логика exec/hook_drive.rs (Drive, TradeHooks), покрыта тестами на записях узла:
//! 1. оценка: пути долей (exec/live.rs share) с лесенкой хранилища в режиме open — только чтобы узнать входы пулов;
//!    лимит hooks.max_impact_* — только по факту пробы (ревью 3 Н-г);
//! 2. проба: входы пулов с крючком, не покрытые фактом этой сделки, всех долей — одним пакетом на одном блоке
//!    (exec/trade_probe.rs hooks_again; ревью 2 Н7);
//! 3. пути заново с лесенкой сделки (лесенка хранилища + точки этой сделки — напрямую, ревью 2 Н1);
//! 4. все входы покрыты — итог; вход вырос за точки сделки (точный выход) — проба на входе с поднятым влиянием
//!    (ревью 3 Б3, hook_drive.rs) и снова 3.
//!
//! Ревью 4 лесенки:
//! - Н4: кругов пробы — два (сам способ), сверх — только hooks.trade_probe_rounds_max (не число точек лесенки);
//! - Н7: все пробы одной сделки — на блоке первой пробы (trade_probe.rs pin): факты сделки — одна картина пула, без
//!   настройки и без «расстояния между пробами» (прежняя проверка головы по verify.max_staleness_blocks убрана); возраст
//!   этой картины ограничен числом кругов;
//! - Н-ж: доли с общим пулом (решение 223) исполняются по порядку, и вход пула с крючком у доли после общего пула
//!   меняется (quote/graph_seq.rs resequence): входы шагов с крючком берутся и из путей по порядку исполнения — проба
//!   покрывает и их фактом квотера (на состоянии блока пробы; зависимость крючка от состояния после прошлых долей
//!   квотер не видит — её страхует симуляция бандла eth_simulateV1 и amountOutMin ноги, docs/notes).
//!
//! Пулов с крючком B/C нет — один расчёт, как раньше. Проба не прошла на сумме сделки — отказ только этой сделке
//! (ревью 2 Б2, exec/trade_probe.rs).
//!
//! Кто вызывает: exec/live.rs (path), exec/bundle_trade.rs, exec/smart_legs.rs, exec/dry_paths.rs.
//! От чего зависит: exec/{live, trade_probe, hook_drive}.rs, check/quoter_amount.rs, quote/{path, via, graph_seq}.rs.
//! Настройки: hooks.trade_probe_rounds_max (необязательна; не задана — два круга).

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::RouteHop;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::quoter_amount;
use super::super::pools::registry::Entry;
use super::super::quote::graph_seq;
use super::super::quote::path::Path;
use super::super::quote::via::Via;
use super::hook_drive::{Drive, Next, ROUNDS_MAX, Want};
use super::live;
use super::trade_probe::{self, Ask};

pub use super::hook_drive::TradeHooks;

/// Что делает: пути долей сделки (см. шапку). Принимает: расчёт путей, доли (пулы, количество монет), прямой ли режим.
/// Возвращает: пути по порядку долей (как посчитаны, без пересчёта по порядку исполнения — его делает вызывающий) и
/// факты пробы. Ошибки: как exec/live.rs share и quote/graph_seq.rs resequence; NoRoute — проба не прошла или вход не
/// покрыт фактом за предел кругов; узел; настройка в неверном виде.
pub async fn paths(
    v: &Via<'_>,
    shares: &[(&[RouteHop], Decimal)],
    direct_mode: bool,
) -> GatewayResult<(Vec<Path>, TradeHooks)> {
    let ctx = &v.q.ctx;
    let rounds = match ctx.int(ROUNDS_MAX) {
        Ok(n) => Some(n),
        Err(e) if e.code == ErrorCode::NotConfigured => None,
        Err(e) => return Err(e),
    };
    let mut drive = Drive::new(rounds);
    let mut th = TradeHooks::default();
    let mut pin: Option<u64> = None;
    let mut out = all(v, shares, direct_mode, &th).await?;
    loop {
        let ins = seen(v, shares, &out)?;
        let wants: Vec<Want> = ins.iter().map(|(w, _, _)| w.clone()).collect();
        match drive.next(&mut th, &wants) {
            Next::Done => return Ok((out, th)),
            Next::Fail(why) => return Err(ctx.err(ErrorCode::NoRoute, why)),
            Next::Eval => {}
            Next::Probe(need) => {
                let asks: Vec<Ask> = need
                    .iter()
                    .filter_map(|n| {
                        ins.iter()
                            .find(|(w, ..)| w == n)
                            .map(|(w, e, t)| (e.clone(), *t, w.x, w.applied))
                    })
                    .collect();
                // Ревью 4 (Н7): повторная проба — на блоке первой.
                let (fresh, block) = trade_probe::hooks_again(v.q, &asks, pin).await?;
                if pin.is_none() {
                    pin = Some(block);
                }
                th.fresh.extend(fresh);
            }
        }
        out = all(v, shares, direct_mode, &th).await?;
    }
}

/// Входы шагов с крючком B/C путей и (ревью 4 Н-ж) тех же путей по порядку исполнения долей с общим пулом
/// (quote/graph_seq.rs resequence; без общих пулов — те же входы). Ошибки: как resequence.
fn seen(
    v: &Via<'_>,
    shares: &[(&[RouteHop], Decimal)],
    out: &[Path],
) -> GatewayResult<Vec<(Want, Entry, Address)>> {
    let mut got = views(out);
    if shares.len() < 2 || got.is_empty() {
        return Ok(got);
    }
    let mut seq: Vec<(Path, Decimal)> = out
        .iter()
        .cloned()
        .zip(shares.iter().map(|s| s.1))
        .collect();
    graph_seq::resequence(&v.q.ctx, v.side, v.taxes, &mut seq)?;
    let paths: Vec<Path> = seq.into_iter().map(|(p, _)| p).collect();
    for (w, e, t) in views(&paths) {
        // Запись пула — из путей как посчитаны (у общего пула в пути по порядку — состояние после прошлых долей, а
        // проба читает пул сама).
        let e = got
            .iter()
            .find(|a| a.0.pool == w.pool)
            .map_or(e, |a| a.1.clone());
        if !got.iter().any(|a| a.0 == w) {
            got.push((w, e, t));
        }
    }
    Ok(got)
}

/// Входы шагов с крючком B/C путей: вход, пул из памяти, токен входа. Ошибок нет.
fn views(paths: &[Path]) -> Vec<(Want, Entry, Address)> {
    paths
        .iter()
        .flat_map(|p| p.hops.iter())
        .filter(|h| quoter_amount::hooked(&h.entry))
        .map(|h| {
            let w = Want {
                pool: h.entry.pool.reference.clone(),
                buy: h.token_in == h.entry.usdt,
                x: h.swapped.amount_in,
                applied: h.usable.hook_pct,
                steps: h.usable.hook.steps.as_deref().cloned().unwrap_or_default(),
            };
            (w, h.entry.clone(), h.token_in)
        })
        .collect()
}

/// Пути всех долей по текущим фактам. Ошибки: как exec/live.rs share.
async fn all(
    v: &Via<'_>,
    shares: &[(&[RouteHop], Decimal)],
    direct_mode: bool,
    th: &TradeHooks,
) -> GatewayResult<Vec<Path>> {
    let mut out = Vec::with_capacity(shares.len());
    for (hops, qty) in shares {
        out.push(live::share(v, hops, *qty, direct_mode, th).await?);
    }
    Ok(out)
}
