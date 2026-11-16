//! Файл: venues/bsc_dex/check/quoter_verify.rs
//! Что делает: сверка одной стороны пула с квотером своей DEX (после фильтра ликвидности): пустой пул / не хватает
//! ликвидности на сумму — честная причина; у пула с крючком B/C — за вычетом измеренного влияния крючка на сторону;
//! покупка и продажа — независимо (решение 73).
//! Решение 261 (Тигран 12.11): сверка — на ОДНОМ блоке: пул прочитан заново на голове H (check/quoter_block.rs at_head —
//! один раз на пул, после проверки настроек, лениво), математика по свежему состоянию и квотер на H
//! (check/quoter_compare.rs); сумма стороны —
//! min(наш максимум money.max_per_trade_usd среди направлений с DEX, ликвидность стороны в полосе) USDT
//! (check/quoter_amount.rs; пул с крючком B/C — прежняя check.probe_usdt), для пула с промежуточной монетой — пересчёт
//! во встречный токен по живому пулу (решение 52, quote/rate.rs). Итог: прошла / пауза (расхождение на одном блоке) /
//! иная причина / «не сверено (блок)».
//! Решение 335: влияние крючка — check/hook_store.rs for_verify; пауза у пула с крючком B/C в безопасную сторону —
//! перемер на том же H и повтор (check/quoter_reprobe.rs). Настройки не заданы — как раньше.
//! Кто вызывает: check/quoter_one.rs.
//! От чего зависит: check/quoter_block.rs, check/quoter_compare.rs, check/quoter_amount.rs, check/hook_store.rs,
//! check/quoter_reprobe.rs, quote/emptiness.rs, quote/rate.rs, quote/spot.rs, pools/registry.rs, tokens.rs, num.rs.
//! Настройки: dex.quoter_max_diff_pct, money.max_per_trade_usd, check.probe_usdt,
//! hooks.reprobe_before_pause [bsc_dex] (через вызываемые).

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{HookImpact, Side};
use crate::gateway::errors::ErrorCode;
use crate::storage::journal::Level;

use super::super::num;
use super::super::pools::listen::Pools;
use super::super::pools::registry::Entry;
use super::super::quote::{emptiness, rate};
use super::quoter_amount;
use super::quoter_block::{self, Head, NOT_CHECKED};
use super::quoter_compare::{self, Cmp, Nums};
use super::quoter_reprobe::{self, Try};
use super::{hook_store, reprobe_rule};

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
/// во встречном токене в пул; продажа — монеты на эту сумму по цене пула на H (quote/spot.rs, без обмена). Настройки
/// (влияние крючка, сумма, допуск) — до обращения к узлу (ревью #65); голова и свежее чтение — в `head` (одно на пул:
/// None — ещё не читали); перемер влияния на блоке (решение 335) — в `cache` (один на пул). Принимает: пулы, запись
/// из памяти, голову пула, перемер, знаки USDT, сторону. Возвращает: итог. Ошибок нет.
pub async fn verify(
    p: &Pools,
    e: &Entry,
    (head, cache): (&mut Option<Head>, &mut Option<(u64, HookImpact)>),
    usdt_dec: u8,
    side: Side,
) -> Verdict {
    if emptiness::empty(e) {
        return Verdict::Fail(plain(emptiness::EMPTY));
    }
    let old = match hook_store::for_verify(&p.checks, e) {
        Ok(v) => v,
        Err(why) => return Verdict::Fail(plain(why)),
    };
    // Решение «335 (лесенка влияния)»: пул с крючком B/C сверяется на верхней точке лесенки стороны — её входе и
    // влиянии на нём (как сумма сверки пулов без крючка: min(наш максимум, ликвидность стороны), решение 261).
    let top = old
        .as_ref()
        .and_then(|i| reprobe_rule::side_steps(i, side).last().cloned());
    let pct = match &top {
        Some(t) => t.pct,
        None => old
            .as_ref()
            .map_or(Decimal::ZERO, |i| reprobe_rule::side_pct(i, side)),
    };
    // Сумма — по глубине стороны в памяти (её же фильтр глубины пропустил сторону; кэш глубины свежим чтением не
    // трогается).
    let a = match &top {
        Some(t) => quoter_amount::Amount {
            usd: t.usd,
            source: format!("верхняя точка лесенки влияния крючка ({} USDT)", t.usd),
        },
        None => match quoter_amount::side_usd(p, e, usdt_dec, side) {
            Ok(a) => a,
            Err(f) => return Verdict::Fail(f),
        },
    };
    let amount = match &top {
        Some(t) => t.amount_in,
        None => match counter(p, e, usdt_dec, a.usd) {
            Ok(v) => v,
            Err(f) => return Verdict::Fail(f),
        },
    };
    if let Err(err) = p.ctx.dec("dex.quoter_max_diff_pct") {
        return Verdict::Fail(from_err(err));
    }
    if head.is_none() {
        *head = Some(quoter_block::at_head(p, e).await);
    }
    let (block, fresh): (u64, &Entry) = match head.as_ref() {
        Some(Head::Ready { block, entry }) => (*block, &**entry),
        Some(Head::NoBlock(why)) => return Verdict::NoBlock(why.clone()),
        Some(Head::Fail(f)) => return Verdict::Fail(f.clone()),
        None => return Verdict::NoBlock(format!("{NOT_CHECKED}: голова не прочитана")),
    };
    if emptiness::empty(fresh) {
        return Verdict::Fail(plain(emptiness::EMPTY));
    }
    let (token_in, amount) = match side {
        Side::Buy => (fresh.usdt, amount),
        // Верхняя точка лесенки продажи — уже монеты (вход пула продажи).
        Side::Sell if top.is_some() => (fresh.coin, amount),
        // Продажа — монеты на ту же сумму по цене пула на H, без обмена на стороне покупки (решение 73): у пула с
        // ликвидностью только с одной стороны цены покупка невозможна, а продажа глубокая — стороны независимы.
        Side::Sell => match super::super::quote::spot::coins_for(fresh, amount) {
            Some(c) if !c.is_zero() => (fresh.coin, c),
            _ => {
                return Verdict::Fail(plain(
                    "цены пула нет — монет на сумму сверки продажи не посчитать",
                ));
            }
        },
    };
    let sum = format!("сумма сверки {} USDT — {}", a.usd.round_dp(2), a.source);
    let got = quoter_compare::compare(&p.ctx, fresh, token_in, amount, pct, block).await;
    // Решение 335 (а): пауза у пула с крючком B/C — перемер влияния на том же H и повтор (hooks.reprobe_before_pause).
    let t = Try {
        fresh,
        block,
        side,
        token_in,
        amount,
        old,
        cache,
    };
    let got = quoter_reprobe::again(p, e, t, got).await;
    verdict(p, e, got, sum, a.usd).await
}

/// Итог сравнения → итог стороны. Принимает: пулы, запись, итог compare, строку суммы, сумму USDT. Ошибок нет.
async fn verdict(
    p: &Pools,
    e: &Entry,
    got: crate::gateway::errors::GatewayResult<Cmp>,
    sum: String,
    usd: Decimal,
) -> Verdict {
    match got {
        Ok(Cmp::Ok { block }) => Verdict::Ok { block },
        Ok(Cmp::Pause { block, why, nums }) => Verdict::Pause {
            why: format!("{why}; {sum}"),
            block,
            usd,
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
