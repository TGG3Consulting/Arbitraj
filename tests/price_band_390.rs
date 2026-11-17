//! Файл: tests/price_band_390.rs
//! Что проверяет: решение 392 (задача t180) — ценовой коридор лимитного ордера биржи.
//! - Правила коридора из справочников пар — на ЖИВЫХ записях: MEXC exchangeInfo 2026-11-02
//!   (tests/data/mexc_2026-11-02/exchange_info.json: CAKEUSDT PERCENT_PRICE_BY_SIDE bidMultiplierUp 0.2,
//!   askMultiplierDown 0.2), XT /v4/public/symbol 06.11.2026 (tests/data/xt_live_2026-11-06/symbol_pepe.json:
//!   PROTECTION_LIMIT 0.8 / 0.8 / 4 / 4).
//! - Опорные цены и границы, которых в записях НЕТ, — на ПРИМЕРАХ ОФИЦИАЛЬНОЙ ДОКУМЕНТАЦИИ (помечено у каждого теста
//!   «из документации, не запись»; в docs/TASKS.md — «нужна запись»): MEXC ticker/price, XT ticker/price, Binance
//!   PERCENT_PRICE_BY_SIDE / referencePrice / avgPrice, Bybit price-limit, Poloniex highestBid; OKX price-limit — поля
//!   документации SWAP с instType SPOT (не пример спота). XT и OKX ядру коридор не отдают (ревью t180 Б3, Н5).
//! - Учёт в расчёте (core/price_band.rs check): худшая цена за коридором «дальше» — TooLarge (подбор берёт меньший
//!   объём), «не с той стороны» — Stop; на ЗАПИСАННОМ стакане Gate CAKE_USDT (cex_samples_2026_11_04) с правилом MEXC
//!   CAKE из записи и опорной ценой — параметром теста (пример «на пальцах»: стакан и правило — записи разных бирж).
//!
//! Кто вызывает: cargo test. От чего зависит: arb::gateway::price_band, arb::core::price_band, адаптеры mexc, xt,
//! binance, bybit, okx, poloniex. Моков нет — чистые функции.

use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::{Value, json};

use arb::core::size::Fail;
use arb::core::types::{Asset, OrderBook, PriceLevel, Side, VenueId};
use arb::gateway::price_band::{Miss, PriceBand};

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn file(path: &str) -> Value {
    let p = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).expect("файл записи")).expect("JSON")
}

