//! Файл: core/credit_rest_park.rs
//! Что делает: решение 413 (шестое ревью t198, Б-1, С-3; вынесено из credit_rest.rs — размер файла) — остаток «не
//! присвоено» отдельной позицией сделки без подтверждения (park) и его решение на круге сверки (round). park: сделка
//! остановлена с ПОДТВЕРЖДЁННЫМ количеством, остаток остаётся в записи (CreditFact.rest) — видно на экране сделки и в
//! «Ручном управлении»; в итоге дня — оценкой, как остановленные (решения 53, 152); журнал Warn core.credit_fact и
//! Telegram deal.credit_hold с цифрами. round: остановленные с остатком на бирже круга — решение в тихий момент
//! (credit_rest::locked); остатка на счёте нет — фантом закрывается; есть — сделка остаётся остановленной с решённым
//! количеством (продаст человек кнопкой); человек успел нажать кнопку — его действие не перетирается.
//! Ожидание тихого момента после продажи (at_close): перечитывание с интервалом venue.status_poll_ms, не дольше
//! transfer.max_time_s; сделку взял человек — выход; решено — добор продолжением sell_loop::hop или закрытие.
//! Кто вызывает: core/sell_next.rs (at_close), core/ledger.rs (finish — закрытие с остатком), core/reconcile.rs.
//! От чего зависит: credit_rest.rs (locked), credit_fact.rs, ledger.rs, deal_words.rs, deps.rs, engine.rs.
//! Настройки: venue.status_poll_ms, transfer.max_time_s (существующие).

use rust_decimal::Decimal;

use crate::core::types::VenueId;
use crate::storage::journal::Level;

use super::credit_rest::Res;
use super::deal::{Deal, Step};
use super::deal_words as w;
use super::engine::{Core, lk};
use super::remain_others::Others;

/// Что делает: остаток отдельно, без подтверждения (шапка): сделка остановлена с подтверждённым количеством, остаток
/// «не присвоено» остаётся в записи. Принимает: ядро, сделку, причину. Ошибок нет.
pub async fn park(core: &Core, deal: &mut Deal, why: &str) {
    let t = deal.direction.asset.ticker.clone();
    let rest = super::credit_fact::rest(deal).unwrap_or_default();
    let record = super::credit_fact::here(deal).map_or(Decimal::ZERO, |c| c.record);
    let sold: Decimal = deal.sells.iter().map(|s| s.filled_qty).sum();
    let text = format!(
        "{}: {why}. Запись депозита {}, продано {}, у сделки {}, не присвоено {} (не подтверждено фактом — может быть \
         удержано биржей или у других сделок). Сделка остановлена; остаток решится сам в тихий момент счёта (круг \
         сверки) или кнопкой: «Продать сейчас» — присвоить по факту и продать, «Продано вручную» / «Беру на себя» — \
         остаток к вам (решение 413)",
        w::deal(core, deal),
        w::coins(record, &t),
        w::coins(sold, &t),
        w::coins(deal.qty, &t),
        w::coins(rest, &t)
    );
    // Финальное ревью t198: «уже решено совместно» — проверка памяти прямо перед закрытием, без ожиданий между ними;
    // решённое не перетирается (сделка остановлена с решённым количеством), журнал и Telegram — после закрытия.
    let mem = lk(&core.inner.deals).get(&deal.id).cloned();
    let text = match mem.filter(|x| x.credit.is_some() && super::credit_fact::rest(x).is_none()) {
        Some(x) => {
            (deal.qty, deal.credit) = (x.qty, x.credit.clone());
            (deal.timeline, deal.sale_unsure) = (x.timeline.clone(), x.sale_unsure);
            format!(
                "{}: {why}; остаток уже решён совместно — у сделки {}; сделка остановлена — продайте или разберите \
                 её кнопкой «Ручного управления» (решение 413)",
                w::deal(core, deal),
                w::coins(deal.qty, &t)
            )
        }
        None => text,
    };
    Box::pin(super::ledger::finish(core, deal, Step::Halted, &text)).await;
    let deps = &core.inner.deps;
    Box::pin(deps.log(
        Level::Warn,
        "core.credit_fact",
        Some(&deal.at.0),
        &text,
        None,
    ))
    .await;
    Box::pin(deps.notify("deal.credit_hold", Some(&deal.at.0), &text)).await
}

