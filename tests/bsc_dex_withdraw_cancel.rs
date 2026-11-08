//! Файл: tests/bsc_dex_withdraw_cancel.rs
//! Что проверяет: вывод из кошелька DEX с неизвестным исходом, когда nonce мог занять наша отмена (ветка
//! t49/withdraw-cancel; как у свопа и цикла — tests/bsc_dex_single_cancel.rs), на НАСТОЯЩИХ квитанциях BSC и живом
//! ядре (настоящая SQLite, моков нет):
//! 1) «чем найти» перевода несёт хеши отмен: exec/txs.rs / wallet/transfer.rs seed + with_cancels → venue.rs
//!    resolve::attach (не перетирает) → JSON (как ядро хранит ошибку) — список отмен не теряется;
//! 2) итог «вывода нет» (exec/single_rebuild.rs withdraw_cancelled): WithdrawRejected, факт транзакции тот же, что
//!    обычный путь даёт при отмене, занявшей nonce (exec/txs.rs chain_tx: NotIncluded, cancel_hash, cancel_gas_bnb по
//!    квитанции отмены); ядро считает его окончательным (core/outcome_resolve.rs definitive), «исход неизвестен» снят;
//!    без факта транзакции в контексте — Internal (не окончательно, ядро спросит ещё);
//! 3) живое ядро (core/usdt_return_retry.rs fail): найденный итог — «вывод отклонён площадкой — не повторяем», а не
//!    «перевод мог уйти»; исходная ошибка «исход неизвестен» — «исход перегона неизвестен».
//!
//! Вердикт по квитанциям (judge) — общий со свопом, проверен в tests/bsc_dex_single_cancel.rs. Чтение узлом и снятие
//! очереди (resolve_withdraw.rs → resolve_single.rs verdict, resolve.rs release_if_ours) — через узел, здесь не
//! проверяются (как у свопа).
//!
//! Квитанция отмены — tests/data/receipts_2026-10-25/responses.json results[4] индекс 7 (tx 0xc10c272c…, блок
//! 0x7bb195d): перевод 0 BNB самому себе без логов, gasUsed 0x5208 — того же вида, что отмена send/cancel.rs. Хеш
//! исходного перевода — responses_3.json results[2] индекс 54 (как в tests/bsc_dex_single_cancel.rs). Nonce, канал —
//! параметры теста.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec::{chain_tx, single_rebuild, swap_ctx}, resolve, send}, arb::core::{
//! op_error, outcome_resolve, usdt_return_retry, ledger}, tests/usdt_return_common, настоящая SQLite.

mod usdt_return_common;

use std::str::FromStr;

use alloy_primitives::{B256, U256};
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::broadcast;

use arb::core::deal::Step;
use arb::core::op_error::OpError;
use arb::core::outcome_resolve;
use arb::core::types::{ChainTx, SendMethod, TxOutcome, VenueId};
use arb::core::usdt_return_retry::fail;
use arb::core::usdt_return_state::UsdtReturn;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::swap_ctx::{self, SwapCtx};
use arb::venues::bsc_dex::exec::{chain_tx, single_rebuild};
use arb::venues::bsc_dex::resolve;
use arb::venues::bsc_dex::send::receipt::Receipt;
use arb::venues::bsc_dex::send::track_util::Tracked;
use usdt_return_common::{dir, doge, start};

const R1: &str = "tests/data/receipts_2026-10-25/responses.json";
const R3: &str = "tests/data/receipts_2026-10-25/responses_3.json";
const NONCE: u64 = 41;

/// Квитанция записи: results[res] — eth_getBlockReceipts, индекс транзакции. Паника — файла нет.
fn receipt(file: &str, res: usize, i: usize) -> (B256, Receipt) {
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    let r = rec["results"][res]["response"]["result"][i].clone();
    let h = B256::from_str(r["transactionHash"].as_str().unwrap()).unwrap();
    (h, serde_json::from_value(r).unwrap())
}

/// Отмена: перевод 0 BNB себе (responses.json, results[4], индекс 7).
fn cancel_rc() -> (B256, Receipt) {
    receipt(R1, 4, 7)
}

/// Хеш исходного перевода (responses_3.json, results[2], индекс 54).
fn hash() -> B256 {
    receipt(R3, 2, 54).0
}

/// Окружение площадки (тексты ошибок). Настоящая SQLite во временной папке; сеть не нужна.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-wd-cancel-{}-{tag}", std::process::id()));
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

/// «Исход неизвестен» перевода, как его отдаёт путь: wallet/transfer.rs («вытеснена» — seed + отмены) → venue.rs
/// (resolve::attach) → JSON. Принимает: окружение, хеш перевода, отмены.
fn unknown(c: &Ctx, hash: B256, cancels: &[B256]) -> GatewayError {
    let t = Tracked::bare(TxOutcome::Replaced, None);
    let chain = chain_tx::chain_tx(
        hash,
        NONCE,
        vec!["48club".into()],
        SendMethod::Single,
        &t,
        None,
    );
    let e = c
        .err(ErrorCode::OutcomeUnknown, "исход перевода неизвестен")
        .with_tx(chain.clone())
        .pending(swap_ctx::with_cancels(
            swap_ctx::seed(&chain, None),
            cancels,
        ));
    let e = resolve::attach(e);
    serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap()
}

