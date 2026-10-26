//! Файл: venues/bsc_dex/check/quoter_verify.rs
//! Что делает: сверка одного пула с квотером своей DEX (после фильтра ликвидности): пустой пул / не хватает
//! ликвидности на сумму — честная причина; у пула с крючком B/C — за вычетом измеренного влияния крючка на сторону;
//! покупка и продажа — независимо (решение 73);
//! сумма — check.probe_usdt USDT (для пула с промежуточной монетой — пересчёт по живому пулу, решение 52).
//! Кто вызывает: check/quoter_cycle.rs.
//! От чего зависит: quote/quoter.rs, quote/emptiness.rs, quote/rate.rs, check/state.rs, pools/registry.rs, tokens.rs,
//! num.rs.
//! Настройки: dex.quoter_max_diff_pct (через quoter.rs), check.probe_usdt [bsc_dex].

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup, Side};
use crate::gateway::errors::ErrorCode;
use crate::storage::journal::Level;

use super::super::num;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::super::quote::{emptiness, quoter, rate};
use super::state::Checks;

/// Причина «влияние крючка ещё не измерено»: такой пул сверяется снова, как только крючок измерен (check/cycle.rs
/// будит цикл сигналом «пул готов», check/quoter_cycle.rs hook_measured).
pub const HOOK_WAIT: &str = "влияние крючка ещё не измерено — сверка с квотером невозможна";

/// Сверка стороны пула (после фильтра глубины этой стороны; решение 73 — стороны независимо): покупка — check.probe_usdt
/// USDT во встречном токене в пул; продажа — монеты на эту сумму по текущей цене пула (quote/spot.rs, без обмена). Возвращает: Ok или Err(причина). Ошибок нет.
pub async fn verify(p: &Pools, e: &Entry, side: Side) -> Result<(), String> {
    if emptiness::empty(e) {
        return Err(emptiness::EMPTY.into());
    }
    let pct = hook_pct(&p.checks, e, side)?;
    let (amount, usdt) = probe(p, e).await?;
    let (token_in, amount) = match side {
        Side::Buy => (e.usdt, amount),
        // Продажа — монеты на ту же сумму по ТЕКУЩЕЙ цене пула, без обмена на стороне покупки (решение 73): у пула с
        // ликвидностью только с одной стороны цены покупка невозможна, а продажа глубокая — стороны независимы.
        Side::Sell => match super::super::quote::spot::coins_for(e, amount) {
            Some(c) if !c.is_zero() => (e.coin, c),
            _ => return Err("цены пула нет — монет на сумму пробы продажи не посчитать".into()),
        },
    };
    match quoter::compare(&p.ctx, e, token_in, amount, pct).await {
        Ok(Err(why)) if why.starts_with(emptiness::SHORT) => {
            Err(format!("{why}, check.probe_usdt = {usdt} USDT"))
        }
        Ok(r) => r,
        // Не задана настройка (например dex.quoter вида пула) — одна запись на причину, не на пул.
        Err(err) if err.code == ErrorCode::NotConfigured => {
            let msg = format!(
                "{}: сверка с квотером невозможна — {}",
                e.family.title, err.message
            );
            p.signals
                .once(&p.ctx, Level::Error, "dex.quoter", msg)
                .await;
            Err(err.message)
        }
        Err(err) => Err(err.message),
    }
}

/// Знаки USDT. Err — причина (не задан тикер, узел). Ошибок нет.
pub async fn usdt_decimals(p: &Pools) -> Result<u8, String> {
    let usdt = p.tokens.usdt(&p.ctx).map_err(|e| e.message)?;
    p.tokens.decimals(&p.ctx, usdt).await.map_err(|e| e.message)
}

/// Влияние крючка на сторону, %: 0 — крючка B/C нет. Err — влияние ещё не измерено или проверка крючка не прошла.
fn hook_pct(checks: &Checks, e: &Entry, side: Side) -> Result<Decimal, String> {
    if e.pool
        .hooks
        .as_ref()
        .is_none_or(|h| h.group == HookGroup::A)
    {
        return Ok(Decimal::ZERO);
    }
    match checks.pool(&e.pool.reference).hook.map(|c| c.impact) {
        Some(CheckState::Known(i)) => Ok(if side == Side::Buy {
            i.buy_pct
        } else {
            i.sell_pct
        }),
        // Не хватает ликвидности на пробу — свойство пула, не провал крючка: причина как есть.
        Some(CheckState::Failed { reason }) if reason.starts_with(super::hooks_probe::SHORT) => {
            Err(reason)
        }
        Some(CheckState::Failed { reason }) => Err(format!("проверка крючка не прошла: {reason}")),
        _ => Err(HOOK_WAIT.into()),
    }
}

/// Сумма сверки во встречном токене пула: check.probe_usdt USDT, для пула с промежуточной монетой (решение 52) —
/// пересчёт по живому пулу «промежуточная — USDT» (quote/rate.rs). Возвращает: (сумма, check.probe_usdt). Err — причина.
async fn probe(p: &Pools, e: &Entry) -> Result<(U256, Decimal), String> {
    let usdt = p.tokens.usdt(&p.ctx).map_err(|e| e.message)?;
    let dec = usdt_decimals(p).await?;
    let v = p.ctx.dec("check.probe_usdt").map_err(|e| e.message)?;
    let amount = num::to_units(v, dec).ok_or("check.probe_usdt не переводится в единицы USDT")?;
    let amount = rate::from_usdt(&p.ctx, &p.reg, usdt, e.usdt, amount).map_err(|e| e.message)?;
    Ok((amount, v))
}

/// Какая настройка нужна сверке (и пробе крючка) пула и не задана: глубина (pools/depth_book.rs), тикер USDT,
/// check.probe_usdt, dex.quoter_max_diff_pct, адрес квотера или роутера (V2) вида пула. Имя — из ошибки хранилища
/// (GatewayError.setting), не из текста: по нему итог перепроверяется при задании именно этой настройки (решение 75).
/// None — всё задано (итог — не «не задана настройка»). Ошибок нет.
pub fn missing_setting(p: &Pools, e: &Entry) -> Option<String> {
    let ctx = &p.ctx;
    if let Some(n) = super::super::pools::depth_book::missing_setting(ctx) {
        return Some(n);
    }
    let addr = if e.pool.kind == crate::core::types::PoolKind::V2 {
        "dex.router"
    } else {
        "dex.quoter"
    };
    [
        p.tokens.usdt(ctx).err(),
        ctx.dec("check.probe_usdt").err(),
        ctx.dec("dex.quoter_max_diff_pct").err(),
        ctx.dex_addr(addr, e.family.id).err(),
    ]
    .into_iter()
    .flatten()
    .find_map(|err| err.setting)
}