/// Что делает: круг сверки (шестое ревью С-3, седьмое С-3) — остановленные сделки с остатком на бирже `v`: по каждой
/// монете — сначала дешёвая проверка «тихо» по учёту (без запроса), затем одно чтение и совместное решение всех сделок
/// с остатком этой монеты под одним замком (credit_rest::locked, guard: взял человек — не трогаем); решённая без монет
/// — empty. Принимает: ядро, биржу. Ошибок нет.
pub async fn round(core: &Core, v: &VenueId) {
    let list: Vec<Deal> = lk(&core.inner.deals)
        .values()
        .filter(|d| &d.at == v && d.step == Step::Halted && super::credit_fact::rest(d).is_some())
        .cloned()
        .collect();
    let mut done = std::collections::HashSet::new();
    for mut d in list {
        // Решение 416: монета на счёте биржи (одна на все сети) — coin_account.rs.
        let key = super::coin_account::key(v, &d.direction.asset);
        if done.contains(&key) {
            continue;
        }
        // Восьмое ревью (С-2): занята для этой сделки — пробуем следующую той же монеты; решённой монета считается
        // только после настоящего чтения под замком.
        let o = super::remain_others::on_account(core, &d, v);
        if super::credit_rest::busy(core, &d, &o).is_some() {
            continue;
        }
        let l = Box::pin(super::credit_rest::locked(core, &mut d, true)).await;
        done.insert(key);
        let (Res::Done, Ok(fact)) = (l.res, l.fact) else {
            continue;
        };
        // Решённые этим чтением (сама и совместные), остановленные и без монет — фантом или человеку.
        let ids: Vec<String> = std::iter::once(d.id.clone())
            .chain(l.joint.iter().cloned())
            .collect();
        for id in ids {
            let x = lk(&core.inner.deals).get(&id).cloned();
            if let Some(mut x) = x.filter(|x| x.step == Step::Halted && x.qty <= Decimal::ZERO) {
                let why = "остатка зачисления на счёте нет — фантом учёта (решение 222, 413)";
                Box::pin(super::remain_rest::empty(
                    core, &mut x, fact, &l.others, why,
                ))
                .await;
            }
        }
        if d.qty > Decimal::ZERO {
            let fresh = lk(&core.inner.deals).get(&d.id).cloned();
            if let Some(x) = fresh {
                Box::pin(super::ledger::save(core, &x)).await;
            }
        }
    }
}

/// Итог ожидания тихого момента.
enum Waited {
    Done(Decimal, Others),
    Unknown,
    Timeout,
    Gone,
}

/// Ожидание тихого момента и решение (шапка; седьмое ревью С-2: «взял человек» — под замком, до решения). Ошибок нет.
async fn wait_quiet(core: &Core, deal: &mut Deal) -> Waited {
    let Some((ms, max)) = super::credit_alert::pace(core, deal) else {
        return Waited::Timeout;
    };
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(max);
    loop {
        let l = Box::pin(super::credit_rest::locked(core, deal, true)).await;
        match (l.res, l.fact) {
            (Res::Done, Ok(f)) => return Waited::Done(f, l.others),
            // Решён совместно другой сделкой, а этот баланс не прочитан — без закрытия: решит человек.
            (Res::Done, Err(_)) => return Waited::Unknown,
            (Res::Taken, _) => return Waited::Gone,
            _ => {}
        }
        if tokio::time::Instant::now() >= until {
            return Waited::Timeout;
        }
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
}

/// Что делает: продажа сделки с остатком кончилась — ждём тихий момент (шапка). Принимает: ядро, сделку, причину
/// продажи. Ошибок нет.
pub async fn at_close(core: &Core, mut deal: Deal, why: String) {
    match Box::pin(wait_quiet(core, &mut deal)).await {
        Waited::Gone => {}
        Waited::Timeout => {
            // Восьмое ревью: остаток мог быть решён совместно между последним чтением и пределом — решённое не
            // перезаписываем.
            let mem = lk(&core.inner.deals).get(&deal.id).cloned();
            if let Some(x) =
                mem.filter(|x| x.credit.is_some() && super::credit_fact::rest(x).is_none())
            {
                deal = x;
                if deal.qty > Decimal::ZERO {
                    let left = super::stuck_reason::StuckReason::Partial(deal.qty);
                    return super::sell_loop::hop(super::sell_next::stuck(core, deal, left, true))
                        .await;
                }
                let t = "остаток решён совместно, баланс сейчас не прочитан — проверьте сделку (решение 413)";
                return Box::pin(super::ledger::finish(core, &mut deal, Step::Halted, t)).await;
            }
            let t =
                "тихий момент счёта не наступил (не задано или вышел предел transfer.max_time_s)";
            Box::pin(park(core, &mut deal, t)).await
        }
        Waited::Done(..) | Waited::Unknown if deal.qty > Decimal::ZERO => {
            // Добранное продаёт автоматика, как остаток частичной продажи (решение 177) — продолжением цикла.
            Box::pin(super::ledger::save(core, &deal)).await;
            let left = super::stuck_reason::StuckReason::Partial(deal.qty);
            super::sell_loop::hop(super::sell_next::stuck(core, deal, left, true)).await
        }
        Waited::Unknown => {
            let t = "остаток решён совместно, баланс сейчас не прочитан — проверьте сделку (решение 413)";
            Box::pin(super::ledger::finish(core, &mut deal, Step::Halted, t)).await
        }
        Waited::Done(fact, others) => {
            let t = format!("продано ({why}); остатка зачисления на счёте нет");
            Box::pin(super::remain_rest::empty(
                core, &mut deal, fact, &others, &t,
            ))
            .await
        }
    }
}
