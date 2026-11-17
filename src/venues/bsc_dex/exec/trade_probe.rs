//! Файл: venues/bsc_dex/exec/trade_probe.rs
//! Что делает: повторная проба крючков B/C перед сделкой (решение 32) на СУММЕ СДЕЛКИ — точки пробы = входы пулов с
//! крючком в шагах путей сделки (решение «335 (лесенка влияния)»). Решение «335 (В1, пакет)» (check/hook_batch.rs): на
//! ОДНОМ блоке и одним кругом до узла для всех пулов всех долей (ревью 2 Н7; точки одного пула — в одном плане) —
//! графовый маршрут: свежие записи верификации (Quoting.route_fresh) и только вызовы квотера; иначе голова
//! node/heads.rs и пулы по памяти в том же пакете.
//! - Ревью 2 Н1: точки пробы возвращаются вызывающему (exec/hook_trade.rs) и идут в расчёт сделки напрямую; в
//!   хранилище вливаются под одним замком с чтением записанного (check/hook_store.rs fold_head, check/hook_fold.rs
//!   on_success; без лесенки — хранилище не трогается).
//! - Ревью 2 Б2, ревью 3 Н-в: квотер на сумме сделки отказал («не хватает ликвидности», откат крючка —
//!   HookImpact.refused) — отказ ТОЛЬКО этой сделке; в лесенке этой стороны точки ≥ x убираются, сторона «не прошла» —
//!   только если её точек не осталось, пул целиком — только если не прошли обе (check/hook_fold.rs on_failure).
//! - Ревью 3 Н-б: точка без итога, но без отказа квотера (не сошлась НАША математика на блоке пробы) — отказ этой
//!   сделке, хранилище не трогается.
//! - Сбой узла, ответ не по виду, незаданная настройка — сделки нет, записанное не трогается.
//! - Ревью 4: точек сделки в хранилище — не больше hooks.impact_points на сторону (Н1, check/hook_trim.rs); вход
//!   точки — и в токене входа для экрана (Н3); повторная проба сделки — на блоке первой (Н7, pin).
//!
//! Кто вызывает: exec/hook_trade.rs.
//! От чего зависит: check/{hook_batch, hook_fold, hook_store, hooks_probe}.rs, exec/hook_drive.rs (Fresh), node/heads.rs.
//! Настройки: hooks.impact_points (предел точек сделки в хранилище, ревью 4 Н1); через вызываемые — dex.quoter [вид
//! пула], dex.multicall3, dex.tick_words.
//!
//! Квотер исполняет настоящий обмен (с крючками) и откатывается — research/flow/bsc_v4.md, раздел 5.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::CheckState;
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::check::hooks_probe::Plan;
use super::super::check::{hook_batch, hook_fold, hook_points, hook_store, hooks_probe};
use super::super::pools::registry::Entry;
use super::super::quote::Quoting;
use super::hook_drive::Fresh;
use super::trade_probe_plan;

/// Шаг пробы: пул из памяти, токен входа, вход пула в минимальных единицах, влияние, применённое на нём до пробы.
pub type Ask = (Entry, Address, U256, Decimal);

