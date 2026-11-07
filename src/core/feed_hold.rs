//! Файл: core/feed_hold.rs
//! Что делает: решение 196 (Тигран, 06.11.2026) — биржу выключили (venue.enabled = нет), а по ней идут сделки: новых
//! сделок на площадке нет сразу (выключенная площадка выпадает из справочника направлений — rebuild.rs), но подписка
//! рынка держится до закрытия этих сделок. Идущей сделке нужен живой стакан площадки: худшая цена ордера
//! (trade_ops.rs), продажа на площадке получения (sell.rs), застрявшая (stuck.rs), «Перегон» (relay.rs), «Продать
//! сейчас». Раньше выключение сразу снимало подписку, и монета зависала («соединение молчит … — котировки нет»,
//! сделка RWA на bitget 06.11). Последняя такая сделка закрылась — следующая пересборка снимает подписку.
//! Сделка держит поток, пока открыта (Deal::is_open) или остановлена с монетами (Halted, qty > 0 — по ней жмут
//! «Продать сейчас»), и площадка — её покупка, продажа или место монет сейчас.
//! Кто вызывает: core/subscribe.rs (sync — список выключенной площадки), core/feed_live.rs (on_disabled), core/ledger.rs
//! (on_saved).
//! От чего зависит: engine.rs, deal.rs, sub_state.rs, venue_ticker.rs (тикер площадки, решение 211).
//! Настройки: нет (venue.enabled — через вызывающих).

use crate::core::types::{Asset, VenueId};
use crate::storage::journal::Level;

use super::deal::{Deal, Step};
use super::engine::{Core, lk};

/// Держит ли сделка поток площадки. Ошибок нет.
fn holds(d: &Deal, venue: &VenueId) -> bool {
    let live = d.is_open() || (d.step == Step::Halted && !d.qty.is_zero());
    let on = &d.direction.buy_on == venue || &d.direction.sell_on == venue || &d.at == venue;
    live && on
}

/// Сделки, которые держат поток площадки: (id, монета). Ошибок нет.
pub fn holders(core: &Core, venue: &VenueId) -> Vec<(String, Asset)> {
    let mut v: Vec<(String, Asset)> = lk(&core.inner.deals)
        .values()
        .filter(|d| holds(d, venue))
        .map(|d| (d.id.clone(), d.direction.asset.clone()))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

/// Список подписки выключенной площадки: пусто — сделок нет (подписка снимается); есть сделки — текущая подписка и
/// монеты сделок (после перезапуска текущей нет — только монеты сделок). Второе значение — держится ли поток.
/// Ошибок нет.
pub fn want_disabled(core: &Core, venue: &VenueId) -> (Vec<Asset>, bool) {
    let deals = holders(core, venue);
    if deals.is_empty() {
        return (vec![], false);
    }
    let mut out: Vec<Asset> = lk(&core.inner.subs)
        .have(venue)
        .map(|h| h.to_vec())
        .unwrap_or_default();
    // Решение 211 (ревью В3): монета сделки — монета ядра; площадке — с её тикером (on_venue), как в subscribe::wanted.
    out.extend(
        deals
            .into_iter()
            .map(|(_, a)| super::venue_ticker::on_venue(core, &a, venue)),
    );
    // Одна монета — один раз (Asset == по адресу; без адреса — по тикеру): сортировка по тикеру не ставит дубли рядом.
    let mut seen = std::collections::HashSet::new();
    out.retain(|a| seen.insert(a.clone()));
    out.sort_by(|a, b| a.ticker.cmp(&b.ticker));
    (out, true)
}

/// Отмечает, держится ли поток выключенной площадки; на переходе — запись в журнал. Ошибок нет.
pub async fn mark(core: &Core, venue: &VenueId, held: bool) {
    if !lk(&core.inner.subs).hold(venue, held) {
        return;
    }
    let msg = if held {
        let ids: Vec<String> = holders(core, venue).into_iter().map(|(id, _)| id).collect();
        format!(
            "Биржа выключена: новых сделок нет; поток рынка держится до закрытия сделок: {}",
            ids.join(", ")
        )
    } else {
        "Биржа выключена: сделки по ней закрыты — поток рынка снят".to_string()
    };
    core.inner
        .deps
        .log(Level::Info, "core.feed", Some(&venue.0), msg, None)
        .await;
}

/// Сделка сохранена (ledger.rs): больше не держит поток, а её площадка выключена и держит поток — толчок пересборки,
/// чтобы снять подписку сразу, а не на следующем случайном толчке. Ошибок нет.
pub fn on_saved(core: &Core, d: &Deal) {
    let venues = [&d.direction.buy_on, &d.direction.sell_on, &d.at];
    let held = {
        let subs = lk(&core.inner.subs);
        venues.iter().any(|v| subs.is_held(v) && !holds(d, v))
    };
    if held {
        core.request_rebuild();
    }
}

/// Выключение площадки (venue.enabled да → нет): сделок нет — запись «поток снят»; есть — пересборка оставит поток,
/// запись о нём делает mark из subscribe.rs. Ошибок нет.
pub async fn on_disabled(core: &Core, venue: &VenueId) {
    if holders(core, venue).is_empty() {
        core.inner
            .deps
            .log(
                Level::Info,
                "core.feed",
                Some(&venue.0),
                "Биржа выключена: идущих сделок нет — поток рынка снимается",
                None,
            )
            .await;
    }
}
