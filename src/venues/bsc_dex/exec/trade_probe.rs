//! Файл: venues/bsc_dex/exec/trade_probe.rs
//! Что делает: повторная проба крючков B/C перед сделкой (решение 32) на СУММЕ СДЕЛКИ — точки пробы = входы пулов с
//! крючком в шагах путей сделки (решение «335 (лесенка влияния)»). Решение «335 (В1, пакет)» (check/hook_batch.rs): на
//! ОДНОМ блоке и одним кругом до узла для всех пулов всех долей (ревью Н7: пулы долей сплита — одним пакетом, а не
//! доля за долей; точки одного пула — в одном плане) — графовый маршрут: свежие записи верификации
//! (Quoting.route_fresh) и только вызовы квотера; иначе голова node/heads.rs и пулы по памяти в том же пакете.
//! Ревью второго прохода:
//! - Н1: точки пробы возвращаются вызывающему (exec/hook_trade.rs) и идут в расчёт сделки напрямую; в хранилище
//!   вливаются под одним замком с чтением записанного (check/hook_store.rs fold_head, check/hook_fold.rs on_success;
//!   без лесенки — хранилище не трогается);
//! - Б2: квотер на сумме сделки отказал («не хватает ликвидности», откат крючка, точка без ответа) — отказ ТОЛЬКО этой
//!   сделке; в лесенке этой стороны точки ≥ x убираются, пул целиком «не прошла» — только если точек стороны не
//!   осталось (check/hook_fold.rs on_failure). Сбой узла, ответ не по виду, незаданная настройка — сделки нет,
//!   записанное не трогается.
//!
//! Кто вызывает: exec/hook_trade.rs.
//! От чего зависит: check/{hook_batch, hook_fold, hook_store, hooks_probe}.rs, node/heads.rs, quote/mod.rs (Quoting).
//! Настройки: через вызываемые — dex.quoter [вид пула], dex.multicall3, dex.tick_words.
//!
//! Квотер исполняет настоящий обмен (с крючками) и откатывается — research/flow/bsc_v4.md, раздел 5.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup, HookStep, PoolRef};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::check::hooks_probe::{Plan, Point};
use super::super::check::{hook_batch, hook_fold, hook_store, hooks_probe};
use super::super::pools::registry::Entry;
use super::super::quote::Quoting;

/// Точка пробы этой сделки: пул, сторона (покупка — вход встречный токен), точка лесенки с выходом квотера.
#[derive(Debug, Clone, PartialEq)]
pub struct Fresh {
    pub pool: PoolRef,
    pub buy: bool,
    pub step: HookStep,
}

/// Пул пробы: запись, группа, план (точки обеих сторон).
type Ask = (Entry, HookGroup, Plan);

