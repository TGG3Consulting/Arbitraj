//! Файл: tests/bingx_minqty_293.rs
//! Что делает: решение 293 — настройка venue.ignore_deprecated_qty [bingx] на записанных публичных ответах BingX
//! 15.11.2026 (tests/data/bingx_minqty_2026-11-15: «Spot trading symbols» и bookTicker; как записано — README папки):
//! 1) справочник пар SHOPINX-USDT через тот же путь, что Symbols::refresh (symbols_qty::take): настройка не задана или
//!    «нет» — min_qty 57505, max_qty 575043128.24 (как раньше); «да» — пределов количества нет, minNotional 1 и
//!    maxNotional 1029 на месте; то же видит ядро (SpotPairState);
//! 2) правила ордера (order_rules::ready — общие у ордера и check_order; market_sell::ready): продажа 11 000 SHOPINX по
//!    записанному bid 0.00870 — без настройки отказ «minQty», с настройкой проходит; сумма сверх maxNotional 1029 —
//!    отказ и с настройкой;
//! 3) core/signal_probe::floor_qty / qty — объём проверки при signal.probe_usd = 10 и потолке = maxNotional пары (1029):
//!    без настройки 57 505 монет, с настройкой 10 / цена. Минимумы вывода / депозита SHOPINX не учтены: записанного
//!    ответа capital/config/getall по SHOPINX нет (подписанный запрос; отчёт решения 293);
//! 4) факт выборки: у скольких из 9 пар minQty × bid больше minNotional (7);
//! 5) ревью 293: значение монеты сильнее значения площадки (все 9 пар одним справочником); «да → нет» — пределы
//!    вернулись; смена видна в журнале (pairs.rs changes, only_qty_changed).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bingx (symbols, symbols_qty, order_rules, market_sell, ctx), arb::core::signal_probe,
//! arb::settings, arb::storage, arb::secrets.

use std::str::FromStr;

use arb::core::signal_probe::{floor_qty, qty};
use arb::core::types::{Asset, Side, TradeRequest, VenueId};
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bingx::ctx::Ctx;
use arb::venues::bingx::symbols::{Symbols, parse_list};
use arb::venues::bingx::{market_sell, order_rules, symbols_qty};
use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

const DIR: &str = "tests/data/bingx_minqty_2026-11-15";
/// Пары выборки (файлы symbols_<ПАРА>.json и book_ticker_<ПАРА>.json).
const PAIRS: [&str; 9] = [
    "SHOPINX-USDT",
    "CAKE-USDT",
    "TWT-USDT",
    "SFP-USDT",
    "BTC-USDT",
    "DOGE-USDT",
    "BNB-USDT",
    "MUBARAK-USDT",
    "BANANAS31-USDT",
];
/// Время запроса symbols (параметр timestamp записанного запроса, README папки).
const SYMBOLS_AT_MS: i64 = 1_794_742_830_000;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn read(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(format!("{DIR}/{name}")).unwrap()).unwrap()
}

/// Лучший bid записанного bookTicker пары.
fn bid(pair: &str) -> Decimal {
    let v = read(&format!("book_ticker_{pair}.json"));
    d(v["data"][0]["bidPrice"].as_str().unwrap())
}

