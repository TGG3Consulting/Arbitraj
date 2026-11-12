//! Файл: venues/bsc_dex/check/quoter_cycle.rs
//! Что делает: цикл сверки локальной математики пулов с квотером своей DEX (каждые dex.quoter_check_s и по кнопке
//! «Перепроверить»). Решение 261: сверка — на ОДНОМ блоке (check/quoter_block.rs, check/quoter_compare.rs), сумма
//! стороны — min(наш максимум money.max_per_trade_usd, ликвидность стороны) (check/quoter_amount.rs); расхождение
//! больше dex.quoter_max_diff_pct — сторона на ПАУЗЕ (исключена из котировок и сделок) до сверки, прошедшей на одном
//! блоке; уход на паузу и снятие — алерт Telegram по одному на событие (check/quoter_pause.rs); блок не определён —
//! «не сверено (блок)», итог стороны прежний (не исключается и не паузится).
//! «Перепроверить» — только пулы переданных монет, «новые пулы» — пулы без итога (check/recheck.rs).
//! Порядок: достоверность → ликвидность не ниже dex.min_pool_liquidity_usd (мелкие пулы не сверяются и не пишутся в
//! журнал вовсе) → пустой пул / не хватает ликвидности на сумму (quote/emptiness.rs, quote/quoter_revert.rs — честная
//! причина, не «сбой квотера») → крючок → квотер. Пустой пул, получивший ликвидность (событие), сверяется сразу
//! (pools/progress.rs, refill).
//! Стороны пула — независимо (решение 73): покупка и продажа сверяются каждая своей пробой, когда сторона глубокая
//! (quote/select.rs deep_side); мелкая сторона — без итога, итог другой стороны от этого не меняется; котировка и
//! сделка берут сторону, только если прошла её сверка (check/quoter_sides.rs).
//! Сверяются все пулы, и с крючком тоже: у пула с крючком B/C локальный выход берётся за вычетом измеренного
//! влияния крючка на сторону (check/hooks_probe.rs); у такого пула сумма сверки — прежняя check.probe_usdt USDT (влияние
//! крючка измерено на ней, решение 261), у продажи — монеты на эту сумму по цене пула.
//! Пул сверяется сразу, как только стал достоверным (прочитан при старте, перечитан, принят новым) и циклом ещё не
//! сверен (check/fresh.rs, сигнал «пул готов»), — не ждёт dex.quoter_check_s (решение 65).
//! Журнал: первый проход — одна сводка (прошли, исключены, частые причины), когда ни один пул не читается и все
//! прочитанные сверены (застрявшие из-за незаданной настройки не держат, решение 63); дальше — запись только при смене
//! состояния пула (прошла ↔ не прошла, сменилась причина; цифры в причине не в счёт — progress::reason_key).
//! Кто вызывает: pools/market.rs.
//! От чего зависит: check/quoter_verify.rs (сверка пула), check/quoter_pause.rs (пауза, алерты), quote/select.rs
//! (deep_side), check/fresh.rs,
//! check/state.rs, check/quoter_tally.rs (сводка), pools/listen.rs (Pools), pools/progress.rs.
//! Настройки: dex.quoter_check_s, dex.quoter_max_diff_pct, dex.min_pool_liquidity_usd, check.probe_usdt [bsc_dex],
//! money.max_per_trade_usd (через check/quoter_amount.rs).
//!
//! «Сверка: периодически и перед сделкой — квотер; расхождение выше порога → стоп по пулу» —
//! research/flow/bsc_dex.md, схема адаптера п. 6.

use std::time::Duration;

use crate::core::types::{PoolRef, Side};
use crate::storage::journal::Level;

use super::super::pools::index_watch;
use super::super::pools::listen::Pools;
use super::super::quote::select;
use super::super::settings_wait;
use super::fresh::Fresh;
use super::quoter_pause;
use super::quoter_tally::Tally;
use super::quoter_verify::{HOOK_WAIT, Verdict, usdt_decimals, verify};
use super::recheck::Queue;

