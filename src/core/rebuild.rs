//! Файл: core/rebuild.rs
//! Что делает: пересборка справочника монет и направлений (registry.rs) вместе с чёрным списком
//! (blacklist.rs) — по «толчку»: новый справочник сетей, новый пул или стакан, проверка монеты или
//! крючка, изменение настроек монет, налогов, крючков и площадок, а при неудавшейся подписке площадки — изменение
//! любой настройки. Толчки подряд сливаются.
//! Уведомляет о входе и выходе монеты из чёрного списка и об исключении пула по крючку.
//! Кто вызывает: core/engine.rs (запускает задачи run и watch_settings).
//! От чего зависит: engine.rs, registry.rs, blacklist.rs, subscribe.rs, episode_run.rs (закрыть эпизоды пропавших направлений), settings.
//! Настройки: venue.enabled, venue.quote_asset, venue.bsc_network, venue.symbols_stale_s (по площадке); coin.*, tax.*, hooks.* — через blacklist.rs.

use std::collections::HashMap;

use chrono::Utc;

use crate::core::types::{Pool, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::blacklist;
use super::engine::{Core, lk, rd, wr};
use super::registry::{self, CoinEntry, Exclusion, Inputs};
use super::registry_sync::enabled;
use super::view::CoreEvent;

/// Задача пересборки: ждёт толчка и пересобирает. Работает, пока жива система.
pub async fn run(core: Core) {
    loop {
        core.inner.kick_rebuild.notified().await;
        rebuild(&core).await;
    }
}

/// Настройки, от которых зависит справочник: их изменение — всегда толчок пересборки.
const REGISTRY: [&str; 8] = [
    "coin.",
    "tax.",
    "hooks.",
    "venue.enabled",
    "venue.quote_asset",
    "venue.bsc_network",
    "venue.symbols_stale_s",
    "dex.intermediate_",
];

/// Задача: изменение настроек справочника — толчок пересборки. Пока подписка какой-то площадки не удалась
/// (sub_state.rs), толчок даёт изменение любой настройки: адаптер мог упасть на любой своей (например
/// bsc.node_http_rps), и после того как её задали, подписка возвращается сама, без перезапуска.
pub async fn watch_settings(core: Core) {
    let mut w = core.inner.deps.watch(&[]);
    while let Some(c) = w.next().await {
        // Нативная монета ↔ обёртка (решение 62, native.rs): справочник сетей бирж — перечитать сразу.
        if super::native::affects(&c.name) {
            core.inner.kick_networks.notify_waiters();
        }
        if retry_on(&c.name, lk(&core.inner.subs).any_failed()) {
            core.request_rebuild();
        }
    }
}

/// Нужна ли пересборка после изменения настройки `name`: настройка справочника или есть неудавшаяся подписка.
/// Ошибок нет.
pub fn retry_on(name: &str, any_failed: bool) -> bool {
    any_failed || REGISTRY.iter().any(|p| name.starts_with(p))
}

/// Пересобирает справочник и чёрный список, публикует итог, обновляет подписки площадок.
/// Ошибок нет: причины отсева видны в самом справочнике.
pub async fn rebuild(core: &Core) {
    let inner = &core.inner;
    let s = &inner.deps.settings;
    let venues: Vec<(VenueId, VenueKind)> = rd(&inner.kinds)
        .iter()
        .filter(|(v, _)| enabled(core, v))
        .cloned()
        .collect();
    let mut quote = HashMap::new();
    let mut bsc = HashMap::new();
    for (v, _) in venues.iter().filter(|(_, k)| *k == VenueKind::Cex) {
        let at = At {
            venue: Some(&v.0),
            coin: None,
        };
        if let Ok(q) = s.text("venue.quote_asset", at) {
            quote.insert(v.clone(), q);
        }
        if let Ok(n) = s.text("venue.bsc_network", at) {
            bsc.insert(v.clone(), n);
        }
    }
    let usdt = registry::usdt_address(&rd(&inner.networks), &quote);
    let mut coins = build(core, &venues, &quote, &bsc, usdt.as_ref().ok().copied());
    let changes = super::rebuild_blacklist::apply_blacklist(core, &mut coins);
    for ch in changes {
        super::rebuild_report::report_block(core, ch).await;
    }
    super::rebuild_report::report_pools(core, &coins).await;
    super::rebuild_report::report_check_done(core, &coins).await;
    let live_dirs: std::collections::HashSet<_> =
        coins.iter().flat_map(|c| c.shown_directions()).collect();
    let directions = live_dirs.len();
    let count = coins.len();
    // Направления, которых больше нет, уходят с «Живого экрана».
    wr(&inner.spreads).retain(|d, _| live_dirs.contains(d));
    *wr(&inner.usdt) = usdt;
    *wr(&inner.coins) = coins;
    // Эпизоды направлений, которых больше нет, закрываются (решение 74) — только память и сообщения.
    super::episode_run::sweep(core);
    inner.live.send(CoreEvent::Coins {
        coins: count,
        directions,
    });
    super::subscribe::sync(core, &venues).await;
}

/// Сборка справочника под блокировками рынка и сетей (без await).
fn build(
    core: &Core,
    venues: &[(VenueId, VenueKind)],
    quote: &HashMap<VenueId, String>,
    bsc_network: &HashMap<VenueId, String>,
    usdt: Option<alloy_primitives::Address>,
) -> Vec<CoinEntry> {
    let s = &core.inner.deps.settings;
    let market = rd(&core.inner.market);
    // Свежесть справочника пар каждой биржи (pairs_fresh.rs): устаревший — по её парам не торгуем.
    let now = Utc::now();
    let pairs_fresh: HashMap<VenueId, Result<(), String>> = venues
        .iter()
        .filter(|(_, k)| *k == VenueKind::Cex)
        .map(|(v, _)| {
            let limit = super::pairs_fresh::limit(s, v);
            (
                v.clone(),
                super::pairs_fresh::check(&market, v, &limit, now),
            )
        })
        .collect();
    let pool_ok =
        |p: &Pool| blacklist::pool_admission(p, market.pool_check(&p.venue, &p.reference), s);
    let nets = rd(&core.inner.networks);
    let intermediate: HashMap<VenueId, super::intermediate::Intermediate> = venues
        .iter()
        .filter(|(_, k)| *k == VenueKind::Dex)
        .map(|(v, _)| (v.clone(), super::intermediate::read(s, v)))
        .collect();
    registry::build(&Inputs {
        venues,
        networks: &nets,
        usdt,
        quote,
        bsc_network,
        pairs_fresh: &pairs_fresh,
        market: &market,
        pool_ok: &pool_ok,
        intermediate: &intermediate,
    })
}

/// Есть ли у монеты на DEX пул с USDT (допущенный или нет — проверка налогов нужна в обоих случаях).
pub fn has_pool(excluded: &[Exclusion]) -> bool {
    !excluded
        .iter()
        .any(|e| matches!(e, Exclusion::NoUsdtPool | Exclusion::NoUsdtAddress))
}
