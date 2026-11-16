//! Файл: venues/bsc_dex/exec/hook_trade.rs
//! Что делает: решение «335 (лесенка влияния)», ревью второго прохода — пути всех долей сделки с влиянием крючка B/C
//! НА СУММЕ СДЕЛКИ (факт пробы квотером), без выпадения пула из-за «не измерено»:
//! 1. оценка: пути долей (exec/live.rs share) с лесенкой хранилища в режиме open (quote/hook_curve.rs: сумма вне
//!    измеренного — наибольшее значение лесенки; ревью Б1 а) — только чтобы узнать входы пулов;
//! 2. проба: входы пулов с крючком, не покрытые фактом этой сделки (check/hook_fold.rs covered), всех долей — одним
//!    пакетом на одном блоке (exec/trade_probe.rs hooks_again; ревью Н7 — доли не по очереди);
//! 3. пути заново с лесенкой сделки (check/hook_fold.rs local: лесенка хранилища + точки этой сделки — напрямую, не
//!    через хранилище, ревью Н1);
//! 4. все входы покрыты — итог: влияние на входе каждого шага с крючком = значение ближайшей точки с входом ≥ x, и эта
//!    точка снята ЭТОЙ сделкой (для неубывающего влияния — не лучше факта); вход сдвинулся за точки сделки (точный
//!    выход: влияние больше оценки → вход больше) — снова 2–3. Кругов пробы — не больше hooks.impact_points (не задана —
//!    один, ревью Н6); не покрыто — отказ этой сделке с причиной.
//!
//! Пулов с крючком B/C нет — один расчёт, как раньше. Проба не прошла на сумме сделки — отказ только этой сделке
//! (ревью Б2, exec/trade_probe.rs).
//!
//! Кто вызывает: exec/live.rs (path, paths), exec/bundle_trade.rs, exec/smart_legs.rs, exec/dry_paths.rs,
//! exec/trade_direct_prep.rs.
//! От чего зависит: exec/{live, trade_probe}.rs, check/{hook_fold, hook_points, quoter_amount}.rs, quote/{path,
//! select, via}.rs.
//! Настройки: hooks.impact_points (необязательна; предел кругов пробы перед сделкой).

use std::sync::Arc;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{HookStep, PoolRef, RouteHop, Side};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::{hook_fold, hook_points, quoter_amount};
use super::super::ctx::Ctx;
use super::super::pools::registry::Entry;
use super::super::quote::path::Path;
use super::super::quote::select::Usable;
use super::super::quote::via::Via;
use super::live;
use super::trade_probe::{self, Fresh};

/// Факты пробы этой сделки.
#[derive(Debug, Clone, Default)]
pub struct TradeHooks {
    pub fresh: Vec<Fresh>,
}

impl TradeHooks {
    /// Точки этой сделки по пулу и стороне (по возрастанию входа). Ошибок нет.
    fn pts(&self, r: &PoolRef, buy: bool) -> Vec<HookStep> {
        let mut v: Vec<HookStep> = self
            .fresh
            .iter()
            .filter(|f| &f.pool == r && f.buy == buy)
            .map(|f| f.step.clone())
            .collect();
        v.sort_by_key(|s| s.amount_in);
        v
    }

    /// Что делает: допуск пула для пути сделки — лесенка сделки (hook_fold::local), оценка вне измеренного (open).
    /// Пул без крючка B/C — без изменений. Принимает: запись, сторону пула, допуск из quote/select.rs. Ошибок нет.
    pub fn apply(&self, e: &Entry, side: Side, u: &mut Usable) {
        if !quoter_amount::hooked(e) {
            return;
        }
        let fresh = self.pts(&e.pool.reference, side == Side::Buy);
        let stored = u.hook.steps.clone().unwrap_or_default();
        let local = hook_fold::local(&stored, u.hook_pct, &fresh);
        u.hook.steps = (!local.is_empty()).then(|| Arc::new(local));
        u.hook.open = true;
    }

    /// Шаги путей с крючком B/C, вход которых не покрыт фактом этой сделки (без повторов). Ошибок нет.
    pub fn need(&self, paths: &[Path]) -> Vec<(Entry, Address, U256)> {
        let mut out: Vec<(Entry, Address, U256)> = Vec::new();
        for h in paths.iter().flat_map(|p| p.hops.iter()) {
            if !quoter_amount::hooked(&h.entry) {
                continue;
            }
            let buy = h.token_in == h.entry.usdt;
            let fresh = self.pts(&h.entry.pool.reference, buy);
            let local = h.usable.hook.steps.clone().unwrap_or_default();
            let x = h.swapped.amount_in;
            let dup = out.iter().any(|(e, t, a)| {
                e.pool.reference == h.entry.pool.reference && *t == h.token_in && *a == x
            });
            if !dup && !hook_fold::covered(&local, &fresh, x) {
                out.push((h.entry.clone(), h.token_in, x));
            }
        }
        out
    }

    /// Выход квотера этой сделки на входе x пула и стороны (ревью Н7: сверка прямого режима без второго вызова).
    /// Ошибок нет.
    pub fn quoted(&self, r: &PoolRef, buy: bool, x: U256) -> Option<U256> {
        self.fresh
            .iter()
            .find(|f| &f.pool == r && f.buy == buy && f.step.amount_in == x)
            .and_then(|f| f.step.quoter_out)
    }
}

/// Предел кругов пробы перед сделкой: hooks.impact_points; не задана — один (ревью Н6). Ошибки: настройка в неверном
/// виде.
fn rounds(ctx: &Ctx) -> GatewayResult<u64> {
    match ctx.int(hook_points::POINTS) {
        Ok(n) => Ok(n.max(1)),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(1),
        Err(e) => Err(e),
    }
}

/// Что делает: пути долей сделки (см. шапку). Принимает: расчёт путей, доли (пулы, количество монет), прямой ли режим.
/// Возвращает: пути по порядку долей и факты пробы. Ошибки: как exec/live.rs share; NoRoute — проба не прошла или
/// вход не покрыт фактом за предел кругов; узел.
pub async fn paths(
    v: &Via<'_>,
    shares: &[(&[RouteHop], Decimal)],
    direct_mode: bool,
) -> GatewayResult<(Vec<Path>, TradeHooks)> {
    let mut th = TradeHooks::default();
    let mut out = all(v, shares, direct_mode, &th).await?;
    let limit = rounds(&v.q.ctx)?;
    for _ in 0..limit {
        let need = th.need(&out);
        if need.is_empty() {
            return Ok((out, th));
        }
        th.fresh.extend(trade_probe::hooks_again(v.q, &need).await?);
        out = all(v, shares, direct_mode, &th).await?;
    }
    if th.need(&out).is_empty() {
        return Ok((out, th));
    }
    let why = format!(
        "влияние крючка на сумме сделки не покрыто фактом пробы за {limit} кругов ({}) — вход пула растёт с влиянием",
        hook_points::POINTS
    );
    Err(v.q.ctx.err(ErrorCode::NoRoute, why))
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
