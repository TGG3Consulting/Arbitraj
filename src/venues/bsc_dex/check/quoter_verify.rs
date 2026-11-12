//! Файл: venues/bsc_dex/check/quoter_verify.rs
//! Что делает: сверка одной стороны пула с квотером своей DEX (после фильтра ликвидности): пустой пул / не хватает
//! ликвидности на сумму — честная причина; у пула с крючком B/C — за вычетом измеренного влияния крючка на сторону;
//! покупка и продажа — независимо (решение 73).
//! Решение 261 (Тигран 12.11): сверка — на ОДНОМ блоке (check/quoter_compare.rs, check/quoter_block.rs); сумма стороны —
//! min(наш максимум money.max_per_trade_usd среди направлений с DEX, ликвидность стороны в полосе) USDT
//! (check/quoter_amount.rs; пул с крючком B/C — прежняя check.probe_usdt), для пула с промежуточной монетой — пересчёт
//! во встречный токен по живому пулу (решение 52, quote/rate.rs). Итог: прошла / пауза (расхождение на одном блоке) /
//! иная причина / «не сверено (блок)».
//! Кто вызывает: check/quoter_cycle.rs.
//! От чего зависит: check/quoter_compare.rs, check/quoter_amount.rs, quote/emptiness.rs, quote/rate.rs, quote/spot.rs,
//! check/state.rs, pools/registry.rs, tokens.rs, num.rs.
//! Настройки: dex.quoter_max_diff_pct (через quoter_compare.rs), money.max_per_trade_usd, check.probe_usdt [bsc_dex]
//! (через quoter_amount.rs).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup, Side};
use crate::gateway::errors::ErrorCode;
use crate::storage::journal::Level;

use super::super::num;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::super::quote::{emptiness, rate};
use super::quoter_amount;
use super::quoter_compare::{self, Cmp, Nums};
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

/// Итог сверки стороны (решение 261).
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Сверка на одном блоке прошла (блок).
    Ok { block: u64 },
    /// Расхождение с квотером на одном блоке больше допуска — пауза стороны: причина с цифрами, блок, сумма USDT, цифры.
    Pause {
        why: String,
        block: u64,
        usd: Decimal,
        nums: Option<Nums>,
    },
    /// Иная причина — сторона исключена (как раньше), без паузы.
    Fail(Fail),
    /// «Не сверено (блок)» — итог стороны прежний, причина видна на экране.
    NoBlock(String),
}

/// Сверка стороны пула (после фильтра глубины этой стороны; решение 73 — стороны независимо): покупка — сумма стороны
/// во встречном токене в пул; продажа — монеты на эту сумму по текущей цене пула (quote/spot.rs, без обмена).
/// Принимает: пулы, запись, знаки USDT, сторону. Возвращает: итог. Ошибок нет.
pub async fn verify(p: &Pools, e: &Entry, usdt_dec: u8, side: Side) -> Verdict {
    if emptiness::empty(e) {
        return Verdict::Fail(plain(emptiness::EMPTY));
    }
    let pct = match hook_pct(&p.checks, e, side) {
        Ok(v) => v,
        Err(why) => return Verdict::Fail(plain(why)),
    };
    let a = match quoter_amount::side_usd(p, e, usdt_dec, side) {
        Ok(a) => a,
        Err(f) => return Verdict::Fail(f),
    };
    let amount = match counter(p, e, usdt_dec, a.usd) {
        Ok(v) => v,
        Err(f) => return Verdict::Fail(f),
    };
    let (token_in, amount) = match side {
        Side::Buy => (e.usdt, amount),
        // Продажа — монеты на ту же сумму по ТЕКУЩЕЙ цене пула, без обмена на стороне покупки (решение 73): у пула с
        // ликвидностью только с одной стороны цены покупка невозможна, а продажа глубокая — стороны независимы.
        Side::Sell => match super::super::quote::spot::coins_for(e, amount) {
            Some(c) if !c.is_zero() => (e.coin, c),
            _ => {
                return Verdict::Fail(plain(
                    "цены пула нет — монет на сумму сверки продажи не посчитать",
                ));
            }
        },
    };
    let sum = format!("сумма сверки {} USDT — {}", a.usd.round_dp(2), a.source);
    match quoter_compare::compare(p, e, token_in, amount, pct).await {
        Ok(Cmp::Ok { block }) => Verdict::Ok { block },
        Ok(Cmp::Pause { block, why, nums }) => Verdict::Pause {
            why: format!("{why}; {sum}"),
            block,
            usd: a.usd,
            nums,
        },
        Ok(Cmp::Fail(why)) if why.starts_with(emptiness::SHORT) => {
            Verdict::Fail(plain(format!("{why}, {sum}")))
        }
        Ok(Cmp::Fail(why)) => Verdict::Fail(plain(why)),
        Ok(Cmp::NoBlock(why)) => Verdict::NoBlock(why),
        // Не задана настройка (например dex.quoter вида пула) — одна запись на причину, не на пул.
        Err(err) if err.code == ErrorCode::NotConfigured => {
            let msg = format!(
                "{}: сверка с квотером невозможна — {}",
                e.family.title, err.message
            );
            p.signals
                .once(&p.ctx, Level::Error, "dex.quoter", msg)
                .await;
            Verdict::Fail(from_err(err))
        }
        Err(err) => Verdict::Fail(from_err(err)),
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

/// Сумма сверки USDT → встречный токен пула (минимальные единицы): для пула с промежуточной монетой (решение 52) —
/// пересчёт по живому пулу «промежуточная — USDT» (quote/rate.rs). Ошибки: причина и имя незаданной настройки.
fn counter(p: &Pools, e: &Entry, usdt_dec: u8, usd: Decimal) -> Result<U256, Fail> {
    let usdt = p.tokens.usdt(&p.ctx).map_err(from_err)?;
    let amount = num::to_units(usd, usdt_dec).ok_or_else(|| {
        plain(format!(
            "сумма сверки {usd} USDT не переводится в единицы USDT"
        ))
    })?;
    rate::from_usdt(&p.ctx, &p.reg, usdt, e.usdt, amount).map_err(from_err)
}
