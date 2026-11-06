//! Файл: core/rebuild.rs
//! Что делает: пересборка справочника монет и направлений (registry.rs) вместе с чёрным списком
//! (blacklist.rs) — по «толчку»: новый справочник сетей, новый пул или стакан, проверка монеты или
//! крючка, изменение настроек монет, налогов, крючков и площадок, а при неудавшейся подписке площадки — изменение
//! любой настройки. Толчки подряд сливаются.
//! Решение 129: сборка — на снимках рынка и сетей вне их блокировок; неизменный справочник не публикуется заново.
//! Решение 131: неподвижные монеты (без адреса контракта) — только при смене их входов (coin_book.rs).
//! Уведомляет о входе и выходе монеты из чёрного списка и об исключении пула по крючку.
//! Кто вызывает: core/engine.rs (запускает задачи run и watch_settings).
//! От чего зависит: engine.rs, registry.rs, blacklist.rs, subscribe.rs, episode_run.rs (закрыть эпизоды пропавших направлений), settings.
//! Настройки: venue.enabled, venue.quote_asset, venue.bsc_network, venue.symbols_stale_s (по площадке); coin.*, tax.*, hooks.* — через blacklist.rs.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::VenueId;
use crate::gateway::contract::VenueKind;
use crate::settings::read::At;

use super::coin_book::{CoinBook, InertKey};
use super::engine::{Core, lk, rd, wr};
use super::registry::{self, Exclusion};
use super::registry_build::{Networks, Part};
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
const REGISTRY: [&str; 10] = [
    "coin.",
    "cex_cex.",
    "tax.",
    "hooks.",
    "venue.enabled",
    "venue.quote_asset",
    "venue.bsc_network",
    "venue.symbols_stale_s",
    "dex.intermediate_",
    // Решение 181: монеты для цены затрат — в подписке биржи (subscribe.rs).
    "venue.price_assets",
];

/// Задача: изменение настроек справочника — толчок пересборки. Пока подписка какой-то площадки не удалась
/// (sub_state.rs), толчок даёт изменение любой настройки: адаптер мог упасть на любой своей (например
/// bsc.node_http_rps), и после того как её задали, подписка возвращается сама, без перезапуска.
pub async fn watch_settings(core: Core) {
    let mut w = core.inner.deps.watch(&[]);
    while let Some(c) = w.next().await {
        // Нативная монета ↔ обёртка (решение 62, native.rs): справочник сетей бирж — перечитать сразу.
        super::coin_book::on_setting(&c.name);
        // Решение 196: биржу включили — поднять поток, если молчит; выключили — держать до закрытия сделок.
        super::feed_live::on_setting(&core, &c);
        // Решение 200: склейку тикеров считает шлюз при получении справочника сетей — перечитать его сразу.
        if super::native::affects(&c.name) || c.name == crate::gateway::aliased::SETTING {
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
    // Решение 129: снимки рынка и сетей — блокировки держатся только на копию (сети — указатели за Arc, рынок — без
    // уровней стаканов); сборка справочника идёт без них, тик пишет в рынок не дожидаясь пересборки.
    let nets: Networks = rd(&inner.networks).clone();
    let market = rd(&inner.market).registry_view();
    let usdt = registry::usdt_address(&nets, &quote);
    let usdt_at = usdt.as_ref().ok().copied();
    let prev = rd(&inner.coins).clone();
    // Решение 131: неподвижные монеты (без адреса контракта ни у одной биржи — coin_book.rs) собираются заново только
    // при смене их входов: справочника сетей, названий сети BSC, котируемой монеты, площадок, ручного чёрного списка.
    let key = InertKey::new(&venues, &nets, &quote, &bsc);
    use super::rebuild_build::build;
    let (inert, inert_new) = match &prev.inert_key {
        Some(k) if k.same(&key) => (prev.inert.clone(), false),
        _ => {
            let mut v = build(
                core,
                &venues,
                &quote,
                &bsc,
                usdt_at,
                &nets,
                &market,
                Part::Inert,
            );
            for ch in super::rebuild_blacklist::apply_blacklist(core, &market, &mut v) {
                super::rebuild_report::report_block(core, ch).await;
            }
            (Arc::new(v), true)
        }
    };
    let mut coins = build(
        core,
        &venues,
        &quote,
        &bsc,
        usdt_at,
        &nets,
        &market,
        Part::Live,
    );
    let changes = super::rebuild_blacklist::apply_blacklist(core, &market, &mut coins);
    for ch in changes {
        super::rebuild_report::report_block(core, ch).await;
    }
    super::rebuild_report::report_pools(core, &coins).await;
    super::rebuild_report::report_check_done(core, &coins).await;
    let live_dirs: std::collections::HashSet<_> =
        coins.iter().flat_map(|c| c.shown_directions()).collect();
    let directions = live_dirs.len();
    let count = coins.len() + inert.len();
    // Решение 129: справочник не изменился (тот же список, тот же USDT) — карту тикеров, справочник и «Живой экран» не
    // трогаем; подписки всё равно сверяются (повтор неудавшейся подписки идёт по толчку пересборки).
    let same =
        !inert_new && *prev.live == coins && rd(&inner.usdt).as_ref().ok() == usdt.as_ref().ok();
    if same {
        super::subscribe::sync(core, &venues).await;
        return;
    }
    // Направления, которых больше нет, уходят с «Живого экрана».
    wr(&inner.spreads).retain(|d, _| live_dirs.contains(d));
    *wr(&inner.usdt) = usdt;
    // Карта (площадка, тикер) → монета для тика (решение 118, Я-9) и журнал разных тикеров одного адреса (Я-8).
    super::ticker_index::rebuild(core, &coins).await;
    *wr(&inner.coins) = CoinBook {
        live: Arc::new(coins),
        inert,
        inert_key: Some(key),
    };
    // Эпизоды направлений, которых больше нет, закрываются (решение 74) — только память и сообщения.
    super::episode_run::sweep(core);
    inner.live.send(CoreEvent::Coins {
        coins: count,
        directions,
    });
    super::subscribe::sync(core, &venues).await;
}

/// Есть ли у монеты на DEX пул с USDT (допущенный или нет — проверка налогов нужна в обоих случаях).
pub fn has_pool(excluded: &[Exclusion]) -> bool {
    !excluded
        .iter()
        .any(|e| matches!(e, Exclusion::NoUsdtPool | Exclusion::NoUsdtAddress))
}
