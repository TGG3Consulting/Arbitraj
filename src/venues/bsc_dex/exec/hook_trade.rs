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
//!    (ревью 3 Б3, hook_drive.rs) и снова 3. Кругов — не больше предела Drive; ревью 3 Н-з: перед каждой пробой после
//!    первой голова сети не дальше verify.max_staleness_blocks от блока первой пробы (не задана — без проверки).
//!
//! Пулов с крючком B/C нет — один расчёт, как раньше. Проба не прошла на сумме сделки — отказ только этой сделке
//! (ревью 2 Б2, exec/trade_probe.rs).
//!
//! Кто вызывает: exec/live.rs (path), exec/bundle_trade.rs, exec/smart_legs.rs, exec/dry_paths.rs.
//! От чего зависит: exec/{live, trade_probe, hook_drive}.rs, check/{hook_points, quoter_amount}.rs, quote/{path,
//! via}.rs, node/heads.rs.
//! Настройки: hooks.impact_points (необязательна; предел кругов пробы — не меньше двух), verify.max_staleness_blocks
//! (необязательна здесь).

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::RouteHop;
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::{hook_points, quoter_amount};
use super::super::pools::registry::Entry;
use super::super::quote::path::Path;
use super::super::quote::via::Via;
use super::hook_drive::{Drive, Next, Want};
use super::live;
use super::trade_probe::{self, Ask};

pub use super::hook_drive::TradeHooks;

/// Что делает: пути долей сделки (см. шапку). Принимает: расчёт путей, доли (пулы, количество монет), прямой ли режим.
/// Возвращает: пути по порядку долей и факты пробы. Ошибки: как exec/live.rs share; NoRoute — проба не прошла или
/// вход не покрыт фактом за предел кругов; StaleData — голова ушла дальше verify.max_staleness_blocks; узел.
pub async fn paths(
    v: &Via<'_>,
    shares: &[(&[RouteHop], Decimal)],
    direct_mode: bool,
) -> GatewayResult<(Vec<Path>, TradeHooks)> {
    let ctx = &v.q.ctx;
    let points = match ctx.int(hook_points::POINTS) {
        Ok(n) => Some(n),
        Err(e) if e.code == ErrorCode::NotConfigured => None,
        Err(e) => return Err(e),
    };
    let mut drive = Drive::new(points);
    let mut th = TradeHooks::default();
    let mut first_block: Option<u64> = None;
    let mut out = all(v, shares, direct_mode, &th).await?;
    loop {
        let seen = views(&out);
        let wants: Vec<Want> = seen.iter().map(|(w, _, _)| w.clone()).collect();
        match drive.next(&mut th, &wants) {
            Next::Done => return Ok((out, th)),
            Next::Fail(why) => return Err(ctx.err(ErrorCode::NoRoute, why)),
            Next::Eval => {}
            Next::Probe(need) => {
                if let Some(b) = first_block {
                    fresh_enough(v, b)?;
                }
                let asks: Vec<Ask> = need
                    .iter()
                    .filter_map(|n| {
                        seen.iter()
                            .find(|(w, ..)| w == n)
                            .map(|(w, e, t)| (e.clone(), *t, w.x, w.applied))
                    })
                    .collect();
                let (fresh, block) = trade_probe::hooks_again(v.q, &asks).await?;
                if first_block.is_none() {
                    first_block = Some(block);
                }
                th.fresh.extend(fresh);
            }
        }
        out = all(v, shares, direct_mode, &th).await?;
    }
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

/// Ревью 3 Н-з: голова сети не дальше verify.max_staleness_blocks от блока первой пробы. Не задана — без проверки.
/// Ошибки: StaleData — ушла дальше; настройка в неверном виде.
fn fresh_enough(v: &Via<'_>, first: u64) -> GatewayResult<()> {
    let ctx = &v.q.ctx;
    let max = match ctx.int("verify.max_staleness_blocks") {
        Ok(m) => m,
        Err(e) if e.code == ErrorCode::NotConfigured => return Ok(()),
        Err(e) => return Err(e),
    };
    let head = v.q.heads.latest().map_or(first, |h| h.number);
    if head.saturating_sub(first) > max {
        let why = format!(
            "проба крючка перед сделкой: голова {head} ушла от первой пробы ({first}) больше чем на \
             verify.max_staleness_blocks = {max}"
        );
        return Err(ctx.err(ErrorCode::StaleData, why));
    }
    Ok(())
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