/// Что делает: проба (см. шапку). Принимает: окружение котировки, шаги (пулы без крючка B/C пропускаются, повторы
/// точек — одним вызовом), блок первой пробы этой сделки (ревью 4 Н7: повторная проба — на нём же; None — первая
/// проба: голова или блок верификации маршрута). Возвращает: точки этой сделки и блок пробы. Ошибки: NoRoute — проба на
/// сумме сделки не прошла; StaleData — нет головы; узел; NotConfigured.
pub async fn hooks_again(
    q: &Quoting,
    steps: &[Ask],
    pin: Option<u64>,
) -> GatewayResult<(Vec<Fresh>, u64)> {
    let asks = trade_probe_plan::plans(steps);
    let head = match pin {
        Some(b) => b,
        None => {
            q.heads
                .latest()
                .ok_or_else(|| {
                    q.ctx.err(
                        ErrorCode::StaleData,
                        "нет головы сети для пробы крючка перед сделкой",
                    )
                })?
                .number
        }
    };
    if asks.is_empty() {
        return Ok((Vec::new(), head));
    }
    let pools: Vec<(Entry, Plan)> = asks
        .iter()
        .map(|(e, _, p, _)| (e.clone(), p.clone()))
        .collect();
    // Ревью 4 (Н7): повторная проба — на блоке первой: записи верификации — только того же блока, иначе пулы читаются
    // на нём в том же пакете (check/hook_batch.rs: блок = записи верификации или переданный).
    let fresh = q
        .route_fresh
        .as_deref()
        .filter(|f| pin.is_none_or(|b| f.block == b));
    let (block, got) = hook_batch::measure(&q.ctx, &q.tokens, &q.reg, &pools, fresh, head).await?;
    // Ревью 4 (Н1): точек сделки в хранилище — не больше hooks.impact_points на сторону (не задана — по точкам цикла).
    let cap = q
        .ctx
        .int(hook_points::POINTS)
        .ok()
        .and_then(|n| usize::try_from(n).ok());
    let mut out = Vec::new();
    let mut failed: Option<String> = None;
    for ((e, group, plan, est), m) in asks.iter().zip(got) {
        let r = &e.pool.reference;
        let mk = |st: CheckState<_>| hooks_probe::pool_check(&q.ctx, e, *group, st);
        let mut i = match m {
            Ok(Ok(i)) => i,
            // Ответ не по виду или иной отказ разбора — отказ сделке, записанное не трогается.
            Ok(Err(why)) => {
                note(
                    &mut failed,
                    format!("{} {}: {why}", e.family.title, e.ticker),
                );
                continue;
            }
            Err(err) => return Err(err),
        };
        // Ревью 4 (Н3): вход точки — и в токене входа стороны (decimals из памяти, без узла) — для экрана.
        trade_probe_plan::human(&q.tokens, e, &mut i);
        // Отказы квотера по сторонам — наименьший вход стороны с отказом.
        let mut cuts: Vec<(bool, U256, String)> = Vec::new();
        for (buy, x, why) in &i.refused {
            match cuts.iter_mut().find(|c| c.0 == *buy) {
                Some(c) if *x < c.1 => *c = (*buy, *x, why.clone()),
                Some(_) => {}
                None => cuts.push((*buy, *x, why.clone())),
            }
        }
        for (buy, pts) in [(true, &plan.buy), (false, &plan.sell)] {
            let got = hook_fold::side(&i, buy);
            for p in pts {
                match got.iter().find(|s| s.amount_in == p.amount_in) {
                    Some(s) => out.push(Fresh {
                        pool: r.clone(),
                        buy,
                        step: s.clone(),
                        est: est
                            .iter()
                            .find(|t| t.0 == buy && t.1 == p.amount_in)
                            .map_or(Decimal::ZERO, |t| t.2),
                    }),
                    None if cuts.iter().any(|c| c.0 == buy && c.1 <= p.amount_in) => {}
                    None => {
                        let why = format!(
                            "наша математика пула не сошлась на сумме сделки (вход {})",
                            p.amount_in
                        );
                        note(
                            &mut failed,
                            format!("{} {}: {why}", e.family.title, e.ticker),
                        );
                    }
                }
            }
        }
        hook_store::fold_head(&q.checks, r, block, |pc| match &pc.impact {
            CheckState::Known(old) => {
                hook_fold::on_success(old, &i, cap).map(|n| mk(CheckState::Known(n)))
            }
            _ => None,
        });
        for (buy, x, reason) in cuts {
            hook_store::fold_head(&q.checks, r, block, |pc| match &pc.impact {
                CheckState::Known(old) => {
                    hook_fold::on_failure(old, buy, x, reason.clone()).map(mk)
                }
                _ => None,
            });
            note(
                &mut failed,
                format!("{} {}: {reason}", e.family.title, e.ticker),
            );
        }
    }
    match failed {
        None => Ok((out, block)),
        Some(why) => {
            let msg = format!("проверка крючка перед сделкой на сумме сделки не прошла: {why}");
            q.ctx.log(Level::Warn, "dex.check", msg.clone(), None).await;
            Err(q.ctx.err(ErrorCode::NoRoute, msg))
        }
    }
}

/// Первая причина отказа (остальные — та же сделка). Ошибок нет.
fn note(failed: &mut Option<String>, why: String) {
    if failed.is_none() {
        *failed = Some(why);
    }
}
