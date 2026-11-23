//! Файл: tests/bingx_minqty_293.rs
//! Что делает: решение 293 — настройка venue.ignore_deprecated_qty [bingx] (значение площадки) на записанных публичных
//! ответах BingX 15.11.2026 (tests/data/bingx_minqty_2026-11-15: «Spot trading symbols» и bookTicker; как записано —
//! README папки, это пересказ WebFetch — перезаписать сырыми, commands.txt):
//! 1) справочник SHOPINX-USDT тем же путём, что Symbols::refresh (symbols_qty::take): не задана или «нет» — min_qty
//!    57505, max_qty 575043128.24 (как раньше); «да» — у пары пределов количества нет (get_for — правила ордера,
//!    state_for — ядро), minNotional 1 и maxNotional 1029 на месте; справочник хранится как ответил BingX;
//! 2) правила ордера (order_rules::ready — общие у ордера и check_order; market_sell::ready): продажа 11 000 SHOPINX по
//!    записанному bid 0.00870 — без настройки отказ «minQty», с настройкой проходит; сумма сверх maxNotional 1029 и
//!    меньше minNotional 1 — отказ и с настройкой;
//! 3) core/signal_probe::floor_qty / qty — объём проверки при signal.probe_usd = 10 и потолке = maxNotional пары (1029):
//!    без настройки 57 505 монет, с настройкой 10 / цена. Минимумы вывода / депозита SHOPINX не учтены: записанного
//!    ответа capital/config/getall по SHOPINX нет (подписанный запрос; docs/TASKS.md);
//! 4) факт выборки: у скольких из 9 пар minQty × bid больше minNotional (7).
//!
//! Значение у монеты, «да → нет», журнал, неверное значение — tests/bingx_minqty_coin_293.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/bingx_293_common, arb::venues::bingx (symbols, symbols_qty, order_rules, market_sell),
//! arb::core::signal_probe.

mod bingx_293_common;

use arb::core::signal_probe::{floor_qty, qty};
use arb::core::types::{Side, TradeRequest, VenueId};
use arb::gateway::contract::LocalSellRules;
use arb::venues::bingx::symbols::parse_list;
use arb::venues::bingx::{market_sell, order_rules, symbols_qty};
use bingx_293_common::{PAIRS, bid, ctx, d, load, read, set, shopinx};
use serde_json::json;

/// Продажа SHOPINX на BingX по записанному bid.
fn sell(q: &str) -> TradeRequest {
    TradeRequest {
        venue: VenueId("bingx".into()),
        asset: shopinx(),
        side: Side::Sell,
        qty: d(q),
        worst_price: bid("SHOPINX-USDT"),
        deal_id: "t293".into(),
        route: None,
        exit_position: true,
        remeasure: None,
    }
}

#[tokio::test]
async fn symbols_without_and_with_setting() {
    let c = ctx("sym").await;
    let a = shopinx();
    // Не задана — как раньше.
    let r = load(&c).get_for(&c, &a).unwrap();
    assert_eq!(r.min_qty, Some(d("57505")));
    assert_eq!(r.max_qty, Some(d("575043128.24")));
    // «Нет» — тоже как раньше.
    set(&c.settings, symbols_qty::SETTING, Some(json!(false))).await;
    assert_eq!(load(&c).get_for(&c, &a).unwrap().min_qty, Some(d("57505")));
    // «Да» у площадки — у пары пределов количества нет, сумма и шаги на месте; ядро видит то же.
    set(&c.settings, symbols_qty::SETTING, Some(json!(true))).await;
    let s = load(&c);
    let r = s.get_for(&c, &a).unwrap();
    assert_eq!((r.min_qty, r.max_qty), (None, None));
    assert_eq!(r.min_notional, Some(d("1")));
    assert_eq!(r.max_notional, Some(d("1029")));
    assert_eq!(r.step_size, d("0.000001"));
    assert!(r.tradable(false) && r.tradable(true));
    let p = s.state_for(&a).unwrap();
    assert_eq!((p.min_qty, p.max_qty), (None, None));
    assert_eq!(p.min_notional, Some(d("1")));
    // Справочник хранится как ответил BingX: пределы снимаются только при чтении по монете.
    assert_eq!(s.get(&c, "SHOPINX").unwrap().min_qty, Some(d("57505")));
    let raw = parse_list(&read("symbols_SHOPINX-USDT.json")["data"], "USDT").unwrap();
    assert_eq!(raw["SHOPINX"].min_qty, Some(d("57505")));
}

