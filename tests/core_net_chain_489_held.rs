//! Файл: tests/core_net_chain_489_held.rs
//! Что делает: решение 489 — сеть выключенной площадки DEX держится, пока по ней идут сделки (как поток по решению 196),
//! на записанных ответах BingX 2026-11-21 (tests/data/bingx_2026-11-21) и работающем ядре (t489_common):
//! (1) arb_dex зарегистрирована, не включена, её держит открытая сделка — записи ARBITRUM с сетью arbitrum и адресом
//!     (dirctx находит точность и шаг вывода), монета VELA в справочнике в сети arbitrum (не без сети), у неё
//!     присутствие биржи с данными сети (подписка биржи, subscribe.rs wanted: info) и флагами ввода/вывода записи;
//!     направлений нет — problem «… только идущие сделки (решение 196)», отказ исполнителя (start_reason) с этой
//!     причиной; USDT сети arbitrum известен (registry_chains::usdt_of); набор bsc_dex без монет arbitrum;
//! (2) сделка закрыта — сеть не держится: адрес ARBITRUM снимается, монет arbitrum нет, USDT сети неизвестен;
//! (3) зеркальный случай: bsc_dex выключена и держится сделкой, arb_dex включена — монеты arbitrum в своей сети, монеты
//!     BSC без сети и без причины «только идущие сделки» (сеть по умолчанию не останавливается), набор arb_dex без монет
//!     без сети.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (chain, feed_hold, registry_chains, rebuild, start_reason), t489_common. Моков нет.

mod t489_common;

use alloy_primitives::address;
use serde_json::json;

use arb::core::chain;
use arb::core::registry_chains::usdt_of;
use arb::core::types::Direction;
use arb::settings::scope::Scope;
use t489_common::book::{coins, dex_set, pick};
use t489_common::{NEW, arb_chain, arb_only, close, feed, hold, raw, start, v};

/// Причина «сеть держат только сделки» (rebuild_build.rs held_only).
const HELD: &str = "только идущие сделки (решение 196)";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_network_keeps_coins_without_new_deals() {
    let core = start("net-chain-489-held", true, false).await;
    let a = arb_chain();
    hold(&core, "H489", "arb_dex", Some(a.clone()));
    assert!(chain::in_play(&core, &v("arb_dex")), "держит сделка");
    let raw = raw(&core, NEW);
    let only = arb_only(&raw);
    let list = feed(&core, NEW, raw.clone());
    let rec = list.iter().find(|i| i.asset.ticker == "VELA").unwrap();
    assert_eq!(rec.asset.chain, Some(a.clone()));
    let vela_addr = address!("088cd8f5ef3652623c22d48b1605dcfe860cd704");
    assert_eq!(rec.asset.contract, Some(vela_addr));
    arb::core::rebuild::rebuild(&core).await;
    let all = coins(&core);
    assert!(pick(&all, None, &only).is_empty(), "без сети нет ARB");
    let held = pick(&all, Some(&a), &only);
    assert!(held.len() > 40, "монеты держимой сети в справочнике");
    let vela = held.iter().find(|c| c.asset.ticker == "VELA").unwrap();
    let p = vela.presence.iter().find(|p| p.venue == v("bingx")).unwrap();
    let info = p.info.as_ref().unwrap();
    assert_eq!(info.network, "ARBITRUM");
    assert_eq!(p.can_send, rec.withdraw_enabled);
    assert_eq!(p.can_receive, rec.deposit_enabled);
    for c in &held {
        let why = c.problem.as_deref().unwrap_or("");
        assert!(why.contains(HELD), "{}: {why}", c.asset.ticker);
        assert!(c.directions().is_empty());
    }
    let d = Direction {
        asset: vela.asset.clone(),
        buy_on: v("bingx"),
        sell_on: v("arb_dex"),
    };
    let why = arb::core::start_reason::not_tradable(Some(vela), &d);
    assert!(why.contains(HELD), "{why}");
    assert!(usdt_of(&core, Some(&a)).is_ok(), "USDT сети arbitrum");
    let sub = dex_set(&core, "bsc_dex");
    assert!(sub.iter().all(|c| c.asset.chain.is_none()));

    // Последняя сделка закрыта — сеть уходит.
    close(&core, "H489");
    assert!(!chain::in_play(&core, &v("arb_dex")));
    let list = feed(&core, NEW, raw);
    let rec = list.iter().find(|i| i.asset.ticker == "VELA").unwrap();
    assert_eq!(rec.asset.contract, None, "адрес снят");
    arb::core::rebuild::rebuild(&core).await;
    let all = coins(&core);
    let chained = all.iter().filter(|c| c.asset.chain.is_some()).count();
    assert_eq!(chained, 0, "сети arbitrum нет");
    assert!(pick(&all, None, &only).is_empty());
    assert!(usdt_of(&core, Some(&a)).is_err(), "USDT сети неизвестен");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mirror_held_bsc_dex_no_leak() {
    let core = start("net-chain-489-mirror", true, true).await;
    let off = Scope::Venue("bsc_dex".into());
    t489_common::set(&core, "venue.enabled", off, json!(false)).await;
    hold(&core, "M489", "bsc_dex", None);
    assert!(chain::in_play(&core, &v("bsc_dex")));
    let a = arb_chain();
    let raw = raw(&core, NEW);
    let only = arb_only(&raw);
    feed(&core, NEW, raw);
    arb::core::rebuild::rebuild(&core).await;
    let all = coins(&core);
    assert!(pick(&all, None, &only).is_empty(), "без сети нет ARB");
    assert!(pick(&all, Some(&a), &only).len() > 40, "монеты arbitrum");
    for c in all.iter().filter(|c| c.asset.chain.is_none()) {
        let why = c.problem.as_deref().unwrap_or("");
        assert!(!why.contains(HELD), "{}: {why}", c.asset.ticker);
    }
    let sub = dex_set(&core, "arb_dex");
    assert!(!sub.is_empty(), "набор arb_dex есть");
    assert!(sub.iter().all(|c| c.asset.chain == Some(a.clone())));
}
