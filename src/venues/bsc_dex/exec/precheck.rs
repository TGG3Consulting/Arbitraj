//! Файл: venues/bsc_dex/exec/precheck.rs
//! Что делает: проверки прямо перед сделкой (решение 32: «перед каждой сделкой — повторная пробная проверка и
//! „получить не меньше“»), общие для прямого и умного режима: повторное измерение влияния крючка B/C пула пробным
//! обменом квотером (итог — в состояние проверок, и по нему же пул проходит или не проходит допуск quote/select.rs);
//! сверка обмена с квотером своей DEX на реальной сумме сделки (прямой режим; умный сверяет доли MixedQuoter —
//! quote/mixed_quoter.rs). Решение 177: у продажи выхода расхождение — не отказ, а продажа по квотеру (quoter_hop).
//! Решение 261 (Тигран 12.11: «если в нашу сторону то ок если наоборот то не делаем»): у остальных сделок сверка
//! односторонняя — выход квотера меньше нашего больше допуска — отказ; больше (на любую величину) — сделка идёт,
//! поправка с favour = true (только строка в хронологию; ожидаемое и минимум — по нашему расчёту).
//! Кто вызывает: exec/trade.rs, exec/trade_smart.rs, exec/exit_quote.rs.
//! От чего зависит: check/hook_batch.rs, check/hooks_probe.rs, check/hook_store.rs, node/heads.rs,
//! check/state.rs, quote/quoter.rs, quote/emptiness.rs, pools/registry.rs.
//! Настройки: через вызываемые — check.probe_usdt, dex.quoter [вид пула], dex.router (V2), dex.quoter_max_diff_pct.
//!
//! Квотер исполняет настоящий обмен (с крючками) и откатывается — research/flow/bsc_v4.md, раздел 5; «Сверка:
//! периодически и перед сделкой — квотер» — research/flow/bsc_dex.md, схема адаптера п. 6.

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::check::hooks_probe::{Plan, Point};
use super::super::check::{hook_batch, hook_store, hooks_probe};
use super::super::ctx::Ctx;
use super::super::math::swap_cl::Swapped;
use super::super::pools::registry::Entry;
use super::super::quote::quoter::Judged;
use super::super::quote::{Quoting, emptiness, hook_curve, quoter, quoter_revert};
use super::exit_quote::ExitFix;

