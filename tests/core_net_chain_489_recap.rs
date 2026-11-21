//! Файл: tests/core_net_chain_489_recap.rs
//! Что делает: решение 489 (ревью 3, 4) — переходы сети выключенной площадки DEX и толчки перечитывания:
//! (1) на записанных ответах BingX 2026-11-21 (tests/data/bingx_2026-11-21), сделки — настоящим ledger::save:
//!     «Перегнать» по «не вернулись» захватывает сеть; пока справочник сетей не перечитан — USDT сети и вывод монеты
//!     сети отказывают причиной «перечитывается» (net_ready.rs), денег без адреса и точности нет; после — есть;
//! (2) на ядре без настроек (t489_common bare: фоновым задачам нечего делать, тест на одном потоке без await между
//!     шагами — посторонних толчков нет) — возвращаемое значение net_hold::on_saved и поколение толчков точно:
//!     захват «Перегнать» и «Вернуть боту» — толчок; повтор — нет; ожидание группы на НЕ первой сделке держит сеть;
//!     отпускание — толчок; chain_play.rs live первым видит переход (ledger.rs: on_saved после записи в базу) — толкает
//!     он, on_saved второго толчка не даёт;
//! (3) толчок перечитывания не теряется (net_kick.rs): пришёл за проход — пауза пропускается; толчка нет — пауза
//!     ждёт (тестовый срок); толчок во время паузы — будит.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (chain_play, ledger, net_hold, net_kick, net_ready, wait), t489_common. Моков нет.

mod t489_common;

use std::time::Duration;

use arb::core::chain_play::in_play;
use arb::core::deal::Step;
use arb::core::net_hold::on_saved;
use arb::core::net_kick::{self, generation};
use arb::core::wait::{Pacer, Unit};
use t489_common::book::usdt_other;
use t489_common::deals::{closed_with, hold, net_held, new_deal, part, remember};
use t489_common::{NEW, arb_chain, bare, feed, raw, start, v};

/// Причина «сеть перечитывается» (net_ready.rs).
const REREAD: &str = "перечитывается";
/// Тестовый срок: пауза без толчка за это время не кончается (интервал опроса в ядре без настроек не задан).
const LIMIT: Duration = Duration::from_millis(300);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manual_relay_waits_for_reread() {
    let core = start("net-chain-489-recap", true, false).await;
    let a = arb_chain();
    let raw = raw(&core, NEW);
    hold(&core, "R489", "arb_dex", Some(a.clone())).await;
    closed_with(&core, "R489", part("not_returned", "arb_dex", "bingx")).await;
    assert!(!net_held(&core, "arb_dex"), "«не вернулись» — отпущена");
    let list = feed(&core, NEW, raw.clone());
    let vela = list.iter().find(|i| i.asset.ticker == "VELA").unwrap();
    assert_eq!(vela.asset.contract, None, "сети arbitrum нет");
    closed_with(&core, "R489", part("pending", "arb_dex", "bingx")).await;
    assert!(net_held(&core, "arb_dex"), "«Перегнать» — захват");
    // Справочник сетей ещё прежний — USDT сети и вывод монеты сети: «перечитывается», денег нет.
    let usdt = arb::core::net_ready::usdt_of(&core, &v("arb_dex"), Some(&a));
    let reread = usdt.as_ref().is_err_and(|e| e.contains(REREAD));
    assert!(reread, "{usdt:?}");
    let vela_raw = raw.iter().find(|i| i.asset.ticker == "VELA").unwrap();
    let mut asset = vela_raw.asset.clone();
    asset.chain = Some(a.clone());
    let w = arb::core::net_ready::withdraw(&core, &v("bingx"), &asset, "ARBITRUM");
    let reread = w.as_ref().is_err_and(|e| e.contains(REREAD));
    assert!(reread, "{w:?}");
    // Перечитали — сеть вернулась.
    feed(&core, NEW, raw);
    assert!(usdt_other(&core).contains_key(&a), "USDT сети вернулся");
    let w = arb::core::net_ready::withdraw(&core, &v("bingx"), &asset, "ARBITRUM");
    assert!(w.is_ok(), "{w:?}");
}