/// Что делает: проба (см. шапку). Принимает: окружение котировки, шаги (пул из памяти, токен входа, вход пула в
/// минимальных единицах; пулы без крючка B/C пропускаются, повторы точек — одним вызовом). Возвращает: точки этой
/// сделки. Ошибки: NoRoute — проба на сумме сделки не прошла; StaleData — нет головы; узел; NotConfigured.
pub async fn hooks_again(
    q: &Quoting,
    steps: &[(Entry, Address, U256)],
) -> GatewayResult<Vec<Fresh>> {
    let asks = plans(steps);
    if asks.is_empty() {
        return Ok(Vec::new());
    }
    let head = q
        .heads
        .latest()
        .ok_or_else(|| {
            q.ctx.err(
                ErrorCode::StaleData,
                "нет головы сети для пробы крючка перед сделкой",
            )
        })?
        .number;
    let pools: Vec<(Entry, Plan)> = asks
        .iter()
        .map(|(e, _, p)| (e.clone(), p.clone()))
        .collect();
    let fresh = q.route_fresh.as_deref();
    let (block, got) = hook_batch::measure(&q.ctx, &q.tokens, &q.reg, &pools, fresh, head).await?;
    let mut out = Vec::new();
    let mut failed: Option<String> = None;
    for ((e, group, plan), m) in asks.iter().zip(got) {
        let r = &e.pool.reference;
        let mk = |st: CheckState<_>| hooks_probe::pool_check(&q.ctx, e, *group, st);
        // Отказы по сторонам: (покупка ли, наименьший вход без ответа, причина).
        let mut fails: Vec<(bool, U256, String)> = Vec::new();
        match m {
            Ok(Ok(i)) => {
                for (buy, pts) in [(true, &plan.buy), (false, &plan.sell)] {
                    let got = hook_fold::side(&i, buy);
                    let miss = pts
                        .iter()
                        .map(|p| p.amount_in)
                        .filter(|x| got.iter().all(|s| s.amount_in != *x));
                    if let Some(x) = miss.min() {
                        fails.push((buy, x, "квотер не дал обмен на сумме сделки".into()));
                    }
                    out.extend(got.iter().map(|s| Fresh {
                        pool: r.clone(),
                        buy,
                        step: s.clone(),
                    }));
                }
                hook_store::fold_head(&q.checks, r, block, |pc| match &pc.impact {
                    CheckState::Known(old) => {
                        hook_fold::on_success(old, &i).map(|n| mk(CheckState::Known(n)))
                    }
                    _ => None,
                });
            }
            Ok(Err(reason)) => {
                fails.extend(least(plan).into_iter().map(|(b, x)| (b, x, reason.clone())))
            }
            Err(err) if err.code == ErrorCode::SwapReverted => {
                fails.extend(
                    least(plan)
                        .into_iter()
                        .map(|(b, x)| (b, x, err.message.clone())),
                );
            }
            Err(err) => return Err(err),
        }
        for (buy, x, reason) in fails {
            hook_store::fold_head(&q.checks, r, block, |pc| match &pc.impact {
                CheckState::Known(old) => {
                    hook_fold::on_failure(old, buy, x, reason.clone()).map(mk)
                }
                _ => None,
            });
            if failed.is_none() {
                failed = Some(format!("{} {}: {reason}", e.family.title, e.ticker));
            }
        }
    }
    match failed {
        None => Ok(out),
        Some(why) => {
            let msg = format!("проверка крючка перед сделкой на сумме сделки не прошла: {why}");
            q.ctx.log(Level::Warn, "dex.check", msg.clone(), None).await;
            Err(q.ctx.err(ErrorCode::NoRoute, msg))
        }
    }
}

/// Планы пробы по пулам (чистая): точки пула с крючком B/C по сторонам, без повторов, по возрастанию входа; пулы — в
/// порядке первого появления. Ошибок нет.
fn plans(steps: &[(Entry, Address, U256)]) -> Vec<Ask> {
    let mut out: Vec<Ask> = Vec::new();
    for (e, token_in, amount_in) in steps {
        let Some(g) = e
            .pool
            .hooks
            .as_ref()
            .map(|h| h.group)
            .filter(|g| *g != HookGroup::A)
        else {
            continue;
        };
        let at = match out
            .iter()
            .position(|(x, _, _)| x.pool.reference == e.pool.reference)
        {
            Some(i) => i,
            None => {
                out.push((e.clone(), g, Plan::default()));
                out.len() - 1
            }
        };
        let plan = &mut out[at].2;
        let pts = if *token_in == e.usdt {
            &mut plan.buy
        } else {
            &mut plan.sell
        };
        if pts.iter().all(|p| p.amount_in != *amount_in) {
            pts.push(Point {
                usd: Decimal::ZERO,
                amount_in: *amount_in,
            });
            pts.sort_by_key(|p| p.amount_in);
        }
    }
    out
}

/// Наименьший вход каждой стороны плана (отказ пробы целиком — относится к наименьшей точке). Ошибок нет.
fn least(plan: &Plan) -> Vec<(bool, U256)> {
    let first = |pts: &[Point]| pts.first().map(|p| p.amount_in);
    [(true, first(&plan.buy)), (false, first(&plan.sell))]
        .into_iter()
        .filter_map(|(b, x)| x.map(|x| (b, x)))
        .collect()
}