/// Повторная пробная проверка крючков B/C пулов доли (пулы без крючка B/C пропускаются) — на СУММЕ СДЕЛКИ: точка
/// пробы = вход пула в шаге доли (решение «335 (лесенка влияния)»: влияние растёт с суммой). Решение «335 (В1,
/// пакет)» (check/hook_batch.rs): на ОДНОМ блоке и одним кругом до узла — графовый маршрут: свежие записи
/// верификации (Quoting.route_fresh) и только вызовы квотера; иначе: голова node/heads.rs, slot0, слова и тики по
/// памяти и квотер всех пулов одним пакетом. Итог по пулу вливается в лесенку стороны (quote/hook_curve.rs
/// merge_impact) и пишется check/hook_store.rs put_head — расчёт доли после пробы берёт влияние на своём входе из
/// обновлённой лесенки. Откат квотера — итог «не прошла». Сбой узла, ответ не по виду, незаданная настройка —
/// сделки нет, прежний итог проверки крючка не трогается.
/// Принимает: окружение котировки, шаги доли (пул из памяти, токен входа, вход пула в минимальных единицах).
/// Ошибки: NoRoute — проба не прошла; StaleData — нет головы; узел; NotConfigured.
pub async fn hooks_again(q: &Quoting, steps: &[(Entry, Address, U256)]) -> GatewayResult<()> {
    let hooked: Vec<(Entry, HookGroup, Plan)> = steps
        .iter()
        .filter_map(|(e, token_in, amount_in)| {
            let g = e.pool.hooks.as_ref()?.group;
            let p = Point {
                usd: Decimal::ZERO,
                amount_in: *amount_in,
            };
            let plan = if *token_in == e.usdt {
                Plan {
                    buy: vec![p],
                    sell: Vec::new(),
                }
            } else {
                Plan {
                    buy: Vec::new(),
                    sell: vec![p],
                }
            };
            (g != HookGroup::A).then(|| (e.clone(), g, plan))
        })
        .collect();
    if hooked.is_empty() {
        return Ok(());
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
    let pools: Vec<(Entry, Plan)> = hooked
        .iter()
        .map(|(e, _, p)| (e.clone(), p.clone()))
        .collect();
    let fresh = q.route_fresh.as_deref();
    let (block, got) = hook_batch::measure(&q.ctx, &q.tokens, &q.reg, &pools, fresh, head).await?;
    let mut failed = None;
    for ((e, group, _), m) in hooked.iter().zip(got) {
        let impact = match m {
            Ok(Ok(i)) => {
                let old = q
                    .checks
                    .pool(&e.pool.reference)
                    .hook
                    .and_then(|c| match c.impact {
                        CheckState::Known(i) => Some(i),
                        _ => None,
                    });
                // Прежней лесенки нет — точку не к чему приложить (другая сторона осталась бы без влияния): итог не
                // пишется, расчёт доли скажет «влияние крючка ещё не измерено».
                let Some(old) = old else {
                    continue;
                };
                CheckState::Known(hook_curve::merge_impact(Some(&old), &i))
            }
            Ok(Err(reason)) => CheckState::Failed { reason },
            // Откат квотера (крючок не дал обмен) — итог пула «не прошла»; прочее (узел, ответ не по виду — например
            // блок головы ещё не у узла HTTP, незаданная настройка) — сделки нет, прежний итог не трогается.
            Err(err) if err.code == ErrorCode::SwapReverted => CheckState::Failed {
                reason: err.message,
            },
            Err(err) => return Err(err),
        };
        if let (None, CheckState::Failed { reason }) = (&failed, &impact) {
            failed = Some(format!("{} {}: {reason}", e.family.title, e.ticker));
        }
        let pc = hooks_probe::pool_check(&q.ctx, e, *group, impact);
        hook_store::put_head(&q.checks, &e.pool.reference, block, &pc, None);
    }
    match failed {
        None => Ok(()),
        Some(why) => {
            let msg = format!("проверка крючка перед сделкой не прошла: {why}");
            q.ctx.log(Level::Warn, "dex.check", msg.clone(), None).await;
            Err(q.ctx.err(ErrorCode::NoRoute, msg))
        }
    }
}

/// Сверка обмена сделки с квотером своей DEX. Принимает: пул, токен входа, локальный обмен в пуле (сколько доходит
/// до пула, выход, полон ли расчёт), влияние крючка на эту сторону (%). Пустой пул или неполный расчёт — квотер не
/// вызывается (quote/emptiness.rs). Ошибки: NoRoute (пустой пул, не хватает ликвидности), QuoteMismatch (расхождение —
/// с цифрами), настройки, узел (откат квотера — его текст).
pub async fn quoter_now(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    local: &Swapped,
    hook_pct: Decimal,
) -> GatewayResult<()> {
    quoter_hop(ctx, e, token_in, local, hook_pct, false)
        .await
        .map(|_| ())
}

/// Как quoter_now, но продажа выхода (решение 177, exit = true): расхождение больше допуска с цифрами (Judged::Diff) —
/// не отказ, а Some(ExitFix) — продать по выходу квотера (exec/exit_quote.rs). Пустой пул, неполный расчёт, откат
/// квотера, «сравнить нельзя» — отказ, как у quoter_now. exit = false — ровно quoter_now. Возвращает: None — в допуске;
/// Some(ExitFix с favour = true) — решение 261, расхождение в нашу сторону (exit = false). Ошибки: как у quoter_now.
pub async fn quoter_hop(
    ctx: &Ctx,
    e: &Entry,
    token_in: Address,
    local: &Swapped,
    hook_pct: Decimal,
    exit: bool,
) -> GatewayResult<Option<ExitFix>> {
    let short = if emptiness::empty(e) {
        Some(emptiness::EMPTY.to_string())
    } else if !local.complete {
        Some(emptiness::short(emptiness::SHORT_LOCAL))
    } else {
        None
    };
    let no_route = |why: String| {
        let msg = format!("{} {}: перед сделкой {why}", e.family.title, e.ticker);
        ctx.err(ErrorCode::NoRoute, msg)
    };
    if let Some(why) = short {
        return Err(no_route(why));
    }
    // Настройки — до обращения к узлу (ревью #65).
    ctx.dec("dex.quoter_max_diff_pct")?;
    let remote = match quoter::out(ctx, e, token_in, local.amount_in).await {
        Ok(r) => r,
        Err(err) => {
            return Err(match quoter_revert::liquidity_short(&err) {
                Some(why) => no_route(emptiness::short(why)),
                None => err,
            });
        }
    };
    verdict(
        ctx,
        e,
        quoter::judge(ctx, local.amount_out, remote, hook_pct)?,
        exit,
    )
}

/// Решение по итогу сверки (без узла — проверяется тестами на цифрах живого случая): в допуске — None; расхождение
/// у продажи выхода — Some(ExitFix) (решение 177, по модулю, как было); у остальных расхождение в нашу сторону
/// (квотер больше расчёта) — Some(ExitFix, favour = true) (решение 261); иначе — отказ QuoteMismatch с тем же текстом,
/// что до решения 177. Принимает: окружение, пул, итог сверки, продажа ли выхода. Ошибки: QuoteMismatch.
pub fn verdict(ctx: &Ctx, e: &Entry, j: Judged, exit: bool) -> GatewayResult<Option<ExitFix>> {
    let favour = !exit && j.favours_us();
    match j {
        Judged::Within => Ok(None),
        Judged::Diff {
            local, remote, pct, ..
        } if exit || favour => Ok(Some(ExitFix {
            pool: format!("{} {}", e.family.title, e.ticker),
            local,
            remote,
            pct,
            favour,
        })),
        j => Err(ctx.err(
            ErrorCode::QuoteMismatch,
            format!(
                "{} {}: перед сделкой {}",
                e.family.title,
                e.ticker,
                j.reason().unwrap_or_default()
            ),
        )),
    }
}
