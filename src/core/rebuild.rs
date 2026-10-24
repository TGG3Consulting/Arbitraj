//! Файл: core/rebuild.rs
//! Что делает: пересборка справочника монет и направлений (registry.rs) вместе с чёрным списком
//! (blacklist.rs) — по «толчку»: новый справочник сетей, новый пул или стакан, проверка монеты или
//! крючка, изменение настроек монет, налогов, крючков и площадок, а при неудавшейся подписке площадки — изменение
//! любой настройки. Толчки подряд сливаются.
//! Уведомляет о входе и выходе монеты из чёрного списка и об исключении пула по крючку.
//! Кто вызывает: core/engine.rs (запускает задачи run и watch_settings).
//! От чего зависит: engine.rs, registry.rs, blacklist.rs, subscribe.rs, settings.
//! Настройки: venue.enabled, venue.quote_asset (по площадке); coin.*, tax.*, hooks.* — через blacklist.rs.

use std::collections::HashMap;

use chrono::Utc;

use crate::core::types::{Pool, VenueId};
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::blacklist::{self, BlockChange, TaxLimits};
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
const REGISTRY: [&str; 6] = [
    "coin.",
    "tax.",
    "hooks.",
    "venue.enabled",
    "venue.quote_asset",
    "dex.intermediate_",
];

/// Задача: изменение настроек справочника — толчок пересборки. Пока подписка какой-то площадки не удалась
/// (sub_state.rs), толчок даёт изменение любой настройки: адаптер мог упасть на любой своей (например
/// bsc.node_http_rps), и после того как её задали, подписка возвращается сама, без перезапуска.
pub async fn watch_settings(core: Core) {
    let mut w = core.inner.deps.watch(&[]);
    while let Some(c) = w.next().await {
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
    for (v, _) in venues.iter().filter(|(_, k)| *k == VenueKind::Cex) {
        let at = At {
            venue: Some(&v.0),
            coin: None,
        };
        if let Ok(q) = s.text("venue.quote_asset", at) {
            quote.insert(v.clone(), q);
        }
    }
    let usdt = registry::usdt_address(&rd(&inner.networks), &quote);
    let mut coins = build(core, &venues, &quote, usdt.as_ref().ok().copied());
    let changes = apply_blacklist(core, &mut coins);
    for ch in changes {
        super::rebuild_report::report_block(core, ch).await;
    }
    super::rebuild_report::report_pools(core, &coins).await;
    super::rebuild_report::report_check_done(core, &coins).await;
    let live_dirs: std::collections::HashSet<_> =
        coins.iter().flat_map(|c| c.directions()).collect();
    let directions = live_dirs.len();
    let count = coins.len();
    // Направления, которых больше нет, уходят с «Живого экрана».
    wr(&inner.spreads).retain(|d, _| live_dirs.contains(d));
    *wr(&inner.usdt) = usdt;
    *wr(&inner.coins) = coins;
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
    usdt: Option<alloy_primitives::Address>,
) -> Vec<CoinEntry> {
    let s = &core.inner.deps.settings;
    let market = rd(&core.inner.market);
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
        market: &market,
        pool_ok: &pool_ok,
        intermediate: &intermediate,
    })
}

/// Чёрный список по справочнику: ручная отметка, проверки налогов со всех DEX, где у монеты есть
/// пул с USDT. Заполняет block / problem у монет. Возвращает: изменения для уведомлений.
fn apply_blacklist(core: &Core, coins: &mut [CoinEntry]) -> Vec<BlockChange> {
    let s = &core.inner.deps.settings;
    let limits = TaxLimits::read(s);
    let market = rd(&core.inner.market);
    let mut bl = wr(&core.inner.blacklist);
    let now = Utc::now();
    let mut changes = vec![];
    for c in coins.iter_mut() {
        let checks: Vec<_> = match c.asset.bsc_contract {
            Some(token) => c
                .presence
                .iter()
                .filter(|p| p.info.is_none() && has_pool(&p.excluded))
                .map(|p| market.coin_check(&p.venue, token))
                .collect(),
            None => vec![],
        };
        let manual = blacklist::manual(s, &c.asset.ticker);
        match blacklist::reason(manual, &checks, &limits) {
            Ok(r) => {
                changes.push(bl.set(&c.asset, r, now));
                c.block = bl.get(&c.asset).cloned();
            }
            Err(e) => c.problem = Some(e),
        }
    }
    changes
}

/// Есть ли у монеты на DEX пул с USDT (допущенный или нет — проверка налогов нужна в обоих случаях).
pub fn has_pool(excluded: &[Exclusion]) -> bool {
    !excluded
        .iter()
        .any(|e| matches!(e, Exclusion::NoUsdtPool | Exclusion::NoUsdtAddress))
}
