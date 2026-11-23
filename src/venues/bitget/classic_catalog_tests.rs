//! Проверяет каталог Classic, подписку, REST и допуск ордера на записанных ответах Bitget.
//! См. docs/notes/bitget-classic-reality-2026-11-22.md. Сбой поля воспроизводится его удалением.

use super::{classic_list, parse_list};
use crate::core::types::{Asset, Side, TradeRequest};
use crate::gateway::contract::Venue;
use crate::settings::{scope::Scope, store::Store};
use crate::storage::{db::Db, journal::Journal, raw_log::RawLog};
use crate::venues::bitget::{Bitget, ctx::Ctx, pairs::Snapshot, symbols::Symbols};
use chrono::Utc;
use serde_json::{Value, json};

fn rows() -> (Value, Value) {
    let v: Value = serde_json::from_str(include_str!(
        "../../../tests/data/bitget_classic_reality_2026-11-23.json"
    ))
    .unwrap();
    (
        v["symbols"]["data"].clone(),
        v["instruments"]["data"].clone(),
    )
}

fn assets() -> [Asset; 4] {
    ["CAKE", "rROL", "RENDER", "RWA"].map(|ticker| Asset {
        chain: None,
        ticker: ticker.into(),
        contract: None,
    })
}

#[test]
fn recorded_crypto_and_reality_have_separate_subscription_and_rules() {
    let (v2, v3) = rows();
    let (catalog, skipped) = classic_list(&v2, &v3, "USDT").unwrap();
    assert!(skipped.is_empty());
    let directory = Symbols::default();
    let at = Utc::now();
    directory.publish(catalog, at);
    let (snap, loaded_at) = Snapshot::build(&directory, &assets()).unwrap();
    assert_eq!(loaded_at, at);
    assert_eq!(snap.reality, 1);
    assert!(snap.missing.is_empty());
    assert_eq!(snap.tickers, ["CAKE", "RENDER", "RWA"]);
    assert_eq!(
        snap.books
            .iter()
            .map(|(_, s)| s.as_str())
            .collect::<Vec<_>>(),
        ["CAKEUSDT", "RENDERUSDT", "RWAUSDT"]
    );
    for ticker in snap.tickers {
        assert!(directory.all()[&ticker].tradable());
    }
}

#[test]
fn unusable_catalog_fails_but_an_ambiguous_symbol_only_blocks_itself() {
    let (v2, mut v3) = rows();
    assert!(classic_list(&v2, &json!([]), "USDT").is_err());
    let mut unknown = v3.clone();
    for row in unknown.as_array_mut().unwrap() {
        row.as_object_mut().unwrap().remove("isReality");
    }
    assert!(classic_list(&v2, &unknown, "USDT").is_err());
    let duplicate = v3
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["symbol"] == "RWAUSDT")
        .unwrap()
        .clone();
    v3.as_array_mut().unwrap().push(duplicate);
    let (catalog, skipped) = classic_list(&v2, &v3, "USDT").unwrap();
    assert_eq!(catalog.classic.len(), 2);
    assert!(!catalog.classic.contains_key("RWA"));
    assert_eq!(skipped.len(), 1);
}

#[test]
fn explicit_reality_without_any_confirmed_classic_is_authoritative() {
    let (v2, mut v3) = rows();
    v3.as_array_mut()
        .unwrap()
        .retain(|r| r["symbol"] == "RROLUSDT");
    let (catalog, skipped) = classic_list(&v2, &v3, "USDT").unwrap();
    assert!(catalog.classic.is_empty());
    assert!(catalog.reality.contains("rROL"));
    assert_eq!(skipped.len(), 3);
}

