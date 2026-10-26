//! Файл: venues/bsc_dex/exec/precheck.rs
//! Что делает: проверки прямо перед сделкой (решение 32: «перед каждой сделкой — повторная пробная проверка и
//! „получить не меньше“»), общие для прямого и умного режима: повторное измерение влияния крючка B/C пула пробным
//! обменом квотером (итог — в состояние проверок, и по нему же пул проходит или не проходит допуск quote/select.rs);
//! сверка обмена с квотером своей DEX на реальной сумме сделки (прямой режим; умный сверяет доли MixedQuoter —
//! quote/mixed_quoter.rs).
//! Кто вызывает: exec/trade.rs, exec/trade_smart.rs.
//! От чего зависит: check/hooks_probe.rs, check/state.rs, quote/quoter.rs, quote/emptiness.rs, pools/registry.rs.
//! Настройки: через вызываемые — check.probe_usdt, dex.quoter [вид пула], dex.router (V2), dex.quoter_max_diff_pct.
//!
//! Квотер исполняет настоящий обмен (с крючками) и откатывается — research/flow/bsc_v4.md, раздел 5; «Сверка:
//! периодически и перед сделкой — квотер» — research/flow/bsc_dex.md, схема адаптера п. 6.

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::storage::journal::Level;

use super::super::check::hooks_probe;
use super::super::ctx::Ctx;
use super::super::math::swap_cl::Swapped;
use super::super::pools::registry::Entry;
use super::super::quote::{Quoting, emptiness, quoter, quoter_revert};

/// Повторная пробная проверка крючка пула группы B/C (у пула без крючка B/C — ничего). Итог пишется в состояние
/// проверок: допуск пула (quote/select.rs) считается уже по нему. Ошибки: NoRoute — проба не прошла.
pub async fn hook_again(q: &Quoting, e: &Entry) -> GatewayResult<()> {
    if e.pool
        .hooks
        .as_ref()
        .is_none_or(|h| h.group == HookGroup::A)
    {
        return Ok(());
    }
    let Some(pc) = hooks_probe::check_pool(&q.ctx, &q.tokens, &q.reg, e).await else {
        return Ok(());
    };
    let failed = match &pc.impact {
        CheckState::Failed { reason } => Some(reason.clone()),
        _ => None,
    };
    q.checks
        .update_pool(&e.pool.reference, |f| f.hook = Some(pc));
    match failed {
        None => Ok(()),
        Some(why) => {
            let msg = format!(
                "{} {}: проверка крючка перед сделкой не прошла: {why}",
                e.family.title, e.ticker
            );
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
    quoter::within(ctx, local.amount_out, remote, hook_pct)?.map_err(|why| {
        ctx.err(
            ErrorCode::QuoteMismatch,
            format!("{} {}: перед сделкой {why}", e.family.title, e.ticker),
        )
    })
}
