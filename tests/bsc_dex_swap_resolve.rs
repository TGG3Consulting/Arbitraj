//! Файл: tests/bsc_dex_swap_resolve.rs
//! Что проверяет: решение 231, дополнение (Тигран 08.11: «своп DEX по квитанции — восстанавливать итог полностью»),
//! без сети:
//! 1) итог по квитанции из «чем найти» (exec/swap_rebuild.rs) совпадает с обычным путём (exec/chain_tx.rs +
//!    exec/result.rs + пометка «по квотеру» + подпись исполнителя + причина «лучший из двух») — на НАСТОЯЩЕЙ квитанции
//!    BSC (tests/data/receipts_2026-10-25/responses.json, eth_getBlockReceipts блока 129702236, tx 0x6fa1…: с адреса
//!    0xb9e6… ушло 400 токена 0x63d1…, пришло 12 USDT). Кошелёк и стороны — взгляд теста на эту квитанцию (кошелёк =
//!    отправитель, продажа 0x63d1… за USDT); decimals 18/18 — параметр теста: сверяется равенство двух путей;
//! 2) квитанция со статусом 0 — SwapReverted (как обычный путь), с фактом транзакции «откат»;
//! 3) разрешение (approve) с неизвестным исходом — в блоке: SwapReverted с фактом разрешения, обмена не было;
//! 4) «чем найти»: контекст прикладывается только к OutcomeUnknown с фактом транзакции; чаевые из seed (exec/txs.rs)
//!    сохраняются; подпись и причина дописываются; контекст переживает JSON (ошибка хранится и передаётся ядром);
//!    без кошелька (KyberSwap, бандл, старые операции) — контекст неполный: выясняет человек.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{ctx, exec, send}, arb::gateway::{errors, pending}, настоящая SQLite во
//! временной папке (Ctx: тексты ошибок с площадкой).

use std::str::FromStr;

use alloy_primitives::{Address, B256, U64, address};
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::broadcast;

use arb::core::types::{
    Asset, ChainTx, SendMethod, Side, TradeRequest, TradeResult, TxOutcome, VenueId,
};
use arb::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::swap_ctx::{self, SwapCtx};
use arb::venues::bsc_dex::exec::{chain_tx, result, swap_rebuild, txs};
use arb::venues::bsc_dex::send::receipt::Receipt;
use arb::venues::bsc_dex::send::track_util::Tracked;

const RECEIPTS: &str = "tests/data/receipts_2026-10-25/responses.json";
/// Отправитель транзакции 0x6fa1… в записи — «кошелёк» теста.
const WALLET: Address = address!("0xb9e606aee813ad7e99670066185792ef4f7a027d");
/// Токен, ушедший с кошелька (Transfer на 0x0 в записи).
const COIN: Address = address!("0x63d1eacebb70bbbf0641e7c88e9099301bba3688");
/// USDT в сети BSC (Transfer на кошелёк в записи).
const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
const DECS: (u8, u8) = (18, 18);

/// Квитанция tx 0x6fa1… из записи (results[2] — eth_getBlockReceipts блока 129702236). Паника — файла нет.
fn receipt() -> (B256, Receipt) {
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(RECEIPTS).unwrap()).unwrap();
    let r = rec["results"][2]["response"]["result"][1].clone();
    let hash = B256::from_str(r["transactionHash"].as_str().unwrap()).unwrap();
    (hash, serde_json::from_value(r).unwrap())
}

/// Окружение площадки (тексты ошибок). Настоящая SQLite во временной папке; сеть не нужна.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-swap-resolve-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    }
}

/// Продажа 400 монет (как в записи).
fn req() -> TradeRequest {
    TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "COIN".into(),
            bsc_contract: Some(COIN),
        },
        side: Side::Sell,
        qty: Decimal::new(400, 0),
        worst_price: Decimal::new(1, 2),
        deal_id: "20261108120000000COIN".into(),
        route: None,
        exit_position: false,
    }
}

