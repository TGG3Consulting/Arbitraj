//! Файл: tests/core_registry_chains_430.rs
//! Что делает: решение 430 (шаг 2 двух сетей) — справочник по сетям (core/registry_chains.rs) на записанном справочнике
//! сетей BingX (tests/data/bingx_2026-10-24/config_getall.json, разбор адаптера bingx wallet::parse_networks):
//! (1) одна площадка DEX — одна группа «сеть по умолчанию» с ней (как раньше); две сети — две группы;
//! (2) записи бирж без сети (сегодняшний разбор — только BEP20) — все в сети по умолчанию, USDT — адрес BEP20, у
//!     Arbitrum адреса нет (биржи отдадут адреса по сетям — шаг 3);
//! (3) записи той же биржи по сети ARBITRUM с сетью «arbitrum» — USDT Arbitrum из ответа BingX, USDT BSC прежний;
//! (4) виды площадок сети (chain::kinds_in): биржи + DEX только этой сети.
//! Ядро — t198_common (SQLite во временной папке; bingx, mexc — биржи, bsc_dex — DEX); bsc_dex и arb_dex включены
//! (решение 489: группы — только площадки DEX в игре).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core::{chain, registry_build, registry_chains}, arb::venues::bingx::wallet. Моков нет.

mod t198_common;

use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::{Address, address};
use serde_json::{Value, json};

use arb::core::chain;
use arb::core::registry_build::{Networks, usdt_address};
use arb::core::registry_chains::{self, Group};
use arb::core::types::{ChainId, CoinNetworkInfo, VenueId};
use arb::gateway::contract::VenueKind;
use arb::settings::scope::Scope;
use arb::venues::bingx::wallet;

/// USDT в сети BSC (BingX networkList, network BEP20).
const USDT_BSC: Address = address!("0x55d398326f99059ff775485246999027b3197955");
/// USDT в сети Arbitrum (BingX networkList, network ARBITRUM).
const USDT_ARB: Address = address!("0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9");

/// Площадка по id. Ошибок нет.
fn v(id: &str) -> VenueId {
    VenueId(id.into())
}

/// Справочник сетей BingX по сети `net` из записи. Паника — файла нет или разбор не прошёл.
fn bingx(net: &str) -> Vec<CoinNetworkInfo> {
    let raw = std::fs::read_to_string("tests/data/bingx_2026-10-24/config_getall.json").unwrap();
    let data: Value = serde_json::from_str(&raw).unwrap();
    wallet::parse_networks(&data["data"], net, &v("bingx")).unwrap()
}

/// Задаёт настройку. Паника — запись не прошла.
async fn set(s: &arb::settings::store::Store, name: &str, scope: Scope, val: &str) {
    s.set(name, scope, Some(json!(val)), "тест").await.unwrap();
}

#[tokio::test]
async fn registry_by_chain() {
    let core = t198_common::start("registry-chains-430").await;
    let s = core.inner.deps.settings.clone();
    // Решение 489: в группы справочника входят только площадки DEX в игре (venue.enabled = да).
    for d in ["bsc_dex", "arb_dex"] {
        let on = Some(json!(true));
        s.set("venue.enabled", Scope::Venue(d.into()), on, "тест")
            .await
            .unwrap();
    }
    let one = registry_chains::groups(&core, &chain::dexes(&core));
    assert_eq!(
        one,
        vec![Group {
            tag: None,
            dex: vec![v("bsc_dex")]
        }],
        "одна DEX — как раньше, без настроек"
    );

    let bsc = bingx("BEP20");
    assert!(
        bsc.iter().all(|i| i.asset.chain.is_none()),
        "биржа сети не ставит (шаг 3)"
    );
    let mut nets: Networks = HashMap::new();
    nets.insert(v("bingx"), Arc::new(bsc.clone()));
    let quote = HashMap::from([(v("bingx"), "USDT".to_string())]);
    assert_eq!(
        registry_chains::nets_of(&nets, None)[&v("bingx")].len(),
        bsc.len()
    );
    assert_eq!(usdt_address(&nets, &quote).unwrap(), USDT_BSC);

    core.inner
        .kinds
        .write()
        .unwrap()
        .push((v("arb_dex"), VenueKind::Dex));
    set(&s, "dex.chain", Scope::Venue("bsc_dex".into()), "bsc").await;
    set(&s, "dex.chain", Scope::Venue("arb_dex".into()), "arbitrum").await;
    set(&s, "dex.default_chain", Scope::Global, "bsc").await;
    let arb = ChainId("arbitrum".into());
    let two = registry_chains::groups(&core, &chain::dexes(&core));
    assert_eq!(two.len(), 2);
    assert_eq!(two[0].dex, vec![v("bsc_dex")]);
    assert_eq!(two[1].tag, Some(arb.clone()));
    assert!(
        registry_chains::usdt_other(&two, &nets, &quote).is_empty(),
        "адресов Arbitrum от бирж ещё нет"
    );

    // Шаг 3: биржа отдаст записи по сети ARBITRUM — с сетью «arbitrum».
    let mut on_arb = bingx("ARBITRUM");
    for i in &mut on_arb {
        i.asset.chain = Some(arb.clone());
    }
    let mut both = bsc;
    both.extend(on_arb);
    nets.insert(v("bingx"), Arc::new(both));
    let other = registry_chains::usdt_other(&two, &nets, &quote);
    assert_eq!(other.get(&arb), Some(&USDT_ARB));
    assert_eq!(
        usdt_address(&registry_chains::nets_of(&nets, None), &quote).unwrap(),
        USDT_BSC
    );

    let k = chain::kinds_in(&core, Some(&arb));
    assert!(k.iter().any(|(x, _)| *x == v("arb_dex")));
    assert!(!k.iter().any(|(x, _)| *x == v("bsc_dex")));
    let k = chain::kinds_in(&core, None);
    assert!(k.iter().any(|(x, _)| *x == v("bsc_dex")));
    assert!(!k.iter().any(|(x, _)| *x == v("arb_dex")));
}

