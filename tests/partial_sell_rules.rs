//! Полная подготовка лимитной продажи и локальный отбор на публичных ответах PIEVERSE_USDT.
//! Снимки и URL в data/poloniex_partial_2026-11-24.json. Размеры заявок — расчётные точки.

mod t198_common;

use std::time::Duration;

use arb::core::partial::{Choice, choose_quoted};
use arb::core::types::{Asset, Side, TradeRequest};
use arb::gateway::{contract::LocalSellRules, frozen::deepest, rest_book};
use arb::settings::value::Scope;
use arb::storage::raw_log::RawLog;
use arb::venues::poloniex::{book_print, ctx::Ctx, order_rules, symbols::Symbols, symbols_parse};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use t198_common::{d, start};

#[tokio::test]
async fn partial_sell_matches_preparation_on_recorded_pieverse() {
    let core = start("partial-sell-pieverse").await;
    let deps = &core.inner.deps;
    let ctx = Ctx {
        settings: deps.settings.clone(),
        vault: deps.notifier.vault.clone(),
        http: deps.notifier.http.clone(),
        journal: deps.journal.clone(),
        raw: RawLog::start(deps.data_dir.clone(), deps.settings.clone()),
    };
    for (key, value) in [
        ("venue.symbols_refresh_s", json!(60)),
        ("venue.order_time_in_force", json!("IOC")),
        ("venue.order_poll_ms", json!(200)),
        ("venue.order_final_wait_ms", json!(1000)),
    ] {
        ctx.settings
            .set(key, Scope::Venue(ctx.venue().0), Some(value), "test")
            .await
            .unwrap();
    }
    let data: Value =
        serde_json::from_str(include_str!("data/poloniex_partial_2026-11-24.json")).unwrap();
    let rules = symbols_parse::parse(&data["market"]).unwrap();
    let asset = Asset {
        ticker: rules.base.clone(),
        contract: None,
        chain: None,
    };
    let (bids, asks) = book_print::levels_of(&data["book"]).unwrap();
    let book = rest_book::book(ctx.venue(), asset.clone(), bids, asks, None);
    let best = book.bids[0].price;
    let symbols = Symbols::default();
    assert_eq!(
        order_rules::local_sell(&ctx, &symbols, &asset, d("0.32"), best, best),
        LocalSellRules::Unknown
    );
    symbols.load([(rules.base.clone(), rules)].into(), chrono::Utc::now());
    let request = |qty, worst_price| TradeRequest {
        venue: ctx.venue(),
        asset: asset.clone(),
        side: Side::Sell,
        qty,
        worst_price,
        deal_id: "pieverse-partial".into(),
        route: None,
        exit_position: true,
        remeasure: None,
    };
    let local = |qty, worst, top| order_rules::local_sell(&ctx, &symbols, &asset, qty, worst, top);
    // Обратная продажа дала более высокий расчётный итог. На свежих правилах PIEVERSE
    // 0.32 не проходит минимум, а для 0.33 сохраняется прежний выбор.
    let back = |qty| {
        let worst = deepest(&book, Side::Sell, qty).unwrap();
        Ok((qty * best, move || local(qty, worst, best)))
    };
    let deliver = d("0.31") * best;
    assert_eq!(
        choose_quoted(Ok(deliver), back(d("0.32"))),
        Choice::Deliver(deliver)
    );
    let unavailable = "нет маршрута вывода";
    let reason = order_rules::ready(&ctx, &symbols, &request(d("0.32"), best))
        .await
        .unwrap_err()
        .message;
    assert_eq!(
        choose_quoted(Err(unavailable.into()), back(d("0.32"))),
        Choice::Neither(format!("довезти: {unavailable}; продать на A: {reason}"))
    );
    assert_eq!(
        choose_quoted(Ok(deliver), back(d("0.33"))),
        Choice::SellBack(d("0.33") * best)
    );
    for raw in [d("0.32"), d("0.33"), d("32.701111")] {
        let worst = deepest(&book, Side::Sell, raw).unwrap();
        let actual = order_rules::ready(&ctx, &symbols, &request(raw, worst)).await;
        match actual {
            Ok(order) => {
                assert_ne!(raw, d("0.32"));
                assert_eq!(local(raw, worst, best), LocalSellRules::Passed);
                if raw > book.bids[0].qty {
                    assert_eq!(order.qty, book.bids[0].qty);
                    assert_eq!(order.price, book.bids[1].price, "глубина на исходный объём");
                }
            }
            Err(e) => {
                assert_eq!(raw, d("0.32"));
                assert_eq!(local(raw, worst, best), LocalSellRules::Rejected(e.message));
            }
        }
    }
    // Минимум из рыночного справочника: ровно 1 USDT проходит minAmount, но отклоняется правилом 21350.
    let one = Decimal::ONE;
    let e = order_rules::ready(&ctx, &symbols, &request(one, one))
        .await
        .unwrap_err();
    assert!(e.message.contains("21350"));
    assert_eq!(local(one, one, one), LocalSellRules::Rejected(e.message));
    assert_eq!(local(one, one, best), LocalSellRules::Unknown);
    let all = book.bids.iter().map(|level| level.qty).sum::<Decimal>();
    assert!(deepest(&book, Side::Sell, all + d("0.000001")).is_none());
    ctx.settings
        .set(
            "venue.symbols_refresh_s",
            Scope::Venue(ctx.venue().0),
            Some(json!(1)),
            "test",
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1010)).await;
    assert_eq!(local(d("0.32"), best, best), LocalSellRules::Unknown);
    assert_eq!(
        choose_quoted(Ok(deliver), back(d("0.32"))),
        Choice::SellBack(d("0.32") * best)
    );
}
