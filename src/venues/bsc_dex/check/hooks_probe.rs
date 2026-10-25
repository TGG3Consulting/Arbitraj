//! Файл: venues/bsc_dex/check/hooks_probe.rs
//! Что делает: влияние крючка пула V4 группы B/C (решение 32) — пробный обмен квотером своей DEX (он исполняет
//! крючки) против локальной математики пула без крючка, на сумме check.probe_usdt: покупка (USDT → монета)
//! и продажа (полученные монеты → USDT). Пул с промежуточной монетой (решение 52) — проба в ней (quote/rate.rs). Влияние, % = (математика − квотер) / математика · 100; положительное —
//! получаем меньше. Ошибка пробы (откат квотера, крючок требует hookData и т.п.) — Failed с причиной; откат квотера
//! NotEnoughLiquidity(PoolId) — причина «в пуле не хватает ликвидности для пробы check.probe_usdt (… PoolId …)», не
//! «проверка крючка не прошла».
//! Кто вызывает: check/cycle.rs.
//! От чего зависит: quote/quoter.rs, quote/pool_quote.rs, check/state.rs, pools/registry.rs, tokens.rs, num.rs.
//! Настройки: check.probe_usdt [bsc_dex]; dex.quoter [вид пула V4].
//!
//! Квотер выполняет настоящий swap внутри vault.lock и откатывается — IP/src/pool-cl/lens/CLQuoter.sol#L27-L38
//! (research/flow/bsc_v4.md, раздел 5): «hook-и пула в квоте исполняются».

use alloy_primitives::U256;
use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, CheckState, HookGroup, HookImpact, PoolCheck};
use crate::gateway::errors::GatewayResult;

use super::super::ctx::Ctx;
use super::super::num;
use super::super::pools::registry::{Entry, Registry};
use super::super::quote::{pool_quote, quoter, quoter_revert, rate};
use super::super::tokens::Tokens;

/// Проверка пула. Возвращает: PoolCheck (None — у пула нет крючка группы B/C, проверять нечего).
pub async fn check_pool(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    e: &Entry,
) -> Option<PoolCheck> {
    let hooks = e.pool.hooks.as_ref()?;
    if hooks.group == HookGroup::A {
        return None;
    }
    let impact = match measure(ctx, tokens, reg, e).await {
        Ok(Ok(i)) => CheckState::Known(i),
        Ok(Err(reason)) => CheckState::Failed { reason },
        Err(err) => CheckState::Failed {
            reason: err.message,
        },
    };
    Some(PoolCheck {
        venue: ctx.venue(),
        family: e.family.id.to_string(),
        pool: e.pool.reference.clone(),
        asset: Asset {
            ticker: e.ticker.clone(),
            bsc_contract: Some(e.coin),
        },
        group: hooks.group,
        impact,
        checked_at: Some(Utc::now()),
    })
}

/// Начало причины «в пуле не хватает ликвидности для пробы» — не провал крючка, а свойство пула на сумму пробы:
/// так причина и показывается (check/quoter_verify.rs, quote/select.rs, check/cycle.rs — без «проверка крючка не
/// прошла»).
pub const SHORT: &str = "в пуле не хватает ликвидности для пробы check.probe_usdt";

/// Причина по откату квотера NotEnoughLiquidity(PoolId). Ошибок нет.
fn short(id: alloy_primitives::B256) -> String {
    format!("{SHORT} (NotEnoughLiquidity, PoolId {id})")
}

/// Квотер на пробу; откат «не хватает ликвидности» (UnexpectedRevertBytes(NotEnoughLiquidity(PoolId)),
/// quote/quoter_revert.rs) — Ok(Err(причина)). Ошибки: как у quoter::out.
async fn ask(
    ctx: &Ctx,
    e: &Entry,
    token_in: alloy_primitives::Address,
    amount: U256,
) -> GatewayResult<Result<U256, String>> {
    match quoter::out(ctx, e, token_in, amount).await {
        Ok(v) => Ok(Ok(v)),
        Err(err) => match quoter_revert::short_pool(&err) {
            Some(id) => Ok(Err(short(id))),
            None => Err(err),
        },
    }
}

/// Покупка и продажа: математика против квотера. Ok(Err) — посчитать нельзя (причина).
async fn measure(
    ctx: &Ctx,
    tokens: &Tokens,
    reg: &Registry,
    e: &Entry,
) -> GatewayResult<Result<HookImpact, String>> {
    let usdt = tokens.usdt(ctx)?;
    let usdt_dec = tokens.decimals(ctx, usdt).await?;
    let Some(p) = num::to_units(ctx.dec("check.probe_usdt")?, usdt_dec) else {
        return Ok(Err("check.probe_usdt не переводится в единицы USDT".into()));
    };
    // Пул с промежуточной монетой (решение 52): проба — в ней, по живому пулу «промежуточная — USDT».
    let p = match rate::from_usdt(ctx, reg, usdt, e.usdt, p) {
        Ok(a) => a,
        Err(err) => return Ok(Err(err.message)),
    };
    let local_buy = pool_quote::swap(ctx, e, e.usdt, true, p)?;
    let remote_buy = match ask(ctx, e, e.usdt, p).await? {
        Ok(v) => v,
        Err(why) => return Ok(Err(why)),
    };
    let coins = local_buy.amount_out;
    let local_sell = pool_quote::swap(ctx, e, e.coin, true, coins)?;
    let remote_sell = match ask(ctx, e, e.coin, coins).await? {
        Ok(v) => v,
        Err(why) => return Ok(Err(why)),
    };
    match (
        impact(local_buy.amount_out, remote_buy),
        impact(local_sell.amount_out, remote_sell),
    ) {
        (Some(buy_pct), Some(sell_pct)) => Ok(Ok(HookImpact { buy_pct, sell_pct })),
        _ => Ok(Err("математика пула дала 0 — влияние не определить".into())),
    }
}

/// (математика − квотер) / математика · 100. None — математика 0 или числа не помещаются в Decimal.
fn impact(local: U256, remote: U256) -> Option<Decimal> {
    let l = num::to_decimal(local, 0)?;
    let r = num::to_decimal(remote, 0)?;
    if l.is_zero() {
        return None;
    }
    Some((l - r) / l * Decimal::ONE_HUNDRED)
}