#[tokio::test]
async fn order_rules_sell_11000_shopinx() {
    let c = ctx("ord").await;
    let req = sell("11000");
    set(&c.settings, "venue.symbols_refresh_s", Some(json!(60))).await;
    // Не задана: отказ по minQty — у лимитного ордера (он же check_order) и у рыночной продажи.
    let s = load(&c);
    let e = order_rules::ready(&c, &s, &req).unwrap_err();
    assert!(e.message.contains("minQty 57505"), "{}", e.message);
    let local =
        |qty| order_rules::local_sell(&c, &s, &req.asset, qty, req.worst_price, req.worst_price);
    assert_eq!(local(req.qty), LocalSellRules::Rejected(e.message.clone()));
    let e = market_sell::ready(&c, &s, &req).unwrap_err();
    assert!(e.message.contains("minQty"), "{}", e.message);
    // Да: проходит; количество и цена — по шагам пары.
    set(&c.settings, symbols_qty::SETTING, Some(json!(true))).await;
    let p = order_rules::ready(&c, &s, &req).unwrap();
    assert_eq!(local(req.qty), LocalSellRules::Passed);
    assert_eq!((p.qty, p.price), (d("11000"), d("0.0087")));
    assert!(market_sell::ready(&c, &s, &req).is_ok());
    // Сумма сверх maxNotional 1029 (200 000 × 0.0087 = 1740) — отказ и с настройкой.
    let e = order_rules::ready(&c, &s, &sell("200000")).unwrap_err();
    assert!(e.message.contains("maxNotional"), "{}", e.message);
    assert_eq!(
        local(d("200000")),
        LocalSellRules::Unknown,
        "баланс может уменьшить долю"
    );
    // Сумма меньше minNotional 1 (100 × 0.0087 = 0.87) — отказ и с настройкой.
    let e = order_rules::ready(&c, &s, &sell("100")).unwrap_err();
    assert!(e.message.contains("minNotional"), "{}", e.message);
    assert_eq!(local(d("100")), LocalSellRules::Rejected(e.message));
}

#[tokio::test]
async fn signal_probe_volume_shopinx() {
    let c = ctx("probe").await;
    let a = shopinx();
    let price = bid("SHOPINX-USDT");
    let (probe, cap) = (d("10"), d("1029"));
    // DEX → BingX: пара есть только у стороны B (BingX); минимумы вывода / депозита не записаны — None.
    let s = load(&c);
    let before = s.state_for(&a).unwrap();
    let f = floor_qty([None, Some(&before)], None, None, price);
    assert_eq!(f, d("57505"));
    assert_eq!(qty(cap, Some(probe), price, f), d("57505"));
    set(&c.settings, symbols_qty::SETTING, Some(json!(true))).await;
    let after = s.state_for(&a).unwrap();
    let f = floor_qty([None, Some(&after)], None, None, price);
    assert_eq!(f, d("1") / price);
    assert_eq!(qty(cap, Some(probe), price, f), probe / price);
}

#[test]
fn sample_min_qty_above_min_notional() {
    let mut above = vec![];
    for pair in PAIRS {
        let v = read(&format!("symbols_{pair}.json"));
        let r = &parse_list(&v["data"], "USDT").unwrap()[pair.trim_end_matches("-USDT")];
        if r.min_qty.unwrap() * bid(pair) > r.min_notional.unwrap() {
            above.push(pair);
        }
    }
    assert_eq!(above.len(), 7, "{above:?}");
    assert!(!above.contains(&"MUBARAK-USDT") && !above.contains(&"BANANAS31-USDT"));
}
