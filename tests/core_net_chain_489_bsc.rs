//! Файл: tests/core_net_chain_489_bsc.rs
//! Что делает: решение 489 — справочник BSC байт в байт прежний, пока площадка сети Arbitrum выключена: на записанном
//! справочнике сетей BingX (tests/data/bingx_2026-10-24/config_getall.json, venue.arbitrum_network = ARBITRUM задана) и
//! справочнике пар (symbols.json) справочник сетей для ядра (net_chain::prepare) и справочник монет (rebuild_build::build,
//! рабочие и неподвижные монеты — входы как у rebuild.rs) при «только bsc_dex» и при «bsc_dex + зарегистрированная, но
//! выключенная arb_dex» совпадают.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (net_chain, rebuild_build, registry_build, registry_sync), t489_common. Моков нет.

mod t489_common;

use std::collections::HashMap;

use arb::core::Core;
use arb::core::rebuild_build::build;
use arb::core::registry::CoinEntry;
use arb::core::registry_build::{Networks, Part, usdt_address};
use arb::core::types::{CoinNetworkInfo, VenueId};
use arb::gateway::contract::VenueKind;
use t489_common::{feed, start, v};

/// Справочник монет: рабочие и неподвижные.
type Book = (Vec<CoinEntry>, Vec<CoinEntry>);

/// Справочник монет ядра (рабочие, неподвижные) по входам, как их собирает core/rebuild.rs: включённые площадки,
/// котируемая монета и имя сети BSC биржи, USDT из справочника сетей. Ошибок нет.
fn book(core: &Core) -> Book {
    let kinds = core.inner.kinds.read().unwrap().clone();
    let mut vs: Vec<(VenueId, VenueKind)> = vec![];
    for (x, k) in kinds {
        if arb::core::registry_sync::enabled(core, &x) {
            vs.push((x, k));
        }
    }
    let q = HashMap::from([(v("bingx"), "USDT".to_string())]);
    let b = HashMap::from([(v("bingx"), "BEP20".to_string())]);
    let n: Networks = core.inner.networks.read().unwrap().clone();
    let m = core.inner.market.read().unwrap().registry_view();
    let u = usdt_address(&n, &q).ok();
    let live = build(core, &vs, &q, &b, u, &n, &m, Part::Live);
    let inert = build(core, &vs, &q, &b, u, &n, &m, Part::Inert);
    (live, inert)
}

/// Справочник сетей и справочник монет сценария. Ошибок нет.
async fn scenario(name: &str, with_arb: bool) -> (Vec<CoinNetworkInfo>, Book) {
    let core = start(name, with_arb, false).await;
    let nets = feed(&core);
    let b = book(&core);
    (nets, b)
}

#[tokio::test]
async fn bsc_registry_same_with_disabled_arb_dex() {
    let (nets_one, book_one) = scenario("net-chain-489-bsc-one", false).await;
    let (nets_two, book_two) = scenario("net-chain-489-bsc-two", true).await;
    assert!(!book_one.0.is_empty(), "рабочие монеты собраны");
    assert_eq!(nets_one, nets_two, "справочник сетей для ядра тот же");
    assert!(nets_two.iter().all(|i| i.asset.chain.is_none()));
    assert_eq!(book_one.0, book_two.0, "рабочие монеты те же");
    assert_eq!(book_one.1, book_two.1, "неподвижные монеты те же");
}