/// MEXC: правило из живого exchangeInfo — покупка ≤ last × 1.2, продажа ≥ last × 0.8; в справочник ядра оно уходит.
#[test]
fn mexc_rule_from_recorded_exchange_info() {
    let info = file("tests/data/mexc_2026-11-02/exchange_info.json");
    let cake = info["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["symbol"] == "CAKEUSDT")
        .unwrap();
    let rule = arb::venues::mexc::symbols_parse::price_band(cake)
        .unwrap()
        .expect("фильтр PERCENT_PRICE_BY_SIDE есть");
    assert_eq!(rule.buy_up, Some(d("1.2")));
    assert_eq!(rule.sell_down, Some(d("0.8")));
    assert_eq!((rule.buy_down, rule.sell_up), (None, None));
    let allowed = ["CAKEUSDT".to_string()].into();
    let map = arb::venues::mexc::symbols::parse_list(&info, "USDT", &allowed).unwrap();
    assert_eq!(map["CAKE"].price_band.as_ref(), Some(&rule));
    // Пример ответа Symbol Price Ticker из документации MEXC (из документации, не запись): price «Last price».
    let last =
        arb::venues::mexc::price_band::last_price(&json!({"symbol": "BTCUSDT", "price": "184.34"}))
            .unwrap();
    let b = rule.at(Some(last), String::new());
    assert_eq!(b.buy_max, Some(d("221.208")));
    assert_eq!(b.sell_min, Some(d("147.472")));
    assert!(arb::venues::mexc::price_band::last_price(&json!({"symbol": "X"})).is_err());
}

/// XT: правило PROTECTION_LIMIT из живого ответа разбирается (данные); последняя цена — пример документации (не запись);
/// ядру коридор не отдаётся (ревью Б3).
#[test]
fn xt_rule_from_recorded_symbols() {
    let v = file("tests/data/xt_live_2026-11-06/symbol_pepe.json");
    let map = arb::venues::xt::symbols_parse::parse_list(&v, "USDT")
        .unwrap()
        .map
        .unwrap();
    let rule = map["BTC"].price_band.clone().expect("PROTECTION_LIMIT");
    // buy: price ≥ last − last·0.8, price ≤ last + last·4; sell: price ≤ last + last·4, price ≥ last − last·0.8.
    assert_eq!(rule.buy_up, Some(d("5")));
    assert_eq!(rule.buy_down, Some(d("0.2")));
    assert_eq!(rule.sell_up, Some(d("5")));
    assert_eq!(rule.sell_down, Some(d("0.2")));
    // Пример ответа Get latest prices ticker из документации XT (из документации, не запись).
    let r = json!([{"s": "btc_usdt", "t": 1661856036925_i64, "p": "9000.0000"}]);
    let last = arb::venues::xt::price_band::latest(&r, "btc_usdt")
        .unwrap()
        .unwrap();
    let b = rule.at(Some(last), String::new());
    assert_eq!(b.buy_max, Some(d("45000")));
    assert_eq!(b.buy_min, Some(d("1800")));
    // Пары в ответе нет — последней цены нет: по документации ограничения нет.
    assert_eq!(
        arb::venues::xt::price_band::latest(&r, "eth_usdt").unwrap(),
        None
    );
    // Ревью t180, Б3: опорная цена XT документацией не подтверждена — правило ядру не отдаётся (как до решения 392).
    let syms = arb::venues::xt::symbols::Symbols::default();
    syms.load(map, chrono::Utc::now());
    let state = syms.state("BTC", None).expect("пара BTC");
    assert!(state.price_band.is_none());
}

/// Binance: фильтры из примеров filters.md (из документации, не запись); опорная цена — примеры rest-api.md.
#[test]
fn binance_filters_and_reference_from_docs() {
    let filters = json!([
        {"filterType": "PERCENT_PRICE", "multiplierUp": "1.3000", "multiplierDown": "0.7000", "avgPriceMins": 5},
        {"filterType": "PERCENT_PRICE_BY_SIDE", "bidMultiplierUp": "1.2", "bidMultiplierDown": "0.2",
         "askMultiplierUp": "5", "askMultiplierDown": "0.8", "avgPriceMins": 1}
    ]);
    let find = |k: &str| {
        filters
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["filterType"] == k)
    };
    let list = arb::venues::binance::price_band::filters(find).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].rule.buy_up, Some(d("1.2")));
    assert_eq!(list[0].avg_mins, 1);
    assert_eq!(list[1].rule.sell_down, Some(d("0.7000")));
    use arb::venues::binance::price_band::{avg_price, reference_price};
    assert_eq!(
        reference_price(
            &json!({"symbol": "BAZUSD", "referencePrice": "10.00", "timestamp": 1770736694138_i64})
        )
        .unwrap(),
        Some(d("10.00"))
    );
    assert_eq!(
        reference_price(
            &json!({"symbol": "BAZUSD", "referencePrice": null, "timestamp": 1770736694138_i64})
        )
        .unwrap(),
        None
    );
    let avg = json!({"mins": 5, "price": "9.35751834", "closeTime": 1694061154503_i64});
    assert_eq!(avg_price(&avg, 5).unwrap(), d("9.35751834"));
    // Средняя за другой срок, чем у фильтра, — не та средняя: ошибка, а не догадка.
    assert!(avg_price(&avg, 1).is_err());
}

/// Bybit и OKX: границы считает биржа. Bybit — пример ответа документации (не запись); OKX — см. пометку ниже.
#[test]
fn bybit_okx_limits_from_docs() {
    let r = json!({"symbol": "BTCUSDT", "buyLmt": "105878.10", "sellLmt": "103781.60", "ts": "1750302284491"});
    let b = arb::venues::bybit::price_band::parse(&r, "BTCUSDT").unwrap();
    assert_eq!(b.buy_max, Some(d("105878.10")));
    assert_eq!(b.sell_min, Some(d("103781.60")));
    // OKX — НЕ запись и не пример документации для спота: поля — из таблицы раздела «Get limit price» (пример раздела —
    // SWAP, instId BTC-USDT-SWAP), instType заменён на SPOT; что эндпоинт отдаёт SPOT, подтверждено только ссылкой из
    // раздела «Get instruments». Поэтому у OKX коридор ядру не отдаётся (ревью Н5); нужна запись — docs/TASKS.md.
    let on = json!({"instType": "SPOT", "instId": "BTC-USDT", "buyLmt": "200", "sellLmt": "100", "ts": "1", "enabled": true});
    let b = arb::venues::okx::price_band::parse(&on, "BTC-USDT")
        .unwrap()
        .unwrap();
    assert_eq!((b.buy_max, b.sell_min), (Some(d("200")), Some(d("100"))));
    let off = json!({"instType": "SPOT", "instId": "BTC-USDT", "buyLmt": "", "sellLmt": "", "ts": "1", "enabled": false});
    assert_eq!(
        arb::venues::okx::price_band::parse(&off, "BTC-USDT").unwrap(),
        None
    );
}

