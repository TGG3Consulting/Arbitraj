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
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::credit_rest::Res;
use super::deal::{Deal, Step};
use super::deal_words as w;
use super::engine::{Core, lk};

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
    let deps = &core.inner.deps;
    Box::pin(deps.log(
        Level::Warn,
        "core.credit_fact",
        Some(&deal.at.0),
        &text,
        None,
    ))
    .await;
    Box::pin(deps.notify("deal.credit_hold", Some(&deal.at.0), &text)).await;
    Box::pin(super::ledger::finish(core, deal, Step::Halted, &text)).await
}

/// Что делает: круг сверки (шестое ревью, С-3) — остановленные сделки с остатком на бирже `v`: решить в тихий момент
/// (остатка на счёте нет — закрыть фантом; есть — сделка остаётся остановленной с решённым количеством). Принимает:
/// ядро, биржу. Ошибок нет.
pub async fn round(core: &Core, v: &VenueId) {
    let list: Vec<Deal> = lk(&core.inner.deals)
        .values()
        .filter(|d| &d.at == v && d.step == Step::Halted && super::credit_fact::rest(d).is_some())
        .cloned()
        .collect();
    for mut d in list {
        let seen = (d.step, d.updated_at);
        if !matches!(
            Box::pin(super::credit_rest::locked(core, &mut d)).await,
            Res::Done
        ) {
            continue;
        }
        // Человек успел нажать кнопку — его действие не перетираем (учёт памяти уже обновлён sync — только количество).
        if lk(&core.inner.deals)
            .get(&d.id)
            .map(|x| (x.step, x.updated_at))
            != Some(seen)
        {
            continue;
        }
        if d.qty <= Decimal::ZERO {
            let why = "остатка зачисления на счёте нет — фантом учёта (решение 222, 413)";
            Box::pin(super::ledger::finish(core, &mut d, Step::Closed, why)).await;
        } else {
            Box::pin(super::ledger::save(core, &d)).await;
        }
    }
}

/// (интервал мс, предел с) ожидания тихого момента; None — не задано. Ошибок нет.
fn pace(core: &Core, deal: &Deal) -> Option<(u64, u64)> {
    let a = &deal.direction.asset;
    let coin = crate::settings::scope::coin_id(&a.ticker, a.bsc_contract.as_ref());
    let at = At {
        venue: Some(&deal.at.0),
        coin: Some(&coin),
    };
    let max = core
        .inner
        .deps
        .settings
        .integer("transfer.max_time_s", at)
        .ok()?;
    Some((super::credit_alert::wait_ms(core, &deal.at)?, max))
}

/// Итог ожидания тихого момента.
enum Waited {
    Done,
    Timeout,
    Gone,
}

/// Ожидание тихого момента и решение (шапка). Ошибок нет.
async fn wait_quiet(core: &Core, deal: &mut Deal) -> Waited {
    let Some((ms, max)) = pace(core, deal) else {
        return Waited::Timeout;
    };
    let mark = lk(&core.inner.deals)
        .get(&deal.id)
        .map(|d| (d.step, d.updated_at));
    let until = tokio::time::Instant::now() + std::time::Duration::from_secs(max);
    loop {
        if let Res::Done = Box::pin(super::credit_rest::locked(core, deal)).await {
            return Waited::Done;
        }
        if tokio::time::Instant::now() >= until {
            return Waited::Timeout;
        }
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        // Шестое ревью (М-3): сделку взял человек (её запись в учёте изменилась) — ожидание кончается.
        if lk(&core.inner.deals)
            .get(&deal.id)
            .map(|d| (d.step, d.updated_at))
            != mark
        {
            return Waited::Gone;
        }
    }
}

/// Что делает: продажа сделки с остатком кончилась — ждём тихий момент (шапка). Принимает: ядро, сделку, причину
/// продажи. Ошибок нет.
pub async fn at_close(core: &Core, mut deal: Deal, why: String) {
    match Box::pin(wait_quiet(core, &mut deal)).await {
        Waited::Gone => {}
        Waited::Timeout => {
            let t =
                "тихий момент счёта не наступил (не задано или вышел предел transfer.max_time_s)";
            Box::pin(park(core, &mut deal, t)).await
        }
        Waited::Done if deal.qty > Decimal::ZERO => {
            // Добранное продаёт автоматика, как остаток частичной продажи (решение 177) — продолжением цикла.
            Box::pin(super::ledger::save(core, &deal)).await;
            let left = super::stuck_reason::StuckReason::Partial(deal.qty);
            super::sell_loop::hop(super::sell_next::stuck(core, deal, left, true)).await
        }
        Waited::Done => {
            let t = format!("продано ({why}); остатка зачисления на счёте нет");
            Box::pin(super::ledger::finish(core, &mut deal, Step::Closed, &t)).await
        }
    }
}