/// Цикл сверки с квотером (см. шапку). Ошибок нет: сбой — причина в итоге пула.
pub async fn run(p: Pools, queue: Queue) {
    let mut fresh = Fresh::default();
    // Итоги первого прохода — до сводки (когда ни один пул не читается и все прочитанные сверены, решение 65).
    let mut first = Some(Tally::default());
    let mut full = false;
    // Изменение настроек — повтор сверки сторон с причиной «не задана настройка» (settings_wait.rs).
    let mut changes = settings_wait::watch(&p.ctx);
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
        // Не задана — сверка ждёт настройку и продолжает сама (settings_wait.rs), а не останавливается навсегда.
        let read = || p.ctx.int("dex.quoter_check_s").map(Duration::from_secs);
        let Some(d) =
            settings_wait::until_ok(&p.ctx, "Сверка с квотером", "dex.quoter", read).await
        else {
            return;
        };
        let until = tokio::time::Instant::now() + d;
        loop {
            // Подписка на «пул готов» — до прохода по новым пулам: сигнал во время прохода не теряется.
            let ready = p.signals.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            // Повтор по сигналу: ждали крючка — он измерен; сторона без итога стала глубокой (решение 73: иначе она ждала
            // бы полного прохода dex.quoter_check_s).
            for r in fresh.due(&p, |r| {
                hook_measured(&p, r) || super::quoter_side::side_ready(&p, r)
            }) {
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
                    let (coins, new_pools) = queue.take();
                    for r in p.reg.refs() {
                        let Some(e) = p.reg.get(&r) else { continue };
                        if coins.contains(&e.coin) || (new_pools && p.checks.pool(&r).quoter.none()) {
                            fresh.check(&r, one(&p, &r, &mut first)).await;
                        }
                    }
                }
                _ = p.signals.refill.notified() => {
                    for r in p.signals.take_refilled() {
                        one(&p, &r, &mut first).await;
                    }
                }
                // Настройку задали на лету — стороны, ждавшие её, сверяются сразу, а не через dex.quoter_check_s.
                Some(c) = changes.next() => {
                    // Пачка изменений — один проход; только стороны, ждущие именно изменённой настройки.
                    let changed = settings_wait::drain(c, &mut changes);
                    for r in p.reg.refs() {
                        if p.checks.pool(&r).quoter.waits_for(&changed) {
                            fresh.check(&r, one(&p, &r, &mut first)).await;
                        }
                    }
                }
            }
        }
    }
}

/// Сверка ждала влияния крючка, а оно уже измерено (успешно или нет). Ошибок нет.
fn hook_measured(p: &Pools, r: &PoolRef) -> bool {
    let c = p.checks.pool(r);
    c.quoter.any_err(|why| why == HOOK_WAIT) && c.hook.is_some()
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
    let dec = usdt_decimals(p).await;
    // Стороны — независимо (решение 73): сторона сверяется, когда она глубокая; мелкая — без итога, другая не меняется.
    let mut now = p.checks.pool(r).quoter;
    for side in [Side::Buy, Side::Sell] {
        let deep = match &dec {
            Ok(dec) => select::deep_side(
                &p.ctx,
                &p.reg,
                &p.tokens,
                &e,
                *dec,
                select::Need::Side(side),
            )
            .map(|shallow| (shallow, *dec)),
            Err(why) => Err(why.clone()),
        };
        let prev = now.side(side).clone();
        // Итог и имя незаданной настройки — из ошибки там, где она возникла (решение 75): по нему сторона
        // перепроверяется при задании именно этой настройки. Глубина — её настройки (pools/depth_book.rs).
        let v = match deep {
            Ok((None, dec)) => verify(p, &e, dec, side).await,
            Ok((Some(_), _)) => {
                now.set(side, None);
                now.set_wait(side, None);
                now.set_note(side, None);
                continue;
            }
            Err(why) => Verdict::Fail((
                why,
                super::super::pools::depth_book::missing_setting(&p.ctx),
            )),
        };
        // Решение 261: пауза по расхождению на одном блоке, снятие — прошедшей сверкой; «не сверено (блок)» — итог прежний.
        let change = quoter_pause::apply(&mut now, side, &v);
        if let Verdict::NoBlock(why) = &v {
            let msg = format!(
                "{} {} ({}): {why}",
                e.family.title,
                e.ticker,
                quoter_pause::side_name(side)
            );
            p.signals.once(&p.ctx, Level::Info, "dex.quoter", msg).await;
        }
        match change {
            Some(c) => quoter_pause::alert(p, &e, side, &v, c).await,
            None => {
                if first.is_none()
                    && let Some(res) = now.side(side)
                    && super::quoter_side::changed(prev.as_ref(), res)
                {
                    super::quoter_side::log_change(p, &e, side, res).await;
                }
            }
        }
    }
    if let Some(t) = first {
        t.set(r, now.combined());
    }
    p.checks.update_pool(r, |f| f.quoter = now);
    true
}
