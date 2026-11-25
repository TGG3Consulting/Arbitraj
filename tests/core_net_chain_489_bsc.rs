//! Файл: tests/core_net_chain_489_bsc.rs
//! Что делает: решение 489 — справочник BSC байт в байт прежний во всех состояниях площадки сети Arbitrum, на записанном
//! справочнике сетей BingX 2026-11-21 (tests/data/bingx_2026-11-21/config_getall.json: монеты BSC и 50 монет с адресом
//! только в ARBITRUM; venue.arbitrum_network = ARBITRUM задана) и справочнике пар (symbols.json). Один разбор записи —
//! вход всех состояний (поле updated_at ставит разбор, Utc::now: один разбор — одно значение, сравнивается всё).
//! Состояния: (1) только bsc_dex; (2) + arb_dex зарегистрирована, не включена, сделок нет; (3) + arb_dex не включена, её
//! держит идущая сделка (решение 196); (4) обе включены. Сравнивается (rebuild_build::build — входы как у rebuild.rs):
//! рабочие монеты без сети — во всех четырёх равны; неподвижные без сети — (1) = (2), (3) = (4), а (3) — это (1) без
//! монет, у которых сети BSC нет (их записи ARBITRUM ушли в свою сеть); записи справочника сетей вне ARBITRUM — равны во
//! всех; весь справочник сетей — (1) = (2) (адрес ARBITRUM снят), (3) = (4) (ARBITRUM с сетью arbitrum и адресом).
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
use t489_common::deals::hold;
use t489_common::{NEW, arb_chain, feed, raw, start, v};

/// Монеты справочника без сети: (рабочие, неподвижные) по входам, как их собирает core/rebuild.rs: включённые
/// площадки, котируемая монета и имя сети BSC биржи, USDT из справочника сетей. Ошибок нет.
fn bsc_book(core: &Core) -> Book {
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
    let mut live = build(core, &vs, &q, &b, u, &n, &m, Part::Live);
    let mut inert = build(core, &vs, &q, &b, u, &n, &m, Part::Inert);
    live.retain(|c| c.asset.chain.is_none());
    inert.retain(|c| c.asset.chain.is_none());
    (live, inert)
}

/// Монеты справочника: рабочие и неподвижные.
type Book = (Vec<CoinEntry>, Vec<CoinEntry>);

/// Состояние: справочник сетей для ядра и монеты без сети. Ошибок нет.
async fn state(name: &str, raw: &[CoinNetworkInfo], n: u8) -> (Vec<CoinNetworkInfo>, Book) {
    let core = start(name, n > 1, n == 4).await;
    if n == 3 {
        hold(&core, "H489", "arb_dex", Some(arb_chain())).await;
    }
    let nets = feed(&core, NEW, raw.to_vec());
    let book = bsc_book(&core);
    (nets, book)
}

/// Записи справочника сетей вне сети ARBITRUM. Ошибок нет.
fn not_arb(list: &[CoinNetworkInfo]) -> Vec<CoinNetworkInfo> {
    let mut out = list.to_vec();
    out.retain(|i| i.network != "ARBITRUM");
    out
}

#[tokio::test]
async fn bsc_registry_same_in_all_arb_states() {
    let base = start("net-chain-489-bsc-raw", true, false).await;
    let raw = raw(&base, NEW);
    let (n1, b1) = state("net-chain-489-bsc-1", &raw, 1).await;
    let (n2, b2) = state("net-chain-489-bsc-2", &raw, 2).await;
    let (n3, b3) = state("net-chain-489-bsc-3", &raw, 3).await;
    let (n4, b4) = state("net-chain-489-bsc-4", &raw, 4).await;
    assert!(b1.0.len() >= 20, "монеты BSC собраны");
    assert_eq!(b1, b2, "(2) выключенная arb_dex без сделок");
    assert_eq!(b1.0, b3.0, "(3) рабочие: arb_dex держит сделка");
    assert_eq!(b1.0, b4.0, "(4) рабочие: обе включены");
    assert_eq!(b3, b4, "(3) = (4)");
    // Неподвижные (3) — это (1) без монет, у которых нет сети BSC (VELA, MAGIC … — теперь в сети arbitrum).
    let mut gone = b1.1.clone();
    gone.retain(|c| !b3.1.contains(c));
    assert!(!gone.is_empty(), "монеты ARBITRUM ушли в свою сеть");
    for c in &gone {
        let no_bsc = c.presence.iter().all(|p| p.network.is_none());
        assert!(no_bsc, "{}", c.asset.ticker);
    }
    assert!(b3.1.iter().all(|c| b1.1.contains(c)));
    assert_eq!(n1, n2, "(1) = (2): весь справочник сетей");
    assert_eq!(n3, n4, "(3) = (4): весь справочник сетей");
    assert_eq!(not_arb(&n1), not_arb(&n3), "вне ARBITRUM — то же");
    let vela = |l: &[CoinNetworkInfo]| {
        let i = l.iter().find(|i| i.asset.ticker == "VELA");
        i.cloned().unwrap()
    };
    assert_eq!(vela(&n1).asset.contract, None, "(1): адрес снят");
    let held = vela(&n3).asset.chain;
    assert_eq!(held, Some(arb_chain()), "(3): сеть держится");
    assert!(vela(&n3).asset.contract.is_some());
}

#[tokio::test]
async fn usdt_return_from_cex_uses_the_buying_wallet_network() {
    let core = start("return-to-buying-network", true, true).await;
    let list = feed(&core, NEW, raw(&core, NEW));
    let coin = |net: &str| {
        list.iter()
            .find(|i| i.network == net && i.asset.ticker == "USDT")
            .and_then(|i| i.asset.contract)
            .unwrap()
    };
    let arb = coin("ARBITRUM");
    let bsc = coin("BEP20");
    assert_ne!(arb, bsc);
    *core.inner.usdt.write().unwrap() = Ok(bsc);
    core.inner
        .usdt_other
        .write()
        .unwrap()
        .insert(arb_chain(), arb);
    let from = v("bingx");
    for (destination, contract, network) in [
        ("arb_dex", arb, "ARBITRUM"),
        ("bsc_dex", bsc, "BEP20"),
        ("mexc", bsc, "BEP20"),
    ] {
        let asset = arb::core::usdt_return::route_asset(&core, &from, &v(destination)).unwrap();
        assert_eq!(asset.contract, Some(contract));
        assert_eq!(
            arb::core::transfers::network(&core, &asset, &from).unwrap(),
            network
        );
    }
    // When the requested chain is missing, the default chain must never substitute for it.
    core.inner.usdt_other.write().unwrap().clear();
    assert!(arb::core::usdt_return::route_asset(&core, &from, &v("arb_dex")).is_err());
}
