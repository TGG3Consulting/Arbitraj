//! Файл: tests/core_net_chain_489_held.rs
//! Что делает: решение 489 — сеть выключенной площадки DEX держится, пока сделка не завершена полностью, включая
//! возврат USDT (поток — по решению 196, сеть — core/net_hold.rs), на записанных ответах BingX 2026-11-21
//! (tests/data/bingx_2026-11-21) и работающем ядре (t489_common; справочник — напрямую, book.rs built):
//! (1) arb_dex не включена, её держит открытая сделка — записи ARBITRUM с сетью arbitrum и адресом (dirctx находит
//!     точность и шаг вывода), VELA в справочнике в сети arbitrum, у неё присутствие биржи с данными сети (подписка
//!     биржи) и флагами ввода/вывода записи; направлений нет — problem «… только идущие сделки (решение 196)», отказ
//!     исполнителя (start_reason) с этой причиной; USDT сети arbitrum известен (usdt_other); набор bsc_dex без них;
//! (2) сделка закрыта, возврат USDT с arb_dex идёт (usdt_return_state Pending) — сеть держится: USDT сети есть, монеты
//!     в своей сети, новых сделок нет; толчка перечитывания нет;
//! (3) возврат закончен — сеть отпущена одним толчком (net_hold::on_saved), адрес ARBITRUM снимается, монет arbitrum
//!     нет, USDT сети неизвестен; повторные сохранения закрытой сделки толчков не дают (шторма перечитываний нет);
//! (4) зеркальный случай: bsc_dex выключена и держится сделкой, arb_dex включена — без утечки.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (chain_play, net_hold, registry_chains, rebuild_build, start_reason), t489_common.
//! Моков нет.

mod t489_common;

use alloy_primitives::address;
use serde_json::json;

use arb::core::chain_play::in_play;
use arb::core::types::Direction;
use arb::settings::scope::Scope;
use t489_common::book::{built, cex_set, chained, dex_set, pick, usdt_other};
use t489_common::deals::{close_returning, hold, return_done, save};
use t489_common::{NEW, arb_chain, arb_only, feed, raw, start, v};

/// Причина «сеть держат только сделки» (rebuild_build.rs held_only).
const HELD: &str = "только идущие сделки (решение 196)";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_network_until_usdt_return_done() {
    let core = start("net-chain-489-held", true, false).await;
    let a = arb_chain();
    hold(&core, "H489", "arb_dex", Some(a.clone()));
    assert!(in_play(&core, &v("arb_dex")), "держит открытая сделка");
    let raw = raw(&core, NEW);
    let only = arb_only(&raw);
    let list = feed(&core, NEW, raw.clone());
    let rec = list.iter().find(|i| i.asset.ticker == "VELA").unwrap();
    assert_eq!(rec.asset.chain, Some(a.clone()));
    let vela_addr = address!("088cd8f5ef3652623c22d48b1605dcfe860cd704");
    assert_eq!(rec.asset.contract, Some(vela_addr));
    let all = built(&core);
    assert!(pick(&all, None, &only).is_empty(), "без сети нет ARB");
    let held = pick(&all, Some(&a), &only);
    assert!(held.len() > 40, "монеты держимой сети в справочнике");
    let vela = held.iter().find(|c| c.asset.ticker == "VELA").unwrap();
    let p = vela.presence.iter().find(|p| p.venue == v("bingx")).unwrap();
    let info = p.info.as_ref().unwrap();
    assert_eq!(info.network, "ARBITRUM");
    assert_eq!(p.can_send, rec.withdraw_enabled);
    assert_eq!(p.can_receive, rec.deposit_enabled);
    let on_cex = cex_set(&held, "bingx");
    assert_eq!(on_cex.len(), held.len(), "подписка биржи на монеты сети");
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
    assert!(usdt_other(&core).contains_key(&a), "USDT сети arbitrum");
    let sub = dex_set(&core, &all, "bsc_dex");
    assert_eq!(chained(&sub), 0, "набор bsc_dex без монет arbitrum");

    // Сделка закрыта, возврат выручки USDT с arb_dex на bingx идёт — сеть держится, толчка нет.
    let kick = close_returning(&core, "H489", "arb_dex", "bingx");
    assert!(!kick, "возврат идёт — сеть не отпущена");
    assert!(in_play(&core, &v("arb_dex")), "держит возврат USDT");
    feed(&core, NEW, raw.clone());
    let all = built(&core);
    assert!(usdt_other(&core).contains_key(&a), "USDT для возврата");
    let held = pick(&all, Some(&a), &only);
    assert!(held.len() > 40, "монеты в своей сети");
    let none = held.iter().all(|c| c.directions().is_empty());
    assert!(none, "новых сделок нет");

    // Возврат закончен — сеть отпущена одним толчком; повторные сохранения — без толчков.
    let kick = return_done(&core, "H489", "arb_dex", "bingx");
    assert!(kick, "один толчок");
    assert!(!in_play(&core, &v("arb_dex")));
    assert!(!save(&core, "H489", |_| {}), "повтор — без толчка");
    let again = return_done(&core, "H489", "arb_dex", "bingx");
    assert!(!again, "и снова без толчка");
    let list = feed(&core, NEW, raw);
    let rec = list.iter().find(|i| i.asset.ticker == "VELA").unwrap();
    assert_eq!(rec.asset.contract, None, "адрес снят");
    let all = built(&core);
    let bsc = all.iter().filter(|c| c.asset.contract.is_some()).count();
    assert!(bsc >= 20, "монеты BSC собраны");
    assert_eq!(chained(&all), 0, "сети arbitrum нет");
    assert!(pick(&all, None, &only).is_empty());
    assert!(!usdt_other(&core).contains_key(&a), "USDT не известен");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mirror_held_bsc_dex_no_leak() {
    let core = start("net-chain-489-mirror", true, true).await;
    let off = Scope::Venue("bsc_dex".into());
    t489_common::set(&core, "venue.enabled", off, json!(false)).await;
    hold(&core, "M489", "bsc_dex", None);
    assert!(in_play(&core, &v("bsc_dex")));
    let a = arb_chain();
    let raw = raw(&core, NEW);
    let only = arb_only(&raw);
    feed(&core, NEW, raw);
    let all = built(&core);
    assert!(pick(&all, None, &only).is_empty(), "без сети нет ARB");
    assert!(pick(&all, Some(&a), &only).len() > 40, "монеты arbitrum");
    for c in all.iter().filter(|c| c.asset.chain.is_none()) {
        let why = c.problem.as_deref().unwrap_or("");
        assert!(!why.contains(HELD), "{}: {why}", c.asset.ticker);
    }
    let sub = dex_set(&core, &all, "arb_dex");
    assert!(!sub.is_empty(), "набор arb_dex есть");
    assert!(sub.iter().all(|c| c.asset.chain == Some(a.clone())));
}