/// Настоящие настройки и обработчики адаптера, выключенное хранилище секретов.
async fn adapter() -> Bitget {
    let dir = std::env::temp_dir().join(format!("arb-bitget-classic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("bot.sqlite")).unwrap();
    let journal = Journal::new(db.clone(), tokio::sync::broadcast::channel(16).0);
    let settings = Store::load(db, tokio::sync::broadcast::channel(16).0, journal.clone())
        .await
        .unwrap();
    settings
        .set(
            "venue.symbols_refresh_s",
            Scope::Venue("bitget".into()),
            Some(json!(60)),
            "test",
        )
        .await
        .unwrap();
    let raw = RawLog::start(dir.join("raw"), settings.clone());
    Bitget::new(Ctx {
        settings,
        journal,
        raw,
        http: reqwest::Client::new(),
        vault: crate::secrets::Vault::locked(dir.join("secrets.enc")),
    })
}

#[tokio::test]
async fn reclassified_pair_is_revoked_despite_another_missing_class_and_recovers_independently() {
    let (v2, v3) = rows();
    let bot = adapter().await;
    let assets = assets();
    // Набор работающей версии до классификации: V2 считает все четыре пары online.
    bot.symbols
        .load(parse_list(&v2, "USDT").unwrap().0, Utc::now());
    let assigned = Snapshot::build(&bot.symbols, &assets).unwrap().0.books;
    let _connection = bot
        .books
        .feed
        .register("market-1", &assets.each_ref().map(|a| a.ticker.clone()));
    let _reality_only = bot
        .books
        .feed
        .register("market-2", std::slice::from_ref(&assets[1].ticker));
    let mut partial = v3.clone();
    let rwa = partial
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["symbol"] == "RWAUSDT")
        .unwrap();
    rwa.as_object_mut().unwrap().remove("isReality");
    let (catalog, skipped) = classic_list(&v2, &partial, "USDT").unwrap();
    assert_eq!(skipped.len(), 1);
    bot.symbols.publish(catalog, Utc::now());
    // Работающее соединение тоже переходит на новый набор, в том числе после неудачной переподписки.
    bot.symbols.wait_books_change(&assigned, &assigned).await;
    let current = bot.symbols.active_books(&assigned);
    assert_eq!(
        current
            .iter()
            .map(|(a, _)| a.ticker.as_str())
            .collect::<Vec<_>>(),
        ["CAKE", "RENDER"]
    );
    let health = bot.feed_health().unwrap();
    let one = bot.feed_health_for(Some(("market-1", "CAKE"))).unwrap();
    assert_eq!(one.conns.len(), 1);
    assert_eq!(one.conns[0].pairs.len(), 1);
    assert_eq!(one.conns[0].pairs[0].ticker, "CAKE");
    let excluded = bot.feed_health_for(Some(("market-1", &assets[1].ticker))).unwrap();
    assert!(excluded.conns.is_empty());
    assert_eq!(health.conns.len(), 1);
    assert_eq!(
        health.conns[0]
            .pairs
            .iter()
            .map(|p| p.ticker.as_str())
            .collect::<Vec<_>>(),
        ["CAKE", "RENDER"]
    );
    for asset in [&assets[1], &assets[3]] {
        let req = TradeRequest {
            venue: bot.id(),
            asset: asset.clone(),
            side: Side::Buy,
            qty: 1.into(),
            worst_price: 1.into(),
            deal_id: "catalog-regression".into(),
            route: None,
            exit_position: false,
            remeasure: None,
        };
        let reason = bot
            .symbols
            .get(&bot.ctx, &asset.ticker)
            .unwrap_err()
            .message;
        if asset.ticker == "RWA" {
            assert!(bot.symbols.symbol_of("RWA").is_none());
            assert_eq!(
                bot.symbols.state("RWA").unwrap().order_block.as_deref(),
                Some(reason.as_str())
            );
        }
        assert_eq!(bot.check_order(&req).await.unwrap_err().message, reason);
        assert_eq!(bot.trade(req.clone()).await.unwrap_err().message, reason);
        if asset.ticker == "rROL" {
            // Восстановление старого ордера всё ещё доходит до опроса по сохранённым symbol/orderId.
            // В окружении не задан интервал опроса, так что приватного сетевого вызова здесь нет.
            let pending = crate::gateway::pending::PendingRef::order("RROLUSDT", "old", req.qty)
                .received_coin("rROL")
                .with_ref("prior-order");
            assert!(
                bot.resolve_trade(req, &pending)
                    .await
                    .unwrap_err()
                    .message
                    .contains("venue.order_poll_ms")
            );
        }
        assert_eq!(
            bot.book_version(&asset.ticker).await.unwrap_err().message,
            reason
        );
        assert_eq!(
            bot.quote_rest(asset, Side::Buy)
                .await
                .err()
                .unwrap()
                .message,
            reason
        );
    }
    bot.symbols
        .publish(classic_list(&v2, &v3, "USDT").unwrap().0, Utc::now());
    bot.symbols.wait_books_change(&assigned, &current).await;
    assert!(bot.symbols.get(&bot.ctx, "RWA").unwrap().tradable());
    assert!(bot.symbols.get(&bot.ctx, "rROL").is_err());
}

/// У каждой временно неразмеченной пары своя дата. Обновление Z, новый symbol, дубль
/// или отсутствие настройки срока не могут продлить её допуск. В ядре остаётся причина блока.
#[test]
fn individual_class_lifetime_survives_partial_refresh_and_expires_without_delisting() {
    use crate::venues::bitget::classic_catalog::Stamp;
    use std::time::{Duration, Instant};
    let (v2, v3) = rows();
    let origin = Instant::now();
    let utc = Utc::now();
    let make_stamp = |seconds: u64| Stamp {
        instant: origin + Duration::from_secs(seconds),
        utc: utc + chrono::Duration::seconds(seconds as i64),
    };
    let limit = Some(Duration::from_secs(10));
    let directory = Symbols::default();
    let mut prior = v3.clone();
    for r in prior.as_array_mut().unwrap() {
        if r["symbol"] == "RROLUSDT" {
            r["isReality"] = json!("no");
        }
    }
    directory.publish_at(
        classic_list(&v2, &prior, "USDT").unwrap().0,
        make_stamp(0),
        limit,
    );
    let mut partial = v3.clone();
    let field = partial
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["symbol"] == "RWAUSDT")
        .unwrap();
    field.as_object_mut().unwrap().remove("isReality");
    directory.publish_at(
        classic_list(&v2, &partial, "USDT").unwrap().0,
        make_stamp(2),
        limit,
    );
    directory.publish_at(
        classic_list(&v2, &partial, "USDT").unwrap().0,
        make_stamp(8),
        limit,
    );
    let (catalog, at) = directory.dated_catalog().unwrap();
    assert_eq!(at, make_stamp(8).utc);
    assert!(catalog.rule("rROL", make_stamp(8).instant).is_none());
    let before = Snapshot::at(&catalog, &assets(), make_stamp(9).instant);
    let after = Snapshot::at(&catalog, &assets(), make_stamp(10).instant);
    assert_eq!(before.tickers, ["CAKE", "RENDER", "RWA"]);
    assert_eq!(after.tickers, ["CAKE", "RENDER"]);
    assert!(after.missing.is_empty());
    let blocked = after.pairs.iter().find(|p| p.ticker == "RWA").unwrap();
    assert!(
        blocked.online
            && blocked
                .order_block
                .as_deref()
                .unwrap()
                .contains("Последнее подтверждение Classic")
    );
    assert_eq!(
        catalog.expiry(make_stamp(9).instant),
        Some(make_stamp(10).instant)
    );
    assert!(catalog.rule("RENDER", make_stamp(11).instant).is_some());

    // У изменившегося symbol с прежним base нет подтверждения старого инструмента.
    let mut renamed = v2.clone();
    let rwa = renamed
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["baseCoin"] == "RWA")
        .unwrap();
    rwa["symbol"] = json!("changed-symbol");
    directory.publish_at(
        classic_list(&renamed, &partial, "USDT").unwrap().0,
        make_stamp(9),
        limit,
    );
    assert!(
        directory
            .dated_catalog()
            .unwrap()
            .0
            .rule("RWA", make_stamp(9).instant)
            .is_none()
    );

    directory.publish_at(
        classic_list(&v2, &v3, "USDT").unwrap().0,
        make_stamp(15),
        limit,
    );
    assert!(
        directory
            .dated_catalog()
            .unwrap()
            .0
            .rule("RWA", make_stamp(15).instant)
            .is_some()
    );
    directory.publish_at(
        classic_list(&v2, &partial, "USDT").unwrap().0,
        make_stamp(16),
        None,
    );
    assert!(
        directory
            .dated_catalog()
            .unwrap()
            .0
            .rule("RWA", make_stamp(16).instant)
            .is_none()
    );
}
