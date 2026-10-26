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
//! Пул сверяется сразу, как только стал достоверным (прочитан при старте, перечитан, принят новым) и циклом ещё не
//! сверен (check/fresh.rs, сигнал «пул готов»), — не ждёт dex.quoter_check_s (решение 65).
//! Журнал: первый проход — одна сводка (прошли, исключены, частые причины), когда ни один пул не читается и все
//! прочитанные сверены (застрявшие из-за незаданной настройки не держат, решение 63); дальше — запись только при смене
//! состояния пула (прошла ↔ не прошла, сменилась причина; цифры в причине не в счёт — progress::reason_key).
//! Кто вызывает: pools/market.rs.
//! От чего зависит: check/quoter_verify.rs (сверка пула), quote/select.rs (deep_enough), check/fresh.rs,
//! check/state.rs, check/quoter_tally.rs (сводка), pools/listen.rs (Pools), pools/progress.rs.
//! Настройки: dex.quoter_check_s, dex.quoter_max_diff_pct, dex.min_pool_liquidity_usd, check.probe_usdt [bsc_dex].
//!
//! «Сверка: периодически и перед сделкой — квотер; расхождение выше порога → стоп по пулу» —
//! research/flow/bsc_dex.md, схема адаптера п. 6.

use std::time::Duration;

use crate::core::types::PoolRef;
use crate::storage::journal::Level;

use super::super::pools::index_watch;
use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;
use super::super::quote::select;
use super::fresh::Fresh;
use super::quoter_tally::Tally;
use super::quoter_verify::{HOOK_WAIT, usdt_decimals, verify};
use super::recheck::Queue;

/// Начало причины «не задана настройка» — такие причины пишутся одной строкой на причину (Signals::once).
const NOT_SET: &str = "Не задана настройка";

/// Цикл сверки с квотером (см. шапку). Ошибок нет: сбой — причина в итоге пула.
pub async fn run(p: Pools, queue: Queue) {
    let mut fresh = Fresh::default();
    // Итоги первого прохода — до сводки (когда ни один пул не читается и все прочитанные сверены, решение 65).
    let mut first = Some(Tally::default());
    let mut full = false;
    loop {
        // Своя комиссия пар V2 (dex.v2_fee_source = pool) — заново с пары перед сверкой (pools/index_watch.rs).
        index_watch::refresh_fees(&p).await;
        // Полный проход — раз в dex.quoter_check_s; при старте все пулы и так новые для цикла.
        if full {
            for r in p.reg.refs() {
                fresh.check(&r, one(&p, &r, &mut first)).await;
            }
        }
        full = true;
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
            // Подписка на «пул готов» — до прохода по новым пулам: сигнал во время прохода не теряется.
            let ready = p.signals.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            for r in fresh.due(&p, |r| hook_measured(&p, r)) {
                fresh.check(&r, one(&p, &r, &mut first)).await;
            }
            if first.is_some() && fresh.settled(&p) {
                first.take().unwrap_or_default().summary(&p).await;
            }
            tokio::select! {
                _ = tokio::time::sleep_until(until) => break,
                _ = &mut ready => {}
                _ = queue.notified() => {
                    // «Перепроверить» — только пулы переданных монет; «новые пулы» — пулы без итога (check/recheck.rs).
                    let (tickers, new_pools) = queue.take();
                    for r in p.reg.refs() {
                        let Some(e) = p.reg.get(&r) else { continue };
                        if tickers.contains(&e.ticker) || (new_pools && p.checks.pool(&r).quoter.is_none()) {
                            fresh.check(&r, one(&p, &r, &mut first)).await;
                        }
                    }
                }
                _ = p.signals.refill.notified() => {
                    for r in p.signals.take_refilled() {
                        one(&p, &r, &mut first).await;
                    }
                }
            }
        }
    }
}

/// Сверка ждала влияния крючка, а оно уже измерено (успешно или нет). Ошибок нет.
fn hook_measured(p: &Pools, r: &PoolRef) -> bool {
    let c = p.checks.pool(r);
    matches!(&c.quoter, Some(Err(why)) if why == HOOK_WAIT) && c.hook.is_some()
}

/// Сверка одного пула и запись итога. Мелкий пул — итога нет (None), без журнала. До сводки первого прохода итог
/// идёт в неё (`first`), журнал по пулу — только после. Возвращает: была ли проверка (пула нет или он недостоверен —
/// false: пул не отмечается проверенным и сверится, как только станет достоверным, check/fresh.rs). Ошибок нет.
pub async fn one(p: &Pools, r: &PoolRef, first: &mut Option<Tally>) -> bool {
    // Данные пулов отстают (лента догоняет, решение 72) — вердикта нет: пул не отмечен проверенным, сверится после
    // снятия отметки (она будит циклы сигналом «пул готов»).
    if p.lag.busy() {
        return false;
    }
    let Some(e) = p.reg.get(r) else {
        return false;
    };
    if !e.valid {
        return false;
    }
    let deep = match usdt_decimals(p).await {
        Ok(dec) => select::deep_enough(&p.ctx, &p.reg, &p.tokens, &e, dec),
        Err(why) => Err(why),
    };
    let res = match deep {
        Ok(None) => verify(p, &e).await,
        Ok(Some(_)) => {
            if let Some(t) = first {
                t.set(r, None);
            }
            p.checks.update_pool(r, |f| f.quoter = None);
            return true;
        }
        Err(why) => Err(why),
    };
    let prev = p.checks.pool(r).quoter;
    match first {
        Some(t) => t.set(r, Some(res.clone())),
        None if changed(prev.as_ref(), &res) => {
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
        None => {}
    }
    p.checks.update_pool(r, |f| f.quoter = Some(res));
    true
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
