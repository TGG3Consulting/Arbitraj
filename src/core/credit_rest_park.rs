//! Файл: core/credit_rest_park.rs
//! Что делает: решение 413 (шестое ревью t198, Б-1, С-3; вынесено из credit_rest.rs — размер файла) — остаток «не
//! присвоено» отдельной позицией сделки без подтверждения (park) и его решение на круге сверки (round). park: сделка
//! остановлена с ПОДТВЕРЖДЁННЫМ количеством, остаток остаётся в записи (CreditFact.rest) — видно на экране сделки и в
//! «Ручном управлении»; в итоге дня — оценкой, как остановленные (решения 53, 152); журнал Warn core.credit_fact и
//! Telegram deal.credit_hold с цифрами. round: остановленные с остатком на бирже круга — решение в тихий момент
//! (credit_rest::locked); остатка на счёте нет — фантом закрывается; есть — сделка остаётся остановленной с решённым
//! количеством (продаст человек кнопкой); человек успел нажать кнопку — его действие не перетирается.
//! Кто вызывает: core/credit_rest.rs (at_close), core/ledger.rs (finish — закрытие с остатком), core/reconcile.rs.
//! От чего зависит: credit_rest.rs (locked), credit_fact.rs, ledger.rs, deal_words.rs, deps.rs, engine.rs.
//! Настройки: нет.

use rust_decimal::Decimal;

use crate::core::types::VenueId;
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
