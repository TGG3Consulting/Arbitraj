//! Файл: tests/order_probe_412.rs
//! Что проверяет: решение 412 (Тигран 19.11) — пробный ордер (venue.order_test_first) не делается перед настоящим
//! ордером — ни перед покупкой, ни перед продажей; остаётся только в проверке ордера без исполнения (контракт
//! check_order — предпроверка продажи на бирже B до покупки, решение 134).
//! - MEXC, поведение на записанном exchangeInfo 2026-11-02 (tests/data/mexc_2026-11-02/exchange_info.json, пара
//!   CAKEUSDT) и настоящем адаптере без сети: хранилище секретов заперто, поэтому любой подписанный запрос
//!   останавливается на ключах (SecretsLocked) ДО сети. Пробный ордер, если бы он шёл, дал бы ошибку с текстом
//!   «пробный ордер отклонён» (order_rules::test). При venue.order_test_first = да:
//!   подготовка настоящего ордера (order_rules::ready) — покупка и продажа проходят без запроса; настоящий ордер
//!   (trade::trade) — покупка и продажа падают на постановке POST /api/v3/order, текста пробного нет; проверка без
//!   исполнения (order_rules::check) — пробный идёт (покупка и продажа); при «нет» — проверка проходит без запроса.
//! - Binance, KuCoin, Kraken — по исходникам (записанного справочника пар этих бирж в tests/data нет): ready и
//!   trade.rs пробного не делают, check_order — через проверку с пробным.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::mexc (ctx, symbols, order_rules, trade), arb::settings, arb::storage. Моков нет.

use std::collections::HashSet;
use std::str::FromStr;

