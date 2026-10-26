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

/// Отказ сверки: причина и, если это «не задана настройка», её имя — из ошибки там, где она возникла
/// (GatewayError.setting): по нему сторона перепроверяется при задании именно этой настройки (решение 75).
pub type Fail = (String, Option<String>);

/// Причина без имени настройки. Ошибок нет.
fn plain(why: impl Into<String>) -> Fail {
    (why.into(), None)
}

/// Ошибка шлюза → отказ с именем настройки, если оно есть. Ошибок нет.
fn from_err(e: crate::gateway::errors::GatewayError) -> Fail {
    (e.message, e.setting)
}

/// Сверка стороны пула (после фильтра глубины этой стороны; решение 73 — стороны независимо): покупка — check.probe_usdt
/// USDT во встречном токене в пул; продажа — монеты на эту сумму по текущей цене пула (quote/spot.rs, без обмена).
/// Возвращает: Ok или Err(причина, имя незаданной настройки). Ошибок нет.
pub async fn verify(p: &Pools, e: &Entry, side: Side) -> Result<(), Fail> {
    if emptiness::empty(e) {
        return Err(plain(emptiness::EMPTY));
    }
    let pct = hook_pct(&p.checks, e, side).map_err(plain)?;
    let (amount, usdt) = probe(p, e).await?;
    let (token_in, amount) = match side {
        Side::Buy => (e.usdt, amount),
        // Продажа — монеты на ту же сумму по ТЕКУЩЕЙ цене пула, без обмена на стороне покупки (решение 73): у пула с
        // ликвидностью только с одной стороны цены покупка невозможна, а продажа глубокая — стороны независимы.
        Side::Sell => match super::super::quote::spot::coins_for(e, amount) {
            Some(c) if !c.is_zero() => (e.coin, c),
            _ => {
                return Err(plain(
                    "цены пула нет — монет на сумму пробы продажи не посчитать",
                ));
            }
        },
    };
    match quoter::compare(&p.ctx, e, token_in, amount, pct).await {
        Ok(Err(why)) if why.starts_with(emptiness::SHORT) => {
            Err(plain(format!("{why}, check.probe_usdt = {usdt} USDT")))
        }
        Ok(r) => r.map_err(plain),
        // Не задана настройка (например dex.quoter вида пула) — одна запись на причину, не на пул.
        Err(err) if err.code == ErrorCode::NotConfigured => {
            let msg = format!(
                "{}: сверка с квотером невозможна — {}",
                e.family.title, err.message
            );
            p.signals
                .once(&p.ctx, Level::Error, "dex.quoter", msg)
                .await;
            Err(from_err(err))
        }
        Err(err) => Err(from_err(err)),
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
/// пересчёт по живому пулу «промежуточная — USDT» (quote/rate.rs). Возвращает: (сумма, check.probe_usdt). Err — причина и имя незаданной настройки.
async fn probe(p: &Pools, e: &Entry) -> Result<(U256, Decimal), Fail> {
    let usdt = p.tokens.usdt(&p.ctx).map_err(from_err)?;
    let usdt_dec = p.tokens.decimals(&p.ctx, usdt).await.map_err(from_err)?;
    let v = p.ctx.dec("check.probe_usdt").map_err(from_err)?;
    let amount = num::to_units(v, usdt_dec).ok_or_else(|| {
        let why = "check.probe_usdt не переводится в единицы USDT";
        (why.to_string(), Some("check.probe_usdt".to_string()))
    })?;
    let amount = rate::from_usdt(&p.ctx, &p.reg, usdt, e.usdt, amount).map_err(from_err)?;
    Ok((amount, v))
}
