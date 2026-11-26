//! Файл: tests/bsc_dex_send.rs
//! Что делает: проверяет правила отправки DEX без сети: send.max_wait_blocks < send.bundle_max_blocks — отправка
//! не работает с понятной причиной; без send.gas_limit_margin_pct SendCfg не читается (решение 398: запас газа отмены —
//! до отправки исходной); цена газа отмены выше прошлой на надбавку; обмен без минимума (худшая цена ≤ 0,
//! допуск 100%) не собирается; ожидаемое поступление токена и обмен с этим токеном на выходе не идут одновременно.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{send, exec, wallet, ctx}, arb::settings (настоящая SQLite во временной
//! папке). Моков нет: узел не нужен — проверяются правила до отправки.

use std::path::PathBuf;

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::types::{Asset, Pool, PoolRef, PoolState, Side, TradeRequest, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::trade;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::send::{cancel, tx::SendCfg};

const COIN: Address = address!("0x1000000000000000000000000000000000000001");
const USDT: Address = address!("0x2000000000000000000000000000000000000002");

/// Окружение адаптера на настоящей SQLite во временной папке.
async fn ctx(name: &str) -> Ctx {
    let d: PathBuf = std::env::temp_dir().join(format!("arb-dex-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    {
        let c = Ctx {
            raw: RawLog::start(d.join("raw"), settings.clone()),
            pace: Default::default(),
            failover: Default::default(),
            evm_chain: Default::default(),
            place: arb::venues::bsc_dex::ctx::Place::BSC,
            chains: Default::default(),
            account: None,
            index: Default::default(),
            settings,
            vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
            http: reqwest::Client::new(),
            journal,
        };
        // Решение 416: тикер газа площадки DEX — настройка (dex.gas_ticker).
        c.settings
            .set(
                "dex.gas_ticker",
                arb::settings::scope::Scope::Venue("bsc_dex".into()),
                Some(serde_json::json!("BNB")),
                "тест",
            )
            .await
            .unwrap();
        c
    }
}

/// Задаёт настройку площадки bsc_dex.
async fn set(c: &Ctx, name: &str, v: serde_json::Value) {
    c.settings
        .set(name, Scope::Venue("bsc_dex".into()), Some(v), "тест")
        .await
        .unwrap();
}

#[tokio::test]
async fn max_wait_must_cover_bundle_blocks() {
    let c = ctx("cfg").await;
    assert_eq!(
        SendCfg::read(&c).unwrap_err().code,
        ErrorCode::NotConfigured
    );
    set(&c, "send.max_wait_blocks", json!(3)).await;
    set(&c, "send.bundle_max_blocks", json!(5)).await;
    set(&c, "send.cancel_gas_bump_pct", json!("12.5")).await;
    set(&c, "feed.stale_after_ms", json!(3000)).await;
    set(&c, "send.replaced_confirm_blocks", json!(20)).await;
    set(&c, "send.cancel_max_gas_gwei", json!("5")).await;
    set(&c, "send.cancel_max_attempts", json!(3)).await;
    set(&c, "send.queue_max_wait_s", json!(600)).await;
    // Решение 398: запас газа отмены — в SendCfg (отмена уходит после исходной): не задан — отправки нет.
    let e = SendCfg::read(&c).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotConfigured);
    assert!(
        e.message.contains("send.gas_limit_margin_pct"),
        "{}",
        e.message
    );
    set(&c, "send.gas_limit_margin_pct", json!("20")).await;
    let e = SendCfg::read(&c).unwrap_err();
    assert_eq!(e.code, ErrorCode::NotConfigured);
    assert!(e.message.contains("send.max_wait_blocks"));
    set(&c, "send.max_wait_blocks", json!(5)).await;
    let cfg = SendCfg::read(&c).unwrap();
    // Отмена: 100 wei × 1,125 = 112,5 → вверх 113; каналов нет — нижней границы нет.
    // Решение 435: legacy-цена (dex.gas_mode = legacy) — та же арифметика, что до модели газа.
    use arb::venues::bsc_dex::send::tx_price::TxPrice;
    assert_eq!(
        cancel::price(&c, TxPrice::Legacy(U256::from(100u8)), &cfg, &[]).unwrap(),
        TxPrice::Legacy(U256::from(113u8))
    );
}

/// Пара V2 «монета — USDT».
fn entry() -> Entry {
    let family = families::make("pancakeswap_v2", "pancakeswap_v2").unwrap();
    Entry {
        family,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: family.id.into(),
            kind: family.kind,
            reference: PoolRef::Contract(COIN),
            token0: COIN,
            token1: USDT,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::from(1u8),
                reserve1: U256::from(1u8),
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin: COIN,
        usdt: USDT,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
        state_gen: 0,
    }
}

fn req(worst: Decimal) -> TradeRequest {
    TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            chain: None,
            ticker: "X".into(),
            contract: Some(COIN),
        },
        side: Side::Sell,
        qty: Decimal::ONE,
        worst_price: worst,
        deal_id: "т".into(),
        route: None,
        exit_position: false,
        remeasure: None,
    }
}

