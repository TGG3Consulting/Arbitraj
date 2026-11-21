//! Файл: tests/core_net_chain_489_recap.rs
//! Что делает: решение 489 (ревью 3) — переходы сети выключенной площадки DEX на записанных ответах BingX 2026-11-21
//! (tests/data/bingx_2026-11-21) и работающем ядре (t489_common; сделки — настоящим ledger::save):
//! (1) повторный захват: сеть отпущена (возврат «не вернулись»), «Перегнать» ставит часть «идёт» и сохраняет
//!     (manual_usdt.rs) — захват с толчком (Inner.net_held, поколение толчков net_kick.rs); пока справочник сетей не
//!     перечитан — USDT сети и вывод монеты сети отказывают причиной «перечитывается» (net_ready.rs), денег без адреса и
//!     точности нет; после перечитывания — USDT сети есть, вывод проходит;
//! (2) «Вернуть боту» — новая сделка на шаге продажи на выключенной arb_dex (manual_return.rs) — захват с толчком;
//! (3) ожидание общего возврата группы на НЕ первой сделке группы (DealGroup.wait, носитель — coin_group_flush.rs)
//!     держит сеть; ожидание снято без части «идёт» — отпущена;
//! (4) окно общего возврата (coin_group_flush.rs flush): правки — только в копиях, в учёт — сохранением (в коде нет
//!     get_mut по учёту) — снятый wait не виден раньше части «идёт»;
//! (5) толчок перечитывания не теряется: пришёл после начала прохода — пауза задачи справочника сетей пропускается.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (chain_play, ledger, net_hold, net_kick, net_ready, wait), t489_common. Моков нет.

mod t489_common;

use arb::core::chain_play::in_play;
use arb::core::deal::Step;
use arb::core::net_kick;
use arb::core::wait::{Pacer, Unit};
use t489_common::book::usdt_other;
use t489_common::deals::{closed_with, edit, group_waits, hold, net_held, new_deal, part};
use t489_common::{NEW, arb_chain, feed, raw, start, v};

/// Причина «сеть перечитывается» (net_ready.rs).
const REREAD: &str = "перечитывается";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recapture_by_manual_relay_and_return_to_bot() {
    let core = start("net-chain-489-recap", true, false).await;
    let a = arb_chain();
    let raw = raw(&core, NEW);
    hold(&core, "R489", "arb_dex", Some(a.clone())).await;
    closed_with(&core, "R489", part("not_returned", "arb_dex", "bingx")).await;
    assert!(!net_held(&core, "arb_dex"), "«не вернулись» — отпущена");
    let list = feed(&core, NEW, raw.clone());
    let vela = list.iter().find(|i| i.asset.ticker == "VELA").unwrap();
    assert_eq!(vela.asset.contract, None, "сети arbitrum нет");

    // «Перегнать»: часть «идёт» → ledger::save — захват с толчком.
    let was = net_kick::generation(&core);
    closed_with(&core, "R489", part("pending", "arb_dex", "bingx")).await;
    assert!(net_held(&core, "arb_dex"), "повторный захват");
    assert!(net_kick::generation(&core) > was, "толчок перечитывания");
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
    feed(&core, NEW, raw.clone());
    assert!(usdt_other(&core).contains_key(&a), "USDT сети вернулся");
    let w = arb::core::net_ready::withdraw(&core, &v("bingx"), &asset, "ARBITRUM");
    assert!(w.is_ok(), "{w:?}");

    // Возврат закончен — отпущена; «Вернуть боту» на arb_dex — новая сделка на продаже — захват с толчком.
    closed_with(&core, "R489", part("done", "arb_dex", "bingx")).await;
    assert!(!net_held(&core, "arb_dex"));
    let was = net_kick::generation(&core);
    let mut back = new_deal("B489", "arb_dex", Some(a));
    back.at = v("arb_dex");
    back.step = Step::Selling;
    arb::core::ledger::save(&core, &back).await;
    assert!(net_held(&core, "arb_dex"), "«Вернуть боту» — захват");
    assert!(net_kick::generation(&core) > was, "толчок перечитывания");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn group_wait_on_any_deal_holds_network() {
    let core = start("net-chain-489-group", true, false).await;
    hold(&core, "G2", "arb_dex", Some(arb_chain())).await;
    // Не первая сделка группы G1: её выручка ждёт общего возврата с arb_dex.
    group_waits(&core, "G2", "G1", "arb_dex").await;
    assert!(net_held(&core, "arb_dex"), "ожидание группы держит");
    assert!(in_play(&core, &v("arb_dex")));
    // Ожидание снято без части «идёт» — держателя нет, сеть отпущена.
    edit(&core, "G2", |d| d.group = None).await;
    assert!(!net_held(&core, "arb_dex"));
    assert!(!in_play(&core, &v("arb_dex")));
}

#[test]
fn group_flush_edits_copies_only() {
    let code = t489_common::code_of("src/core/coin_group_flush.rs");
    assert!(!code.contains("get_mut("), "flush правит только копии");
    assert!(code.contains("Box::pin(super::ledger::save(core, &lead)).await;"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kick_during_fetch_is_not_lost() {
    let core = start("net-chain-489-kick", true, false).await;
    let mut pacer = Pacer::new("venue.networks_poll_ms", Unit::Ms, Some("bingx".into()));
    // Проход начался (поколение запомнено), толчок пришёл во время запроса — пауза пропускается.
    let seen = net_kick::generation(&core);
    net_kick::kick(&core);
    let skipped = tokio::select! {
        biased;
        _ = net_kick::pause(&core, &mut pacer, seen) => true,
        _ = std::future::ready(()) => false,
    };
    assert!(skipped, "толчок за время прохода не потерян");
}