/// Окружение адаптера BingX на SQLite во временной папке (без сети и ключей). Принимает: имя папки.
async fn ctx(tag: &str) -> Ctx {
    let dir = std::env::temp_dir().join(format!("arb-bingx-293-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    // Настройки окружения теста: котируемая монета и срок ордера (IOC — у BingX по документации place order).
    set(&settings, "venue.quote_asset", Some(json!("USDT"))).await;
    set(&settings, "venue.order_time_in_force", Some(json!("IOC"))).await;
    Ctx {
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(dir.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
        raw: RawLog::start(dir.join("raw"), settings),
        taps: Default::default(),
    }
}

/// Задать или снять настройку области «bingx».
async fn set(s: &Store, name: &str, v: Option<Value>) {
    s.set(name, Scope::Venue("bingx".into()), v, "тест")
        .await
        .unwrap();
}

/// Задать venue.ignore_deprecated_qty у монеты (область «монета», ревью 293).
async fn set_coin(s: &Store, ticker: &str, v: bool) {
    let sc = Scope::Coin(ticker.into());
    s.set(symbols_qty::SETTING, sc, Some(json!(v)), "тест")
        .await
        .unwrap();
}

/// Справочник всех 9 пар: массивы symbols записанных ответов подряд (одним ответом), тем же путём, что refresh.
fn load_all(c: &Ctx) -> Symbols {
    let mut all = vec![];
    for pair in PAIRS {
        let v = read(&format!("symbols_{pair}.json"));
        all.extend(v["data"]["symbols"].as_array().unwrap().iter().cloned());
    }
    let s = Symbols::default();
    let at = Utc.timestamp_millis_opt(SYMBOLS_AT_MS).unwrap();
    let data = json!({ "symbols": all });
    assert_eq!(symbols_qty::take(c, &s, &data, at).unwrap(), PAIRS.len());
    s
}

/// Есть ли у пары монеты пределы количества (minQty и maxQty).
fn has_qty(s: &Symbols, c: &Ctx, ticker: &str) -> bool {
    let r = s.get(c, ticker).unwrap();
    r.min_qty.is_some() && r.max_qty.is_some()
}

/// Справочник пар из записанного ответа SHOPINX тем же путём, что Symbols::refresh.
fn load(c: &Ctx) -> Symbols {
    let s = Symbols::default();
    let v = read("symbols_SHOPINX-USDT.json");
    let at = Utc.timestamp_millis_opt(SYMBOLS_AT_MS).unwrap();
    assert_eq!(symbols_qty::take(c, &s, &v["data"], at).unwrap(), 1);
    s
}

/// Продажа SHOPINX на BingX по записанному bid.
fn sell(q: &str) -> TradeRequest {
    TradeRequest {
        venue: VenueId("bingx".into()),
        asset: Asset {
            ticker: "SHOPINX".into(),
            bsc_contract: None,
        },
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
    // Не задана — как раньше.
    let r = load(&c).get(&c, "SHOPINX").unwrap();
    assert_eq!(r.min_qty, Some(d("57505")));
    assert_eq!(r.max_qty, Some(d("575043128.24")));
    // «Нет» — тоже как раньше.
    set(&c.settings, symbols_qty::SETTING, Some(json!(false))).await;
    assert_eq!(
        load(&c).get(&c, "SHOPINX").unwrap().min_qty,
        Some(d("57505"))
    );
    // «Да» — пределов количества нет, сумма и шаги на месте; ядро видит то же.
    set(&c.settings, symbols_qty::SETTING, Some(json!(true))).await;
    let s = load(&c);
    let r = s.get(&c, "SHOPINX").unwrap();
    assert_eq!((r.min_qty, r.max_qty), (None, None));
    assert_eq!(r.min_notional, Some(d("1")));
    assert_eq!(r.max_notional, Some(d("1029")));
    assert_eq!(r.step_size, d("0.000001"));
    assert!(r.tradable(false) && r.tradable(true));
    let p = s.state("SHOPINX").unwrap();
    assert_eq!((p.min_qty, p.max_qty), (None, None));
    assert_eq!(p.min_notional, Some(d("1")));
    // Чистый разбор (parse_list) не меняется: настройка действует только в symbols_qty::take.
    let raw = parse_list(&read("symbols_SHOPINX-USDT.json")["data"], "USDT").unwrap();
    assert_eq!(raw["SHOPINX"].min_qty, Some(d("57505")));
}

#[tokio::test]
async fn order_rules_sell_11000_shopinx() {
    let c = ctx("ord").await;
    let req = sell("11000");
    // Не задана: отказ по minQty — у лимитного ордера (он же check_order) и у рыночной продажи.
    let s = load(&c);
    let e = order_rules::ready(&c, &s, &req).unwrap_err();
    assert!(e.message.contains("minQty 57505"), "{}", e.message);
    let e = market_sell::ready(&c, &s, &req).unwrap_err();
    assert!(e.message.contains("minQty"), "{}", e.message);
    // Да: проходит; количество и цена — по шагам пары.
    set(&c.settings, symbols_qty::SETTING, Some(json!(true))).await;
    let s = load(&c);
    let p = order_rules::ready(&c, &s, &req).unwrap();
    assert_eq!((p.qty, p.price), (d("11000"), d("0.0087")));
    assert!(market_sell::ready(&c, &s, &req).is_ok());
    // Сумма сверх maxNotional 1029 (200 000 × 0.0087 = 1740) — отказ и с настройкой.
    let e = order_rules::ready(&c, &s, &sell("200000")).unwrap_err();
    assert!(e.message.contains("maxNotional"), "{}", e.message);
    // Сумма меньше minNotional 1 (100 × 0.0087 = 0.87) — отказ и с настройкой.
    let e = order_rules::ready(&c, &s, &sell("100")).unwrap_err();
    assert!(e.message.contains("minNotional"), "{}", e.message);
}

#[tokio::test]
async fn signal_probe_volume_shopinx() {
    let c = ctx("probe").await;
    let price = bid("SHOPINX-USDT");
    let (probe, cap) = (d("10"), d("1029"));
    // DEX → BingX: пара есть только у стороны B (BingX); минимумы вывода / депозита не записаны — None.
    let before = load(&c).state("SHOPINX").unwrap();
    let f = floor_qty([None, Some(&before)], None, None, price);
    assert_eq!(f, d("57505"));
    assert_eq!(qty(cap, Some(probe), price, f), d("57505"));
    set(&c.settings, symbols_qty::SETTING, Some(json!(true))).await;
    let after = load(&c).state("SHOPINX").unwrap();
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

#[tokio::test]
async fn coin_value_beats_venue_value() {
    let c = ctx("coin").await;
    let others = [
        "CAKE",
        "BNB",
        "TWT",
        "SFP",
        "BTC",
        "DOGE",
        "MUBARAK",
        "BANANAS31",
    ];
    // «Да» только у SHOPINX — пределы сняты только у неё.
    set_coin(&c.settings, "SHOPINX", true).await;
    let s = load_all(&c);
    assert!(!has_qty(&s, &c, "SHOPINX"));
    assert!(others.iter().all(|t| has_qty(&s, &c, t)));
    // «Да» у площадки и «нет» у SHOPINX — у SHOPINX пределы остались, у остальных сняты.
    set(&c.settings, symbols_qty::SETTING, Some(json!(true))).await;
    set_coin(&c.settings, "SHOPINX", false).await;
    let s = load_all(&c);
    assert!(has_qty(&s, &c, "SHOPINX"));
    assert!(others.iter().all(|t| !has_qty(&s, &c, t)));
}

#[tokio::test]
async fn yes_then_no_returns_limits_and_journal_text() {
    use arb::venues::bingx::pairs::Snapshot;
    let c = ctx("back").await;
    let assets = [Asset {
        ticker: "SHOPINX".into(),
        bsc_contract: None,
    }];
    let before = Snapshot::build(&load(&c), &assets);
    set(&c.settings, symbols_qty::SETTING, Some(json!(true))).await;
    let on = Snapshot::build(&load(&c), &assets);
    // Смена видна в журнале venue.symbols и отличается только пределами количества.
    assert!(on.only_qty_changed(&before));
    assert_eq!(
        on.changes(&before),
        vec![
            "SHOPINX: API: покупка да, продажа да → API: покупка да, продажа да, пределов количества нет"
        ]
    );
    // «Да → нет» — пределы вернулись со следующим чтением справочника.
    set(&c.settings, symbols_qty::SETTING, Some(json!(false))).await;
    let s = load(&c);
    let r = s.get(&c, "SHOPINX").unwrap();
    assert_eq!(
        (r.min_qty, r.max_qty),
        (Some(d("57505")), Some(d("575043128.24")))
    );
    let off = Snapshot::build(&s, &assets);
    assert_eq!(off, before);
    assert!(!off.only_qty_changed(&before));
}