/// Факт транзакции на момент ошибки «исход неизвестен» (exec/txs.rs: исход не установлен, квитанции нет).
fn unknown_chain(hash: B256) -> ChainTx {
    ChainTx {
        hash,
        nonce: 41,
        channels: vec!["48club".into()],
        method: SendMethod::Bundle,
        outcome: TxOutcome::Pending,
        block: None,
        gas_used: None,
        gas_bnb: None,
        tip_bnb: None,
        cancel_hash: None,
        cancel_gas_bnb: None,
    }
}

/// Обычный путь: тот же факт транзакции (chain_tx), разбор exec/result.rs, пометка, подпись, причина.
fn normal(ctx: &Ctx, hash: B256, r: Receipt, tip: Option<Decimal>) -> GatewayResult<TradeResult> {
    let outcome = if r.status == U64::from(1u8) {
        TxOutcome::Included
    } else {
        TxOutcome::Reverted
    };
    let t = Tracked {
        outcome,
        receipt: Some(r.clone()),
        cancel: None,
        unresolved: None,
    };
    let chain = chain_tx::chain_tx(hash, 41, vec!["48club".into()], SendMethod::Bundle, &t, tip);
    let d = txs::Done {
        chain,
        receipt: Some(r),
        cancel: None,
        tip_bnb: tip,
    };
    let out = result::result(ctx, req(), d, WALLET, (COIN, USDT), DECS, &[]).map(|mut t| {
        t.executor = Some("— продажа по квотеру".into());
        t
    });
    swap_rebuild::tail(result::by(out, result::BY_DIRECT), Some("свой дешевле"))
}

/// Ошибка «исход неизвестен», как её отдаёт путь: seed с чаевыми (txs.rs) → контекст пути → подпись → причина.
fn failed(hash: B256, tip: Option<Decimal>) -> GatewayError {
    let chain = unknown_chain(hash);
    let e = GatewayError::new(ErrorCode::OutcomeUnknown, None, "исход неизвестен")
        .with_tx(chain.clone())
        .pending(swap_ctx::seed(&chain, tip));
    let sc = SwapCtx::swap(
        WALLET,
        (COIN, USDT),
        DECS,
        &[],
        None,
        Some("— продажа по квотеру".into()),
    );
    let e = swap_ctx::who(sc.attach(e), result::BY_DIRECT);
    swap_ctx::best_of(e, "свой дешевле")
}

/// Контекст из ошибки после JSON (как ядро хранит и передаёт GatewayError).
fn ctx_of(e: &GatewayError) -> SwapCtx {
    let back: GatewayError = serde_json::from_str(&serde_json::to_string(e).unwrap()).unwrap();
    SwapCtx::of(back.pending.as_deref().unwrap()).unwrap()
}

#[tokio::test]
async fn rebuilt_equals_normal_path() {
    let c = ctx("same").await;
    let (hash, r) = receipt();
    let tip = Some(Decimal::from_str("0.0001").unwrap());
    let sc = ctx_of(&failed(hash, tip));
    assert!(sc.complete());
    let want = normal(&c, hash, r.clone(), tip).unwrap();
    let got = swap_rebuild::rebuild(&c, req(), &sc, hash, r).unwrap();
    assert_eq!(got.status, want.status);
    assert_eq!(got.filled_qty, want.filled_qty);
    assert_eq!(got.filled_qty, Decimal::new(400, 0));
    assert_eq!(got.avg_price, want.avg_price);
    assert_eq!(got.avg_price, Decimal::from_str("0.03").unwrap());
    assert_eq!(got.fees, want.fees);
    assert_eq!(got.venue_ref, want.venue_ref);
    assert_eq!(got.executor, want.executor);
    assert_eq!(
        got.executor.as_deref(),
        Some("свой сборщик: прямой; — продажа по квотеру; лучший из двух: свой дешевле")
    );
    let (g, w) = (got.chain.unwrap(), want.chain.unwrap());
    assert_eq!(
        serde_json::to_value(&g).unwrap(),
        serde_json::to_value(&w).unwrap()
    );
    assert_eq!(g.outcome, TxOutcome::Included);
    assert_eq!(g.tip_bnb, tip);
    assert_eq!(g.block, Some(129702236));
}