#[tokio::test]
async fn capture_and_release_kick_exactly_once() {
    let core = bare("net-chain-489-kicks").await;
    let arb = v("arb_dex");
    // «Не вернулись» — сеть не держится, толчка нет.
    let mut d = new_deal("K489", "arb_dex", Some(arb_chain()));
    d.step = Step::Closed;
    d.usdt_return = Some(part("not_returned", "arb_dex", "bingx"));
    remember(&core, &d);
    assert!(!on_saved(&core, &d));
    // «Перегнать»: часть «идёт» — захват, ровно один толчок; повтор — без толчка.
    let was = generation(&core);
    d.usdt_return = Some(part("pending", "arb_dex", "bingx"));
    remember(&core, &d);
    assert!(on_saved(&core, &d), "захват — толчок");
    assert_eq!(generation(&core), was + 1);
    assert!(!on_saved(&core, &d), "повтор — без толчка");
    // «Ждёт повтора» — держится; «вернулись» — отпущена одним толчком.
    d.usdt_return = Some(part("retrying", "arb_dex", "bingx"));
    remember(&core, &d);
    assert!(!on_saved(&core, &d));
    d.usdt_return = Some(part("done", "arb_dex", "bingx"));
    remember(&core, &d);
    assert!(on_saved(&core, &d), "отпускание — толчок");
    assert!(!on_saved(&core, &d), "повтор — без толчка");
    assert!(!net_held(&core, "arb_dex"));
    // «Вернуть боту»: новая сделка на продаже на arb_dex — захват.
    let mut back = new_deal("B489", "arb_dex", Some(arb_chain()));
    back.at = arb.clone();
    back.step = Step::Selling;
    remember(&core, &back);
    assert!(on_saved(&core, &back), "«Вернуть боту» — захват");
    // Ожидание общего возврата группы на НЕ первой сделке держит сеть, хотя «Вернуть боту» уже закрыта.
    let mut g = new_deal("G2", "arb_dex", Some(arb_chain()));
    g.step = Step::Closed;
    let grp = arb::core::deal_group::DealGroup {
        id: "G1".into(),
        wait: Some((arb.clone(), rust_decimal::Decimal::ONE)),
        ..Default::default()
    };
    g.group = Some(Box::new(grp));
    remember(&core, &g);
    back.step = Step::Closed;
    remember(&core, &back);
    assert!(!on_saved(&core, &back), "держит ожидание группы");
    assert!(net_held(&core, "arb_dex"));
    g.group = None;
    remember(&core, &g);
    assert!(on_saved(&core, &g), "ожидание снято — отпускание");
}

#[tokio::test]
async fn live_kicks_before_on_saved() {
    let core = bare("net-chain-489-live").await;
    let arb = v("arb_dex");
    let mut d = new_deal("L489", "arb_dex", Some(arb_chain()));
    remember(&core, &d);
    // Пересборка (in_play) увидела держателя раньше on_saved — она и толкает.
    let was = generation(&core);
    assert!(in_play(&core, &arb));
    assert_eq!(generation(&core), was + 1, "новая отметка — толчок");
    assert!(in_play(&core, &arb));
    assert_eq!(generation(&core), was + 1, "отметка стоит — без толчка");
    assert!(!on_saved(&core, &d), "on_saved — второго толчка нет");
    d.step = Step::Closed;
    remember(&core, &d);
    assert!(!in_play(&core, &arb));
    assert_eq!(generation(&core), was + 2, "снятая отметка — толчок");
    assert!(!on_saved(&core, &d), "отпущено раньше — без толчка");
}

#[tokio::test]
async fn kick_is_not_lost() {
    let core = bare("net-chain-489-kick").await;
    let mut pacer = Pacer::new("venue.networks_poll_ms", Unit::Ms, Some("bingx".into()));
    // Толчок за проход — пауза пропускается.
    let seen = generation(&core);
    net_kick::kick(&core);
    let skipped = tokio::select! {
        biased;
        _ = net_kick::pause(&core, &mut pacer, seen) => true,
        _ = std::future::ready(()) => false,
    };
    assert!(skipped, "толчок за время прохода не потерян");
    // Толчка нет — пауза ждёт.
    let seen = generation(&core);
    let waited = tokio::time::timeout(LIMIT, net_kick::pause(&core, &mut pacer, seen)).await;
    assert!(waited.is_err(), "без толчка пауза не пропускается");
    // Толчок во время паузы — будит.
    let seen = generation(&core);
    let woke = tokio::select! {
        biased;
        _ = net_kick::pause(&core, &mut pacer, seen) => true,
        _ = async {
            tokio::task::yield_now().await;
            net_kick::kick(&core);
            tokio::time::sleep(LIMIT).await;
        } => false,
    };
    assert!(woke, "толчок во время паузы будит");
}