/// Контекст из «чем найти».
fn ctx_of(e: &GatewayError) -> SwapCtx {
    SwapCtx::of(e.pending.as_deref().expect("«чем найти»")).expect("контекст")
}

#[tokio::test]
async fn pending_ref_keeps_cancels() {
    let c = ctx("ref").await;
    let (h, _) = cancel_rc();
    let e = unknown(&c, hash(), &[h]);
    let p = e.pending.as_deref().unwrap();
    assert_eq!(p.venue_ref.as_deref(), Some(hash().to_string().as_str()));
    assert_eq!(p.nonce, Some(NONCE));
    let sc = ctx_of(&e);
    assert_eq!(
        sc.cancels,
        Some(vec![h]),
        "attach не перетирает «чем найти»"
    );
    assert_eq!(sc.chain.as_ref().map(|t| t.nonce), Some(NONCE));
    assert_eq!(ctx_of(&unknown(&c, hash(), &[])).cancels, Some(vec![]));
}

#[tokio::test]
async fn cancelled_withdraw_is_final_with_cancel_gas() {
    let c = ctx("cancelled").await;
    let (h, rc) = cancel_rc();
    let sc = ctx_of(&unknown(&c, hash(), &[h]));
    let late = single_rebuild::withdraw_cancelled(
        &c,
        &sc,
        hash(),
        (h, rc.clone()),
        "очередь кошелька снята",
    );
    assert_eq!(late.code, ErrorCode::WithdrawRejected);
    assert!(late.message.contains("вывода нет"), "{}", late.message);
    assert!(late.message.contains(&h.to_string()), "{}", late.message);
    // Обычный путь: трекер нашёл квитанцию отмены (send/track_util.rs landed) → exec/txs.rs chain_tx.
    let t = Tracked {
        outcome: TxOutcome::NotIncluded,
        receipt: None,
        cancel: Some((h, rc.clone())),
        unresolved: None,
    };
    let normal = chain_tx::chain_tx(
        hash(),
        NONCE,
        vec!["48club".into()],
        SendMethod::Single,
        &t,
        None,
    );
    // Сравнение через JSON (как ядро хранит факт).
    let js = |t: Option<&ChainTx>| serde_json::to_value(t).unwrap();
    assert_eq!(
        js(late.tx.as_deref()),
        js(Some(&normal)),
        "факт транзакции тот же"
    );
    let tx = late.tx.as_deref().unwrap();
    assert_eq!(tx.outcome, TxOutcome::NotIncluded);
    assert_eq!(tx.cancel_hash, Some(h));
    // Газ отмены — по квитанции: gasUsed × effectiveGasPrice.
    let wei = U256::from(rc.gas_used) * rc.effective_gas_price;
    assert_eq!(
        tx.cancel_gas_bnb,
        Some(Decimal::from_str(&wei.to_string()).unwrap() / Decimal::from(10u64.pow(18)))
    );
    assert!(
        outcome_resolve::definitive(&late),
        "ядро: итог окончательный"
    );
    let op = OpError::from(&late);
    assert!(!op.outcome_unknown() && op.sent && op.cancelled);
    // Без факта транзакции в контексте — Internal: не окончательно, ядро спросит ещё.
    let mut bare = sc.clone();
    bare.chain = None;
    let x = single_rebuild::withdraw_cancelled(&c, &bare, hash(), (h, rc), "");
    assert_eq!(x.code, ErrorCode::Internal);
    assert!(!outcome_resolve::definitive(&x));
}

#[tokio::test]
async fn core_does_not_retry_withdraw_cancelled() {
    let c = ctx("core").await;
    let (h, rc) = cancel_rc();
    let e = unknown(&c, hash(), &[h]);
    let found = single_rebuild::withdraw_cancelled(
        &c,
        &ctx_of(&e),
        hash(),
        (h, rc),
        "очередь кошелька снята",
    );
    // Живое ядро с повторами возврата USDT (1 с, 3 раза): найденный итог не повторяется.
    let (core, _) = start(&dir("wdcancel"), Some((1, 3))).await;
    let (from, to) = (VenueId("bsc_dex".into()), VenueId("mexc".into()));
    let amount = Decimal::new(8310, 2);
    for (id, err, want) in [
        ("w1DOGE", &found, "вывод отклонён площадкой"),
        ("w2DOGE", &e, "исход перегона неизвестен"),
    ] {
        let mut x = doge(id);
        x.step = Step::Closed;
        arb::core::ledger::save(&core, &x).await;
        assert!(!fail(&core, id, amount, (&from, &to), 0, &OpError::from(err)).await);
        let d = core.deals().into_iter().find(|d| d.id == id).unwrap();
        assert!(
            matches!(&d.usdt_return, Some(UsdtReturn::NotReturned { why, .. }) if why.contains(want)),
            "{id}: {:?}",
            d.usdt_return
        );
    }
}
