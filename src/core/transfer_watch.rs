//! Файл: core/transfer_watch.rs
//! Что делает: наблюдение за переводом (шаг 6 ТЗ): перевод дольше transfer.max_time_s — уведомление
//! exec.transfer_slow и карантин монеты (route.quarantine_in); дошёл — карантин снят (route.quarantine_out);
//! фактическое время в пути пишет transit.rs.
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
    let at = At {
        venue: Some(&to.0),
        coin: Some(&asset.ticker),
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
    deps.notify(
        "route.quarantine_in",
        Some(&to.0),
        &tg("Монета на паузе: перевод идёт дольше нормы"),
    )
    .await;
}

/// Перевод дошёл: если монета была в карантине из-за медленного перевода — снять и уведомить.
pub async fn release(core: &Core, asset: &Asset, to: &VenueId) {
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