/// Две карточки GET /api/coins/{address}?chain=... от 24.11.2026 и снимок
/// data-api.binance.vision/api/v3/depth?symbol=APEUSDT&limit=5. В обеих сетях
/// разрешён выход DEX → Binance. Одному событию стакана соответствуют два тика монет.
#[tokio::test]
async fn one_binance_book_wakes_both_ape_networks() {
    use arb::core::registry::{CoinEntry, Presence};
    use arb::core::ticker_index::index_of;
    use arb::core::types::{Asset, Direction};
    use arb::gateway::events::MarketEvent;
    use arb::venues::binance::depth;

    let rows: Value =
        serde_json::from_str(include_str!("data/binance_ape_books_2026-11-24.json")).unwrap();
    let mut coins = Vec::new();
    for row in rows["coins"].as_array().unwrap() {
        let asset: Asset = serde_json::from_value(row["asset"].clone()).unwrap();
        let dirs: Vec<Direction> = serde_json::from_value(row["directions"].clone()).unwrap();
        assert!(
            dirs.iter()
                .any(|d| d.asset == asset && d.sell_on == v("binance"))
        );
        let presence = row["presence"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| Presence {
                venue: v(p["venue"].as_str().unwrap()),
                ticker: p["ticker"].as_str().unwrap().into(),
                network: p["network"].as_str().map(str::to_owned),
                can_buy: p["can_buy"].as_bool().unwrap(),
                can_sell: p["can_sell"].as_bool().unwrap(),
                can_receive: p["can_receive"].as_bool().unwrap(),
                can_send: p["can_send"].as_bool().unwrap(),
                excluded: vec![],
                info: None,
                via: None,
            })
            .collect();
        coins.push(CoinEntry {
            asset,
            presence,
            block: None,
            problem: None,
            wait: None,
            cex_cex: None,
            cex_cex_off: vec![],
            cex_cex_deny: vec![],
        });
    }
    let core = t198_common::start("ape-book-networks").await;
    let index = index_of(&coins);
    // Перевод по тикеру по-прежнему разрешается в одну запись, а BingX в выборке
    // есть только на BSC. Не рассылаем стакан другой биржи в сеть, которой у неё нет.
    assert_eq!(
        index.get(&v("binance"), "APE"),
        Some(coins[0].asset.clone())
    );
    assert_eq!(
        index.book_assets(&v("bingx"), "APE").collect::<Vec<_>>(),
        vec![coins[0].asset.clone()]
    );
    *core.inner.tickers.write().unwrap() = index;
    let raw = rows["depth"].to_string();
    let depth: depth::Partial<'_> = serde_json::from_str(&raw).unwrap();
    let (bids, asks) = depth::levels(&depth).unwrap();
    let book = depth::to_book(
        Asset {
            ticker: "APE".into(),
            chain: None,
            contract: None,
        },
        bids,
        asks,
    );
    core.inner
        .market
        .write()
        .unwrap()
        .apply(MarketEvent::Book(book.clone()));
    let mut ticks = core.inner.coin_ticks.subscribe();
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    tx.try_send(MarketEvent::Book(book)).unwrap();
    drop(tx);
    arb::core::tick::run(core, rx).await;
    for c in coins {
        assert_eq!(ticks.try_recv().unwrap(), c.asset);
    }
    assert!(ticks.try_recv().is_err());
}
