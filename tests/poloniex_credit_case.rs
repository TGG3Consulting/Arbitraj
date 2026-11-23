//! Poloniex: повтор события баланса, сверка прихода с переводом и поздний WS после REST.
//! Источники: запись core.balance_external 9417247 (23.11.2026), официальный пример канала balances.
//! Вызывает cargo test. Ядро с настоящей SQLite и адаптер Poloniex без ключей.

mod t198_common;

use std::sync::Arc;
use std::time::Duration;

use arb::core::types::{Asset, Balance, BalanceChange, BalanceKind, ChainId, VenueId};
use arb::core::{credit_event, credit_mismatch, credit_watch, transfer_credit};
use arb::settings::value::Scope;
use arb::storage::journal::Entry;
use arb::storage::raw_log::RawLog;
use arb::venues::poloniex::account_update::{self, Frame, Totals};
use arb::venues::poloniex::{Poloniex, ctx::Ctx};
use serde_json::json;
use t198_common::{d, start};

/// Выход адаптера, восстановленный по данным журнала. В нём сохранены тип, валюта и дельта,
/// а не исходные available/hold/version. Время — момент записи в журнал.
fn observed() -> BalanceChange {
    let [row]: [Entry; 1] = serde_json::from_str(include_str!(
        "data/poloniex_deposit_2026-11-23/journal.json"
    ))
    .unwrap();
    assert_eq!(row.id, 9417247);
    let (amount, source) = row
        .message
        .strip_prefix("Внешнее изменение баланса: ")
        .unwrap()
        .strip_suffix(')')
        .unwrap()
        .split_once(" (")
        .unwrap();
    let (currency, delta) = amount.split_once(" +").unwrap();
    let (kind, own) =
        account_update::kind(source.strip_prefix("Poloniex balances eventType=").unwrap());
    BalanceChange {
        venue: VenueId(row.venue.unwrap()),
        asset: Asset {
            ticker: currency.into(),
            chain: None,
            contract: None,
        },
        delta: d(delta),
        kind,
        own,
        tx_hash: None,
        at: row.at,
        source: source.into(),
        account: None,
    }
}

#[test]
fn official_balance_version_and_unchanged_total() {
    // Поля, которые читает парсер, из https://api-docs.poloniex.com/spot/websocket/balance.
    let frame = r#"{"channel":"balances","data":[{"currency":"BTC","available":"9999999983.668","hold":"16.332","eventType":"place_order","version":13641,"changeTime":1657312008411}]}"#;
    let Frame::Balances(rows) = account_update::parse(frame) else {
        panic!("balances")
    };
    let venue = VenueId("poloniex".into());
    let row = &rows[0];
    let mut totals = Totals::default();
    assert!(totals.apply(&venue, row).unwrap().own);
    assert!(totals.apply(&venue, row).is_none());
    let mut seeded = Totals::default();
    seeded.seed(&[Balance {
        venue: venue.clone(),
        asset: "BTC".into(),
        free: d("9999999983.668"),
        locked: d("16.332"),
    }]);
    assert!(seeded.apply(&venue, row).is_none());
    assert_eq!(
        account_update::kind("deposit"),
        (BalanceKind::Deposit, false)
    );
    assert_eq!(
        account_update::kind("transfer_in"),
        (BalanceKind::Other, false)
    );
    assert_eq!(account_update::kind(""), (BalanceKind::Other, false));
}

async fn configured(name: &str) -> arb::core::Core {
    let core = start(name).await;
    let scope = Scope::Venue("poloniex".into());
    for (name, value) in [
        ("transfer.credit_amount_tolerance_pct", json!("0")),
        ("transfer.credit_by_event", json!(true)),
    ] {
        core.inner
            .deps
            .settings
            .set(name, scope.clone(), Some(value), "тест")
            .await
            .unwrap();
    }
    core
}

#[tokio::test]
async fn hashless_deposit_closes_once() {
    let core = configured("poloniex-credit-case").await;
    let deps = &core.inner.deps;
    let event = observed();
    let from = VenueId("bsc_dex".into());
    let ticket =
        |id| credit_watch::register(&core, id, &from, &event.venue, &event.asset, event.delta);
    let first = ticket("first");
    let second = ticket("second");
    credit_event::on_change(&core, event.clone()).await;
    assert!(
        !first.has_event() && !second.has_event(),
        "два получателя той же суммы"
    );
    drop(second);
    credit_event::on_change(&core, event.clone()).await;
    assert_eq!(
        transfer_credit::by_event(Some(&first)).unwrap().delta,
        event.delta
    );
    let leg = transfer_credit::Leg {
        deal: "first",
        from: &from,
        to: &event.venue,
        asset: &event.asset,
        tx: String::new(),
        network: String::new(),
        started: event.at,
    };
    let gw = Poloniex::new(Ctx {
        settings: deps.settings.clone(),
        vault: deps.notifier.vault.clone(),
        http: deps.notifier.http.clone(),
        journal: deps.journal.clone(),
        raw: RawLog::start(deps.data_dir.clone(), deps.settings.clone()),
    });
    let receipt = transfer_credit::wait(&core, &leg, Arc::new(gw), Some(first)).await;
    assert_eq!(receipt.amount, event.delta);
    let rows = deps.journal.list(100, None).await.unwrap();
    assert_eq!(
        rows.iter()
            .filter(|e| e.kind == "core.transfer_credited")
            .count(),
        1
    );
}

