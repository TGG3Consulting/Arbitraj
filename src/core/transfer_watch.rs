//! Файл: core/transfer_watch.rs
//! Что делает: наблюдение за переводом (шаг 6 ТЗ): перевод дольше transfer.max_time_s — уведомление
//! exec.transfer_slow и карантин монеты (route.quarantine_in); дошёл — карантин снят (route.quarantine_out);
//! фактическое время в пути пишет transit.rs. Решение 394: карантин снимает перевод той сделки, что его поставила.
//! Решение 411 (триггер г): перевод дольше нормы — адрес депозита получателя будет запрошен заново (deposit_proof.rs).
//! Кто вызывает: core/transfers.rs.
//! От чего зависит: engine.rs, risk.rs, settings.
//! Настройки: transfer.max_time_s (по площадке получателя и монете).

use chrono::Utc;

use crate::core::types::{Asset, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::Core;

/// Перевод идёт дольше transfer.max_time_s — один раз: уведомление и карантин монеты (ТЗ, шаг 6).
pub async fn slow(
    core: &Core,
    deal: &str,
    to: &VenueId,
    asset: &Asset,
    started: chrono::DateTime<Utc>,
) {
    let coin = super::coin_key::scope_id(asset);
    let at = At {
        venue: Some(&to.0),
        coin: Some(&coin),
    };
    let Ok(max) = core.inner.deps.settings.integer("transfer.max_time_s", at) else {
        return;
    };
    let secs = (Utc::now() - started).num_seconds().max(0) as u64;
    if secs <= max || super::quarantine::reason(core, asset).is_some() {
        return;
    }
    let text = format!(
        "{deal}: перевод {} на {} идёт {secs} с (норма {max} с) — монета в карантине",
        asset.ticker, to.0
    );
    super::quarantine::set(core, asset, Some(text.clone()));
    let deps = &core.inner.deps;
    deps.log(Level::Warn, "core.transfer_slow", Some(&to.0), &text, None)
        .await;
    let tg = |head: &str| {
        crate::notify::human::Msg::new(crate::notify::human::WARN, head)
            .line(format!(
                "{} → {}",
                asset.ticker,
                crate::notify::human::venue(&to.0)
            ))
            .kv("В пути", format!("{secs} с — норма {max} с"))
            .line("Монета на паузе (карантин), пока перевод не дойдёт")
            .id(deal)
            .render()
    };
    deps.notify(
        "exec.transfer_slow",
        Some(&to.0),
        &tg("Перевод идёт дольше нормы"),
    )
    .await;
    // Решение 408: «Снять карантин» на тревоге (c:qr — то же, что меню /menu).
    deps.notify_acts(
        "route.quarantine_in",
        Some(&to.0),
        &tg("Монета на паузе: перевод идёт дольше нормы"),
        vec![crate::notify::acts::release_quarantine(
            &super::quarantine::id_of(asset),
            &asset.ticker,
        )],
    )
    .await;
    // Решение 411 (триггер г): перевод не дошёл за срок — адрес депозита получателя (биржи) при следующем
    // использовании запрашивается заново (у DEX-получателя пометка ни на что не влияет: его адрес — кошелёк счёта).
    let why = format!("{deal}: перевод идёт {secs} с при норме {max} с");
    super::deposit_proof::doubt(core, to, asset, &why).await;
}

/// Перевод дошёл: если монета была в карантине из-за медленного перевода — снять и уведомить. Решение 394 (ревью t179,
/// Н4): при exec.top_up = да карантин медленного перевода ДРУГОЙ сделки (два перевода одной монеты) не снимается — его снимет её
/// собственный перевод. Принимает: ядро, монету, получателя, ID сделки этого перевода. Ошибок нет.
pub async fn release(core: &Core, asset: &Asset, to: &VenueId, deal: &str) {
    if super::coin_ride::top_up_on(core)
        && let Some(r) = super::quarantine::reason(core, asset)
        && slow_of_other(&r, deal)
    {
        return;
    }
    if super::quarantine::set(core, asset, None).is_none() {
        return;
    }
    let text = format!(
        "{}: перевод на {} дошёл — карантин снят",
        asset.ticker, to.0
    );
    core.inner
        .deps
        .log(Level::Info, "core.quarantine", Some(&to.0), &text, None)
        .await;
    core.inner
        .deps
        .notify(
            "route.quarantine_out",
            Some(&to.0),
            &crate::notify::human::Msg::new(crate::notify::human::OK, "Монета снова торгуется")
                .line(format!(
                    "{}: перевод на {} дошёл",
                    asset.ticker,
                    crate::notify::human::venue(&to.0)
                ))
                .render(),
        )
        .await;
}

/// Решение 394: причина карантина — медленный перевод другой сделки (текст slow: «<сделка>: перевод …»). Принимает:
/// причину, ID этой сделки. Ошибок нет.
fn slow_of_other(reason: &str, deal: &str) -> bool {
    match reason.split_once(": перевод ") {
        Some((id, _)) => id != deal,
        None => false,
    }
}
