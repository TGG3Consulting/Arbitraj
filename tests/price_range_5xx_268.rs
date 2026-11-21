//! Файл: tests/price_range_5xx_268.rs
//! Что делает: решение 268, дополнение (ревью 2): ответ биржи с кодом «цена ордера вне диапазона» при HTTP 5XX код
//! шлюза PriceOutOfRange НЕ получает — остаётся прежний отказ ордера (OrderRejected, как до решения 268): по ответу с
//! ошибкой сервера рыночный ордер не ставится. Проверяется настоящий разбор ответа адаптеров (reply::parse) Binance,
//! Bybit, OKX, XT: тот же ответ при HTTP 400 / 200 — PriceOutOfRange, при 500 / 503 — OrderRejected.
//! ТЕЛА ОТВЕТОВ СОБРАНЫ ПО ОФИЦИАЛЬНОЙ ДОКУМЕНТАЦИИ, А НЕ ЗАПИСАНЫ (заменить записанными после первого живого случая,
//! docs/TASKS.md): конверты и коды — как в tests/price_range_268.rs (ссылки на документацию — в errors.rs адаптеров).
//! BingX: тот же запрет стоит в venues/bingx/rest.rs (parse — закрытая функция, отдельного теста нет).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::{binance, bybit, okx, xt}, arb::gateway::http::Reply, arb::settings, arb::storage.

use arb::gateway::errors::ErrorCode as C;
use arb::gateway::http::Reply;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::{binance, bybit, okx, xt};
use reqwest::StatusCode;
use tokio::sync::broadcast;

/// Общие части контекста адаптера (настоящая SQLite во временной папке; хранилище секретов закрыто — не нужно).
async fn parts(name: &str) -> (Store, arb::secrets::Vault, Journal, RawLog) {
    let d = std::env::temp_dir().join(format!("arb-268-5xx-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let raw = RawLog::start(d.join("raw"), settings.clone());
    (settings, vault, journal, raw)
}

fn reply(status: u16, text: &str) -> Reply {
    Reply {
        status: StatusCode::from_u16(status).unwrap(),
        text: text.to_string(),
        retry_after_s: None,
    }
}

/// HTTP-статус → ожидаемый код шлюза у ответа с кодом коридора на ордер.
const CASES: [(u16, C); 4] = [
    (200, C::PriceOutOfRange),
    (400, C::PriceOutOfRange),
    (500, C::OrderRejected),
    (503, C::OrderRejected),
];

#[tokio::test]
async fn binance_bybit_okx_xt_code_at_http_5xx_is_plain_refusal() {
    let (settings, vault, journal, raw) = parts("all").await;
    let http = reqwest::Client::new();

    let ctx = binance::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
    };
    let body = r#"{"code":-1013,"msg":"Filter failure: PERCENT_PRICE_BY_SIDE"}"#;
    for (st, want) in CASES {
        let r = reply(st, body);
        let e = binance::reply::parse(&ctx, "/api/v3/order", &r, binance::errors::Op::Trade);
        assert_eq!(e.unwrap_err().code, want, "binance HTTP {st}");
    }

    let ctx = bybit::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
        taps: Default::default(),
        private: Default::default(),
    };
    let body = r#"{"retCode":170194,"retMsg":"Sell order price cannot be lower than 0.1.","result":{},"retExtInfo":{},"time":1}"#;
    for (st, want) in CASES {
        let r = bybit::http::Reply {
            status: StatusCode::from_u16(st).unwrap(),
            text: body.to_string(),
            limit: None,
            left: None,
            reset_ms: None,
        };
        let e = bybit::reply::parse(&ctx, "/v5/order/create", &r, bybit::errors::Op::Trade);
        assert_eq!(e.unwrap_err().code, want, "bybit HTTP {st}");
    }

    let ctx = okx::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: http.clone(),
        journal: journal.clone(),
        raw: raw.clone(),
    };
    let body = r#"{"code":"1","msg":"","data":[{"ordId":"","clOrdId":"x","sCode":"51138","sMsg":"The lowest price limit for sell orders is 0.1."}]}"#;
    for (st, want) in CASES {
        let r = reply(st, body);
        let e = okx::reply::parse(&ctx, "/api/v5/trade/order", &r, okx::errors::Op::Trade);
        assert_eq!(e.unwrap_err().code, want, "okx HTTP {st}");
    }

    let ctx = xt::ctx::Ctx {
        settings,
        vault,
        http,
        journal,
        raw,
    };
    let body = r#"{"rc":1,"mc":"ORDER_F0501","ma":[],"result":null}"#;
    for (st, want) in CASES {
        let r = reply(st, body);
        let e = xt::reply::parse(
            &ctx,
            "/v4/order",
            &r,
            xt::errors::Op::Trade,
            xt::reply::Raw::Write,
        );
        assert_eq!(e.unwrap_err().code, want, "xt HTTP {st}");
    }
}