#[tokio::test]
async fn reverted_receipt_is_swap_reverted() {
    let c = ctx("revert").await;
    let (hash, mut r) = receipt();
    r.status = U64::from(0u8);
    let sc = ctx_of(&failed(hash, None));
    let want = normal(&c, hash, r.clone(), None).unwrap_err();
    let got = swap_rebuild::rebuild(&c, req(), &sc, hash, r).unwrap_err();
    assert_eq!(got.code, ErrorCode::SwapReverted);
    assert_eq!(got.code, want.code);
    assert_eq!(got.message, want.message);
    assert_eq!(got.tx.unwrap().outcome, TxOutcome::Reverted);
}

#[tokio::test]
async fn approve_in_block_means_no_swap() {
    let c = ctx("approve").await;
    let (hash, r) = receipt();
    let chain = unknown_chain(hash);
    let e = GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        "исход разрешения неизвестен",
    )
    .with_tx(chain.clone())
    .pending(swap_ctx::seed(&chain, None));
    let e = swap_ctx::who(swap_ctx::approve(e), result::BY_SMART);
    let sc = ctx_of(&e);
    assert!(sc.approve && sc.complete());
    let got = swap_rebuild::rebuild(&c, req(), &sc, hash, r).unwrap_err();
    assert_eq!(got.code, ErrorCode::SwapReverted);
    assert!(
        got.message
            .starts_with("[свой сборщик: умный] Разрешение (approve)")
    );
    assert!(got.message.contains("обмен после него не отправлялся"));
    let tx = got.tx.unwrap();
    assert_eq!(tx.outcome, TxOutcome::Included);
    assert!(tx.gas_bnb.is_some());
}

#[test]
fn attach_only_outcome_unknown_with_tx() {
    let hash = B256::repeat_byte(7);
    let chain = unknown_chain(hash);
    let sc = SwapCtx::swap(WALLET, (COIN, USDT), DECS, &[], None, None);
    // Другой код — не трогается.
    let rej =
        sc.attach(GatewayError::new(ErrorCode::SwapReverted, None, "откат").with_tx(chain.clone()));
    assert!(rej.pending.is_none());
    // Без факта транзакции — не трогается.
    let bare = sc.attach(GatewayError::new(
        ErrorCode::OutcomeUnknown,
        None,
        "нет факта",
    ));
    assert!(bare.pending.is_none());
    // Чаевые seed сохраняются, хеш и nonce — из факта.
    let tip = Some(Decimal::new(5, 4));
    let e = GatewayError::new(ErrorCode::OutcomeUnknown, None, "x")
        .with_tx(chain.clone())
        .pending(swap_ctx::seed(&chain, tip));
    let e = sc.attach(e);
    let p = e.pending.as_deref().unwrap();
    assert_eq!(p.venue_ref.as_deref(), Some(hash.to_string().as_str()));
    assert_eq!(p.nonce, Some(41));
    let got = SwapCtx::of(p).unwrap();
    assert_eq!(got.tip_bnb, tip);
    assert_eq!(got.wallet, Some(WALLET));
    assert!(got.complete());
}

#[test]
fn no_wallet_context_is_incomplete() {
    // KyberSwap / бандл / старая операция: в «чем найти» только seed (или подпись) — выясняет человек.
    let chain = unknown_chain(B256::repeat_byte(9));
    let e = GatewayError::new(ErrorCode::OutcomeUnknown, None, "x")
        .with_tx(chain.clone())
        .pending(swap_ctx::seed(&chain, None));
    let e = swap_ctx::who(e, result::BY_BUNDLE);
    assert!(!ctx_of(&e).complete());
    let old = arb::gateway::pending::PendingRef {
        venue_ref: Some("0xabc".into()),
        nonce: Some(1),
        ..Default::default()
    };
    assert!(SwapCtx::of(&old).is_none());
}
