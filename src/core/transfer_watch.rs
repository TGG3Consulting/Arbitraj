//! Файл: core/transfer_watch.rs
//! Что делает: наблюдение за переводом (шаг 6 ТЗ): перевод дольше transfer.max_time_s — уведомление
//! exec.transfer_slow и карантин монеты (route.quarantine_in); дошёл — карантин снят (route.quarantine_out);
//! фактическое время в пути — в историю для T (dirctx.rs).
//! Кто вызывает: core/transfers.rs.
//! От чего зависит: engine.rs, risk.rs, settings.
//! Настройки: transfer.max_time_s (по площадке получателя и монете).

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, VenueId};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::engine::{Core, lk};

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
    if secs <= max || lk(&core.inner.risk).quarantine.contains_key(&asset.ticker) {
        return;
    }
    let text = format!(
        "{deal}: перевод {} на {} идёт {secs} с (норма {max} с) — монета в карантине",
        asset.ticker, to.0
    );
    super::risk::quarantine(core, asset, Some(text.clone()));
    let deps = &core.inner.deps;
    deps.log(Level::Warn, "core.transfer_slow", Some(&to.0), &text, None)
        .await;
    deps.notify("exec.transfer_slow", Some(&to.0), &text).await;
    deps.notify("route.quarantine_in", Some(&to.0), &text).await;
}

/// История времени в пути (монета, откуда, куда) — для T в расчёте. Ошибок нет.
pub fn record_transit(core: &Core, asset: &Asset, from: &VenueId, to: &VenueId, secs: Decimal) {
    let mut t = lk(&core.inner.transits);
    let e = t
        .entry((asset.clone(), from.clone(), to.clone()))
        .or_insert((Decimal::ZERO, 0));
    e.0 += secs;
    e.1 += 1;
}

/// Перевод дошёл: если монета была в карантине из-за медленного перевода — снять и уведомить.
pub async fn release(core: &Core, asset: &Asset, to: &VenueId) {
    if !lk(&core.inner.risk).quarantine.contains_key(&asset.ticker) {
        return;
    }
    super::risk::quarantine(core, asset, None);
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
        .notify("route.quarantine_out", Some(&to.0), &text)
        .await;
}