/// Poloniex: highestBid / lowestAsk — постоянные границы; «0» (как в примере документации) — границы нет.
/// Строка собрана по полям документации Symbol Information (не запись).
#[test]
fn poloniex_fixed_bounds_from_doc_fields() {
    let row = |hb: &str, la: &str| {
        json!([{"symbol": "CAKE_USDT", "baseCurrencyName": "CAKE", "quoteCurrencyName": "USDT", "state": "NORMAL",
            "symbolTradeLimit": {"symbol": "CAKE_USDT", "priceScale": 4, "quantityScale": 2, "amountScale": 4,
            "minQuantity": "0", "minAmount": "1", "maxQuantity": "0", "maxAmount": "0",
            "highestBid": hb, "lowestAsk": la}}])
    };
    let (m, _) = arb::venues::poloniex::symbols::parse_list(&row("0", "0"), "USDT").unwrap();
    assert!(m["CAKE"].price_band().is_none());
    let (m, _) = arb::venues::poloniex::symbols::parse_list(&row("3", "1"), "USDT").unwrap();
    let b = m["CAKE"].price_band().unwrap().at(None, String::new());
    assert_eq!((b.buy_max, b.sell_min), (Some(d("3")), Some(d("1"))));
}

/// Где цена относительно коридора и как это видит подбор объёма (чистая логика).
#[test]
fn miss_and_check_kinds() {
    let band = PriceBand {
        buy_max: Some(d("1.2")),
        buy_min: Some(d("0.2")),
        sell_max: Some(d("5")),
        sell_min: Some(d("0.8")),
        note: "тест".into(),
    };
    assert_eq!(band.miss(Side::Buy, d("1.2")), None, "граница включительно");
    assert_eq!(band.miss(Side::Buy, d("1.21")), Some(Miss::Far(d("1.2"))));
    assert_eq!(band.miss(Side::Buy, d("0.1")), Some(Miss::Wrong(d("0.2"))));
    assert_eq!(band.miss(Side::Sell, d("0.7")), Some(Miss::Far(d("0.8"))));
    assert_eq!(band.miss(Side::Sell, d("6")), Some(Miss::Wrong(d("5"))));
    use arb::core::price_band::check;
    assert!(
        check("mexc", None, Side::Buy, d("100")).is_ok(),
        "нет коридора — как раньше"
    );
    assert!(matches!(
        check("mexc", Some(&band), Side::Buy, d("2")),
        Err(Fail::TooLarge(_))
    ));
    assert!(matches!(
        check("mexc", Some(&band), Side::Buy, d("0.1")),
        Err(Fail::Stop(_))
    ));
    assert!(matches!(
        check("mexc", Some(&band), Side::Sell, d("0.5")),
        Err(Fail::TooLarge(_))
    ));
    // Множитель 1 − x при x ≥ 1 даёт границу ≤ 0 — границы с этой стороны нет.
    let rule = arb::gateway::price_band::PriceBandRule {
        sell_down: Some(d("-1")),
        ..Default::default()
    };
    assert_eq!(rule.at(Some(d("10")), String::new()).sell_min, None);
}

/// «На пальцах» на записанном стакане Gate CAKE_USDT (asks: 1.896 × 587.88, 1.8962 × 1.7, …) и правиле MEXC CAKE из
/// записи (покупка ≤ last × 1.2); опорная цена 1.58 — параметр теста: граница 1.896. Объём 500 — худшая цена 1.896,
/// в коридоре; объём 589 — худшая 1.8962, за коридором → TooLarge: подбор возьмёт меньший объём, а не потеряет сделку.
#[test]
fn deep_buy_beyond_band_shrinks_not_kills() {
    let raw = file("tests/data/cex_samples_2026_11_04/gate_spot_order_book_CAKE_USDT.json");
    let lv = |side: &str| -> Vec<PriceLevel> {
        raw[side]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| PriceLevel {
                price: d(l[0].as_str().unwrap()),
                qty: d(l[1].as_str().unwrap()),
            })
            .collect()
    };
    let book = OrderBook {
        venue: VenueId("gate".into()),
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        bids: lv("bids"),
        asks: lv("asks"),
        exchange_time: None,
        received_at: chrono::Utc::now(),
    };
    let info = file("tests/data/mexc_2026-11-02/exchange_info.json");
    let cake = &info["symbols"][0];
    assert_eq!(cake["symbol"], "CAKEUSDT");
    let rule = arb::venues::mexc::symbols_parse::price_band(cake)
        .unwrap()
        .unwrap();
    let band = rule.at(Some(d("1.58")), "опорная — параметр теста".into());
    assert_eq!(band.buy_max, Some(d("1.896")));
    use arb::core::price_band::check;
    use arb::gateway::frozen::deepest;
    let w500 = deepest(&book, Side::Buy, d("500")).unwrap();
    assert_eq!(w500, d("1.896"));
    assert!(check("mexc", Some(&band), Side::Buy, w500).is_ok());
    let w589 = deepest(&book, Side::Buy, d("589")).unwrap();
    assert_eq!(w589, d("1.8962"));
    assert!(matches!(
        check("mexc", Some(&band), Side::Buy, w589),
        Err(Fail::TooLarge(_))
    ));
}