#[tokio::test]
async fn no_swap_without_minimum() {
    let c = ctx("min").await;
    let e = entry();
    // Худшая цена 0 — минимум не считается (и нет деления на 0).
    let err = trade::order(
        &c,
        &req(Decimal::ZERO),
        &e,
        Decimal::ONE,
        (Decimal::ONE, Decimal::ONE),
        (18, 18),
    );
    assert_eq!(err.err().unwrap().code, ErrorCode::QuoteMismatch);
    // Допуск 100% и худшая цена так мала, что минимум округляется до 0 единиц — обмена нет.
    set(&c, "exec.dex_slippage_pct", json!(100)).await;
    let tiny = Decimal::new(1, 28);
    let err = trade::order(
        &c,
        &req(tiny),
        &e,
        Decimal::ONE,
        (Decimal::ONE, Decimal::ONE),
        (18, 6),
    );
    assert_eq!(err.err().unwrap().code, ErrorCode::QuoteMismatch);
    // Нормальная цена — минимум больше нуля.
    let o = trade::order(
        &c,
        &req(Decimal::ONE),
        &e,
        Decimal::ONE,
        (Decimal::ONE, Decimal::ONE),
        (18, 6),
    )
    .unwrap();
    assert_eq!(o.min_out, U256::from(1_000_000u32));
}

/// Время фиксируется для конкретной попытки HTTP. Отказ настройки до отправки и
/// повтор с реальным ответом канала различимы без логирования тела запроса.
#[tokio::test]
async fn builder_http_start_marks_only_attempted_requests() {
    use alloy_primitives::Bytes;
    use arb::venues::bsc_dex::send::{
        self, blockrazor, public, ready, sequencer_cond::Terms, stats::Stats,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn serve(listener: TcpListener, body: &str) -> chrono::DateTime<Utc> {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        assert!(stream.read(&mut buf).await.unwrap() > 0);
        let observed = Utc::now();
        let http = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(http.as_bytes()).await.unwrap();
        observed
    }

    let c = ctx("builder-http-timestamp").await;
    let rejected = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let accepted = TcpListener::bind("127.0.0.1:0").await.unwrap();
    for (id, addr) in [
        ("public", rejected.local_addr().unwrap()),
        ("blockrazor", accepted.local_addr().unwrap()),
    ] {
        for (name, val) in [
            ("channel.enabled", json!(true)),
            ("channel.url", json!(format!("http://{addr}"))),
            ("channel.method", json!("single")),
            ("channel.min_gas_price_gwei", json!("1")),
        ] {
            c.settings
                .set(name, Scope::Channel(id.into()), Some(val), "тест")
                .await
                .unwrap();
        }
    }
    let targets = [
        ready::prepare(&c, &public::CHANNEL).unwrap(),
        ready::prepare(&c, &blockrazor::CHANNEL).unwrap(),
    ];
    let bytes = Bytes::from_static(&[1, 2, 3]);
    let stats = Stats::default();
    let _ = send::dispatch(&c, &stats, &targets, &bytes, None, 1, Terms::default()).await;
    let before_rows = c.journal.list(20, None).await.unwrap();
    for (name, val) in [
        ("http.timeout_ms", 5000),
        ("http.body_idle_ms", 5000),
        ("http.total_ms", 10000),
        ("http.max_body_bytes", 1000000),
    ] {
        c.settings
            .set(name, Scope::Global, Some(json!(val)), "тест")
            .await
            .unwrap();
    }
    let reject_peer = tokio::spawn(serve(
        rejected,
        r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32000,"message":"execution reverted"}}"#,
    ));
    let accept_peer = tokio::spawn(serve(
        accepted,
        r#"{"jsonrpc":"2.0","id":1,"result":"0x01"}"#,
    ));
    let before = Utc::now();
    let dispatch = send::dispatch(&c, &stats, &targets, &bytes, None, 1, Terms::default())
        .await
        .unwrap();
    let (seen_reject, seen_accept) = (reject_peer.await.unwrap(), accept_peer.await.unwrap());
    assert_eq!(dispatch.accepted, ["blockrazor"]);
    assert!(dispatch.uncertain.is_empty());
    assert!(
        before_rows.iter().any(|e| e.kind == "send.channel_rejected"
            && e.message.starts_with("public:")
            && e.message.contains("HTTP: не начат")),
        "{before_rows:?}"
    );
    let rows = c.journal.list(20, None).await.unwrap();
    let attempts = rows
        .iter()
        .find_map(|e| {
            e.message
                .strip_prefix("public:")
                .and_then(|t| t.split_once("HTTP: старт ").map(|(_, tail)| tail))
        })
        .expect("начало конкретного HTTP запроса");
    let (failed, accepted) = attempts
        .split_once("; принят blockrazor, HTTP старт ")
        .expect("принятый соседний канал");
    for (stamp, received) in [(failed, seen_reject), (accepted, seen_accept)] {
        let recorded = chrono::DateTime::parse_from_rfc3339(stamp)
            .unwrap()
            .to_utc();
        assert!(before.timestamp_micros() <= recorded.timestamp_micros() && recorded <= received);
    }
}
