//! Файл: venues/bsc_dex/check/quoter_cycle.rs
//! Что делает: цикл сверки локальной математики пулов с квотером своей DEX (каждые dex.quoter_check_s и по кнопке
//! «Перепроверить»): расхождение больше dex.quoter_max_diff_pct — пул исключён из котировок до следующей сверки.
//! «Перепроверить» — только пулы переданных монет, «новые пулы» — пулы без итога (check/recheck.rs).
//! Порядок: достоверность → ликвидность не ниже dex.min_pool_liquidity_usd (мелкие пулы не сверяются и не пишутся в
//! журнал вовсе) → пустой пул / не хватает ликвидности на сумму (quote/emptiness.rs, quote/quoter_revert.rs — честная
//! причина, не «сбой квотера») → крючок → квотер. Пустой пул, получивший ликвидность (событие), сверяется сразу
//! (pools/progress.rs, refill).
//! Сверяются все пулы, и с крючком тоже: у пула с крючком B/C локальный выход берётся за вычетом измеренного
//! влияния крючка на покупку (check/hooks_probe.rs; сверка идёт на check.probe_usdt USDT — это покупка).
//! Журнал: первый проход — одна сводка (прошли, исключены, частые причины); дальше — запись только при смене
//! состояния пула (прошла ↔ не прошла, сменилась причина; цифры в причине не в счёт — progress::reason_key).
//! Кто вызывает: pools/market.rs.
//! От чего зависит: quote/quoter.rs, quote/select.rs (deep_enough), quote/emptiness.rs, quote/rate.rs,
//! check/state.rs, check/quoter_tally.rs (сводка), pools/listen.rs (Pools), pools/progress.rs, tokens.rs, num.rs.
//! Настройки: dex.quoter_check_s, dex.quoter_max_diff_pct, dex.min_pool_liquidity_usd, check.probe_usdt [bsc_dex].
//!
//! «Сверка: периодически и перед сделкой — квотер; расхождение выше порога → стоп по пулу» —
//! research/flow/bsc_dex.md, схема адаптера п. 6.

use std::time::Duration;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use crate::core::types::{CheckState, HookGroup, PoolRef};
use crate::gateway::errors::ErrorCode;
use crate::storage::journal::Level;

use super::super::num;
use super::super::pools::index_watch;
use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;
use super::super::pools::registry::Entry;
use super::super::quote::{emptiness, quoter, rate, select};
use super::quoter_tally::Tally;
use super::recheck::Queue;
use super::state::Checks;

/// Начало причины «не задана настройка» — такие причины пишутся одной строкой на причину (Signals::once).
const NOT_SET: &str = "Не задана настройка";

/// Цикл сверки с квотером (см. шапку). Ошибок нет: сбой — причина в итоге пула.
pub async fn run(p: Pools, queue: Queue) {
    let mut first = true;
    loop {
        // Своя комиссия пар V2 (dex.v2_fee_source = pool) — заново с пары перед сверкой (pools/index_watch.rs).
        index_watch::refresh_fees(&p).await;
        let mut tally = Tally::default();
        for r in p.reg.refs() {
            one(&p, &r, first, &mut tally).await;
        }
        if first {
            tally.summary(&p).await;
            first = false;
        }
        let d = match p.ctx.int("dex.quoter_check_s") {
            Ok(s) => Duration::from_secs(s),
            Err(e) => {
                let msg = format!("Сверка с квотером остановлена: {}", e.message);
                p.ctx.log(Level::Error, "dex.quoter", msg, None).await;
                return;
            }
        };
        let until = tokio::time::Instant::now() + d;
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(until) => break,
                _ = queue.notified() => {
                    // «Перепроверить» — только пулы переданных монет; «новые пулы» — пулы без итога (check/recheck.rs).
                    let (tickers, new_pools) = queue.take();
                    for r in p.reg.refs() {
                        let Some(e) = p.reg.get(&r) else { continue };
                        if tickers.contains(&e.ticker) || (new_pools && p.checks.pool(&r).quoter.is_none()) {
                            one(&p, &r, false, &mut Tally::default()).await;
                        }
                    }
                }
                _ = p.signals.refill.notified() => {
                    for r in p.signals.take_refilled() {
                        one(&p, &r, false, &mut Tally::default()).await;
                    }
                }
            }
        }
    }
}

/// Сверка одного пула и запись итога. Мелкий пул — итога нет (None), без журнала. Ошибок нет.
async fn one(p: &Pools, r: &PoolRef, first: bool, tally: &mut Tally) {
    let Some(e) = p.reg.get(r) else { return };
    if !e.valid {
        return;
    }
    let deep = match usdt_decimals(p).await {
        Ok(dec) => select::deep_enough(&p.ctx, &p.reg, &p.tokens, &e, dec),
        Err(why) => Err(why),
    };
    let res = match deep {
        Ok(None) => verify(p, &e).await,
        Ok(Some(_)) => {
            tally.dust += 1;
            p.checks.update_pool(r, |f| f.quoter = None);
            return;
        }
        Err(why) => Err(why),
    };
    tally.add(&res);
    let prev = p.checks.pool(r).quoter;
    if !first && changed(prev.as_ref(), &res) {
        let msg = match &res {
            Ok(()) => format!(
                "{} {}: сверка с квотером снова проходит — пул в котировках",
                e.family.title, e.ticker
            ),
            Err(why) => format!("{} {}: {why} — пул исключён", e.family.title, e.ticker),
        };
        let level = if res.is_ok() {
            Level::Info
        } else {
            Level::Warn
        };
        p.ctx.log(level, "dex.quoter", msg, None).await;
    }
    p.checks.update_pool(r, |f| f.quoter = Some(res));
}

/// Сменилось ли состояние пула так, что это стоит записать (см. шапку). Ошибок нет.
fn changed(prev: Option<&Result<(), String>>, new: &Result<(), String>) -> bool {
    match (prev, new) {
        (_, Err(why)) if why.starts_with(NOT_SET) => false,
        (None, Ok(())) | (Some(Ok(())), Ok(())) => false,
        (Some(Err(a)), Err(b)) => reason_key(a) != reason_key(b),
        _ => true,
    }
}

/// Сверка пула (после фильтра ликвидности). Возвращает: Ok или Err(причина). Ошибок нет.
async fn verify(p: &Pools, e: &Entry) -> Result<(), String> {
    if emptiness::empty(e) {
        return Err(emptiness::EMPTY.into());
    }
    let pct = hook_buy_pct(&p.checks, e)?;
    let (amount, usdt) = probe(p, e).await?;
    match quoter::compare(&p.ctx, e, e.usdt, amount, pct).await {
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
async fn usdt_decimals(p: &Pools) -> Result<u8, String> {
    let usdt = p.tokens.usdt(&p.ctx).map_err(|e| e.message)?;
    p.tokens.decimals(&p.ctx, usdt).await.map_err(|e| e.message)
}

/// Влияние крючка на покупку, %: 0 — крючка B/C нет. Err — влияние ещё не измерено или проверка крючка не прошла.
fn hook_buy_pct(checks: &Checks, e: &Entry) -> Result<Decimal, String> {
    if e.pool
        .hooks
        .as_ref()
        .is_none_or(|h| h.group == HookGroup::A)
    {
        return Ok(Decimal::ZERO);
    }
    match checks.pool(&e.pool.reference).hook.map(|c| c.impact) {
        Some(CheckState::Known(i)) => Ok(i.buy_pct),
        Some(CheckState::Failed { reason }) => Err(format!("проверка крючка не прошла: {reason}")),
        _ => Err("влияние крючка ещё не измерено — сверка с квотером невозможна".into()),
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