#[tokio::test]
async fn late_ws_after_rest_does_not_credit_next_transfer() {
    let core = configured("poloniex-rest-first").await;
    let deps = &core.inner.deps;
    let event = observed();
    let from = VenueId("bsc_dex".into());
    deps.settings
        .set(
            "transfer.credit_mismatch_s",
            Scope::Venue("poloniex".into()),
            Some(json!(120)),
            "тест",
        )
        .await
        .unwrap();
    let ticket =
        |id| credit_watch::register(&core, id, &from, &event.venue, &event.asset, event.delta);
    let leg = transfer_credit::Leg {
        deal: "rest-first",
        from: &from,
        to: &event.venue,
        asset: &event.asset,
        tx: String::new(),
        network: String::new(),
        started: event.at,
    };
    // После подтверждения другого перевода по REST эту же сумму может сообщить поздний WS.
    // Метод no_event удерживает только диагностический билет и не выполняет зачисление повторно.
    let late = ticket("rest-first");
    let next = ticket("new-transfer");
    credit_event::on_change(&core, event.clone()).await;
    assert!(!next.has_event() && !late.has_event());
    drop(next);
    let audit = credit_mismatch::no_event(core.clone(), credit_mismatch::owned(&leg), late);
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(audit, credit_event::on_change(&core, event.clone()));
    })
    .await
    .expect("поздний WS завершает сверку");
    let rows = deps.journal.list(100, None).await.unwrap();
    assert!(
        !rows
            .iter()
            .any(|e| e.kind == "core.transfer_credited" || e.kind == "core.credit_mismatch")
    );
}

#[tokio::test]
async fn networkless_balance_wakes_both_networks_without_selecting_bsc() {
    let core = configured("poloniex-credit-networks").await;
    let e = observed();
    core.inner
        .deps
        .settings
        .set(
            "venue.quote_asset",
            Scope::Venue(e.venue.0.clone()),
            Some(json!("USDT")),
            "тест",
        )
        .await
        .unwrap();
    // Контракты USDT из записи справочника tests/data/bingx_2026-11-21/config_getall.json.
    let home = Asset {
        contract: Some(
            "0x55d398326f99059ff775485246999027b3197955"
                .parse()
                .unwrap(),
        ),
        ..e.asset.clone()
    };
    let arb = Asset {
        contract: Some(
            "0xfd086bc7cd5c481dcc9c85ebe478a1c0b69fcbb9"
                .parse()
                .unwrap(),
        ),
        chain: Some(ChainId("arbitrum".into())),
        ..e.asset.clone()
    };
    *core.inner.usdt.write().unwrap() = Ok(home.contract.unwrap());
    let bsc_ticket = credit_watch::register(
        &core,
        "home",
        &VenueId("bsc_dex".into()),
        &e.venue,
        &home,
        e.delta,
    );
    let arb_ticket = credit_watch::register(
        &core,
        "arb",
        &VenueId("arb_dex".into()),
        &e.venue,
        &arb,
        e.delta,
    );
    credit_event::on_change(&core, e.clone()).await;
    assert!(!bsc_ticket.has_event() && !arb_ticket.has_event());
    for t in [&bsc_ticket, &arb_ticket] {
        tokio::time::timeout(Duration::from_secs(1), t.wake.notified())
            .await
            .expect("немедленный опрос по хешу");
    }
    drop(arb_ticket);
    credit_event::on_change(&core, e.clone()).await;
    assert!(
        bsc_ticket.has_event(),
        "однозначный основной USDT соответствует справочнику"
    );
    drop(bsc_ticket);
    let secondary = credit_watch::register(
        &core,
        "only-arb",
        &VenueId("arb_dex".into()),
        &e.venue,
        &arb,
        e.delta,
    );
    credit_event::on_change(&core, e).await;
    assert!(
        !secondary.has_event(),
        "без подтверждения сети нужен опрос по хешу"
    );
}