use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{Asset, Side, TradeRequest, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::mexc::{ctx::Ctx, order_rules, symbols::Symbols, trade};

/// Текст ошибки пробного ордера (order_rules::test у MEXC).
const PROBE: &str = "пробный ордер отклонён";

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Окружение адаптера MEXC на SQLite во временной папке; секреты заперты (запросы до сети не доходят).
/// Принимает: имя папки теста, значение venue.order_test_first. Ошибок нет (паника — сбой теста).
async fn start(name: &str, test_first: bool) -> Ctx {
    let dir = std::env::temp_dir().join(format!("arb-mexc-412-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = arb::settings::store::Store::load(db, changes, journal.clone())
        .await
        .unwrap();
    let m = |k: &str, v: Value| (k.to_string(), Scope::Venue("mexc".into()), v);
    for (k, sc, v) in [
        m("venue.order_type", json!("IMMEDIATE_OR_CANCEL")),
        m("venue.order_test_first", json!(test_first)),
        m("venue.order_poll_ms", json!(50)),
        m("venue.order_final_wait_ms", json!(300)),
        m("venue.my_trades_limit", json!(1000)),
    ] {
        settings.set(&k, sc, Some(v), "тест").await.unwrap();
    }
    Ctx {
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(dir.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
        raw: RawLog::start(dir.join("raw"), settings),
        taps: Default::default(),
    }
}

/// Справочник пар MEXC из записанного exchangeInfo; ключу разрешена CAKEUSDT. Ошибок нет.
fn symbols() -> Symbols {
    let info: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/mexc_2026-11-02/exchange_info.json").unwrap(),
    )
    .unwrap();
    let allowed: HashSet<String> = ["CAKEUSDT".to_string()].into();
    let s = Symbols::default();
    s.load(
        arb::venues::mexc::symbols::parse_list(&info, "USDT", &allowed).unwrap(),
        chrono::Utc::now(),
    );
    s
}

/// Ордер CAKE на 10 монет по 2.19 (шаги пары 0.01 / 0.001, минимум суммы 1 — правила проходят).
fn req(side: Side) -> TradeRequest {
    TradeRequest {
        venue: VenueId("mexc".into()),
        asset: Asset {
            ticker: "CAKE".into(),
            bsc_contract: None,
        },
        side,
        qty: d("10"),
        worst_price: d("2.19"),
        deal_id: "t412".into(),
        route: None,
        exit_position: false,
        remeasure: None,
    }
}

#[tokio::test]
async fn mexc_real_order_has_no_probe() {
    let ctx = start("real", true).await;
    let s = symbols();
    for side in [Side::Buy, Side::Sell] {
        // Подготовка настоящего ордера — без запроса (пробный дал бы ошибку на запертых секретах).
        let p = order_rules::ready(&ctx, &s, &req(side)).await;
        assert!(p.is_ok(), "{side:?}: {:?}", p.err());
        // Настоящий ордер падает на постановке (ключи заперты), а не на пробном.
        let e = trade::trade(&ctx, &s, req(side), None).await.unwrap_err();
        assert_eq!(e.code, ErrorCode::SecretsLocked, "{side:?}: {}", e.message);
        assert!(!e.message.contains(PROBE), "{side:?}: {}", e.message);
    }
}

#[tokio::test]
async fn mexc_check_keeps_probe() {
    let ctx = start("check", true).await;
    let s = symbols();
    for side in [Side::Sell, Side::Buy] {
        let e = order_rules::check(&ctx, &s, &req(side)).await.unwrap_err();
        assert!(e.message.contains(PROBE), "{side:?}: {}", e.message);
        assert_eq!(e.code, ErrorCode::SecretsLocked);
    }
    // «нет» — проверка без исполнения только правилами, запроса нет.
    let off = start("check-off", false).await;
    assert!(order_rules::check(&off, &s, &req(Side::Sell)).await.is_ok());
}

/// Тело функции `pub async fn <name>` до первой строки «}» в начале строки. Ошибок нет (паника — нет функции).
fn body<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src.find(&format!("pub async fn {name}(")).expect(name);
    let rest = &src[start..];
    &rest[..rest.find("\n}\n").expect("конец функции")]
}

/// Исходник без строк-комментариев (комментарии упоминают пробный ордер словами). Ошибок нет.
fn code(src: &str) -> String {
    src.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn sources_no_probe_before_real_order() {
    let venues = [
        (
            "mexc",
            include_str!("../src/venues/mexc/order_rules.rs"),
            include_str!("../src/venues/mexc/trade.rs"),
            include_str!("../src/venues/mexc/mod.rs"),
        ),
        (
            "binance",
            include_str!("../src/venues/binance/order_rules.rs"),
            include_str!("../src/venues/binance/trade.rs"),
            include_str!("../src/venues/binance/mod.rs"),
        ),
        (
            "kucoin",
            include_str!("../src/venues/kucoin/order_rules.rs"),
            include_str!("../src/venues/kucoin/trade.rs"),
            include_str!("../src/venues/kucoin/mod.rs"),
        ),
        (
            "kraken",
            include_str!("../src/venues/kraken/order_rules.rs"),
            include_str!("../src/venues/kraken/trade.rs"),
            include_str!("../src/venues/kraken/mod.rs"),
        ),
    ];
    for (v, rules, trade, module) in venues {
        let ready = code(body(rules, "ready"));
        let trade = code(trade);
        for probe in ["order_test_first", "test(", "probe(", "validate"] {
            assert!(
                !ready.contains(probe),
                "{v}: ready делает пробный ({probe})"
            );
            assert!(
                !trade.contains(probe),
                "{v}: trade.rs делает пробный ({probe})"
            );
        }
        assert!(
            body(rules, "probe").contains("venue.order_test_first"),
            "{v}: пробный — по настройке"
        );
        let check = &module[module.find("async fn check_order").expect(v)..];
        let check = &check[..check.find("\n    }\n").unwrap()];
        assert!(
            check.contains("order_rules::check(") || check.contains("order_rules::probe("),
            "{v}: check_order без пробного"
        );
    }
}
