//! Файл: venues/bsc_dex/exec/trade_probe_plan.rs
//! Что делает: чистые части пробы крючков перед сделкой (вынесено из exec/trade_probe.rs ревью 4: файл был больше
//! 200 строк):
//! - plans: планы пробы по пулам — точки пулов с крючком B/C по сторонам, без повторов, по возрастанию входа;
//! - human (ревью 4 Н3): вход точек пробы в токене входа стороны (для экрана «Монеты», без hex).
//!
//! Кто вызывает: exec/trade_probe.rs (hooks_again).
//! От чего зависит: check/hooks_probe.rs (Plan, Point), pools/registry.rs (Entry), tokens.rs (decimals_known), num.rs.
//! Настройки: нет.

use std::sync::Arc;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{HookGroup, HookImpact};

use super::super::check::hooks_probe::{Plan, Point};
use super::super::num;
use super::super::pools::registry::Entry;
use super::super::tokens::Tokens;
use super::trade_probe::Ask;

/// Пул пробы: запись, группа, план (точки обеих сторон), влияние до пробы по точкам (покупка ли, вход, влияние).
pub type Pool = (Entry, HookGroup, Plan, Vec<(bool, U256, Decimal)>);

/// Планы пробы по пулам (чистая): точки пула с крючком B/C по сторонам, без повторов, по возрастанию входа; пулы — в
/// порядке первого появления. Ошибок нет.
pub fn plans(steps: &[Ask]) -> Vec<Pool> {
    let mut out: Vec<Pool> = Vec::new();
    for (e, token_in, amount_in, est) in steps {
        let group = e.pool.hooks.as_ref().map(|h| h.group);
        let Some(g) = group.filter(|g| *g != HookGroup::A) else {
            continue;
        };
        let at = match out
            .iter()
            .position(|(x, ..)| x.pool.reference == e.pool.reference)
        {
            Some(i) => i,
            None => {
                out.push((e.clone(), g, Plan::default(), Vec::new()));
                out.len() - 1
            }
        };
        let buy = *token_in == e.usdt;
        let (_, _, plan, ests) = &mut out[at];
        let pts = if buy { &mut plan.buy } else { &mut plan.sell };
        if pts.iter().all(|p| p.amount_in != *amount_in) {
            pts.push(Point {
                usd: Decimal::ZERO,
                amount_in: *amount_in,
            });
            pts.sort_by_key(|p| p.amount_in);
            ests.push((buy, *amount_in, *est));
        }
    }
    out
}

/// Ревью 4 (Н3): вход точек пробы в токене входа стороны (покупка — встречный токен пула, продажа — монета), decimals
/// из памяти (tokens.rs decimals_known; нет — точка без этого поля). Ошибок нет.
pub fn human(tokens: &Tokens, e: &Entry, i: &mut HookImpact) {
    for (steps, token) in [(&mut i.buy, e.usdt), (&mut i.sell, e.coin)] {
        let Some(dec) = tokens.decimals_known(token) else {
            continue;
        };
        for s in Arc::make_mut(steps).iter_mut() {
            s.coins = num::to_decimal(s.amount_in, dec);
        }
    }
}
