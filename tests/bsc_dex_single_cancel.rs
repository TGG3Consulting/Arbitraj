//! Файл: tests/bsc_dex_single_cancel.rs
//! Что проверяет: позднее выяснение одиночной транзакции DEX, когда nonce занят, а своей квитанции нет (ветка
//! t42/small-fixes; дополнение решений 231 и 233 п. 4), на НАСТОЯЩИХ квитанциях BSC и живом ядре (настоящая SQLite,
//! моков нет):
//! 1) «чем найти» несёт хеши отмен: txs.rs seed + with_cancels → контекст цикла (cycle_ref attach) → подпись (who) →
//!    JSON (как ядро хранит ошибку) — список отмен не теряется (и пустой список «отмен не было» — Some);
//! 2) вердикт exec/single_rebuild.rs judge: квитанция отмены в блоке → «не исполнено»; своя квитанция → «в блоке»;
//!    обе → противоречие; квитанций нет, nonce занят не позже «голова − send.replaced_confirm_blocks» → противоречие
//!    (место заняла не наша транзакция); узел знает транзакцию в блоке / nonce не занят → ждём;
//! 3) итог «не исполнено» (cancelled) РАВЕН обычному пути при отмене, занявшей nonce (exec/result.rs: TxNotIncluded,
//!    факт с cancel_hash и cancel_gas_bnb по квитанции отмены), с подписью исполнителя; ядро считает его окончательным
//!    (core/outcome_resolve.rs definitive) и безопасным повтором продажи (OpError::cancel_confirmed, решение 229);
//! 4) живое ядро (core/cycle_deal.rs settle_unknown): цикл, чей nonce занят отменой, — Closed, газ отмены в затраты.
//!
//! Квитанции — tests/data/receipts_2026-10-25: «отмена» — responses.json results[4] индекс 7 (tx 0xc10c272c…, блок
//! 0x7bb195d): перевод 0 BNB самому себе без логов, gasUsed 0x5208 — того же вида, что отмена send/cancel.rs; «своя» —
//! responses_3.json results[2] индекс 54 (tx 0xff0fbd19…, как tests/bsc_dex_cycle_resolve.rs). Nonce, курсы, decimals —
//! параметры теста.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{chain_tx, cycle_ref, cycle_steps, result, single_rebuild, swap_ctx,
//! txs}, arb::core::{cycle_deal, cycle_plan, ledger, op_error, outcome_resolve}, настоящая SQLite.

use std::str::FromStr;

use alloy_primitives::{Address, B256, U256, address};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::Core;
use arb::core::deal::Step;
use arb::core::deps::Deps;
use arb::core::op_error::OpError;
use arb::core::types::{CycleTradeRequest, DexCycle, SendMethod, TxOutcome, VenueId};
use arb::core::{cycle_deal, cycle_plan, ledger, outcome_resolve};
use arb::gateway::contract::VenueKind;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::cycle_trade::BY_CYCLE_EXECUTOR;
use arb::venues::bsc_dex::exec::single_rebuild::{self, Seen, Verdict};
use arb::venues::bsc_dex::exec::swap_ctx::{self, SwapCtx};
use arb::venues::bsc_dex::exec::{chain_tx, cycle_ref, cycle_steps, result, txs};
use arb::venues::bsc_dex::quote::cycle_cost::Rates;
use arb::venues::bsc_dex::send::receipt::Receipt;
use arb::venues::bsc_dex::send::track_util::Tracked;

const R1: &str = "tests/data/receipts_2026-10-25/responses.json";
const R3: &str = "tests/data/receipts_2026-10-25/responses_3.json";
/// Кошелёк и базовая монета цикла (отправитель и токен tx 0xff0fbd19…).
const W1: Address = address!("0x4bf2763aea6a607a3b77b19f3df069e9486d2db0");
const B1: Address = address!("0xe277b2762a4355bdb04da7b98833c8aa7dff8888");
const DEC: u8 = 18;
const NONCE: u64 = 77;
const HEAD: u64 = 129_703_990;
const CONFIRM: u64 = 3;

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

/// Своя транзакция цикла (responses_3.json, results[2], индекс 54).
fn ours_rc() -> (B256, Receipt) {
    receipt(R3, 2, 54)
}

/// Курсы сделки (параметр теста).
fn rates() -> Rates {
    Rates {
        base_usdt: Decimal::new(25, 2),
        bnb_usdt: Decimal::new(612, 0),
        bnb_in_base: Decimal::new(2448, 0),
    }
}

/// Запрос цикла ядра.
fn req() -> CycleTradeRequest {
    let cycle = DexCycle {
        venue: VenueId("bsc_dex".into()),
        base: B1,
        base_ticker: "TKN".into(),
        input: Decimal::from(18),
        output: Decimal::from(18),
        gross_profit: Decimal::ZERO,
        gas_units: 0,
        gas_bnb: Decimal::ZERO,
        tip_bnb: Decimal::ZERO,
        bnb_cost: Decimal::ZERO,
        bnb_cost_in_base: None,
        net_profit: None,
        hops: Vec::new(),
        executable: true,
        exec_note: None,
        block: 0,
    };
    CycleTradeRequest {
        venue: VenueId("bsc_dex".into()),
        deal_id: "20261108120000000TKN".into(),
        cycle,
        input: Decimal::from(18),
        min_out: Decimal::from(19),
    }
}

/// Окружение площадки (тексты ошибок). Настоящая SQLite во временной папке; сеть не нужна.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-single-cancel-{}-{tag}", std::process::id()));
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

/// «Исход неизвестен» одиночной транзакции цикла, как его отдаёт путь: txs.rs (Pending, seed + отмены) → cycle_exec.rs
/// (контекст цикла) → cycle_trade.rs (подпись) → JSON. Принимает: хеш исходной, отмены.
fn unknown(c: &Ctx, hash: B256, cancels: &[B256]) -> GatewayError {
    let pend = Tracked::bare(TxOutcome::Pending, Some("отмен уже 2".into()));
    let ch = vec!["48club".to_string()];
    let chain = chain_tx::chain_tx(hash, NONCE, ch, SendMethod::Single, &pend, None);
    let e = c
        .err(ErrorCode::OutcomeUnknown, "исход неизвестен")
        .with_tx(chain.clone())
        .pending(swap_ctx::with_cancels(
            swap_ctx::seed(&chain, None),
            cancels,
        ));
    let e = cycle_ref::ctx(W1, B1, DEC, &rates()).attach(e);
    let e = swap_ctx::who(e, BY_CYCLE_EXECUTOR);
    serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap()
}

/// Контекст разбора из «чем найти».
fn ctx_of(e: &GatewayError) -> SwapCtx {
    SwapCtx::of(e.pending.as_deref().expect("«чем найти»")).expect("контекст")
}

/// Что видит узел (параметры вердикта).
fn seen(ours: Option<Receipt>, cancel: Option<(B256, Receipt)>, in_block: bool, deep: u64) -> Seen {
    Seen {
        ours,
        cancel,
        in_block,
        nonce: NONCE,
        nonce_deep: deep,
        head: HEAD,
        confirm: CONFIRM,
    }
}

#[tokio::test]
async fn pending_ref_keeps_cancels() {
    let c = ctx("ref").await;
    let (hash, _) = ours_rc();
    let (h, _) = cancel_rc();
    let sc = ctx_of(&unknown(&c, hash, &[h]));
    assert_eq!(sc.cancels, Some(vec![h]), "отмены — после attach и who");
    assert!(sc.cycle.is_some() && sc.who.as_deref() == Some(BY_CYCLE_EXECUTOR));
    assert_eq!(ctx_of(&unknown(&c, hash, &[])).cancels, Some(vec![]));
    // «Чем найти» до этого поля — список неизвестен (None): resolve.rs идёт старым путём.
    let old = swap_ctx::seed(
        &chain_tx::chain_tx(
            hash,
            NONCE,
            vec![],
            SendMethod::Single,
            &Tracked::bare(TxOutcome::Pending, None),
            None,
        ),
        None,
    );
    assert_eq!(SwapCtx::of(&old).unwrap().cancels, None);
}

#[test]
fn judge_on_real_receipts() {
    let (h, rc) = cancel_rc();
    let (_, own) = ours_rc();
    assert!(matches!(
        single_rebuild::judge(seen(None, Some((h, rc.clone())), false, NONCE + 1), 1),
        Verdict::Cancelled(x, _) if x == h
    ));
    assert!(matches!(
        single_rebuild::judge(seen(Some(own.clone()), None, false, NONCE + 1), 1),
        Verdict::Ours(_)
    ));
    assert!(matches!(
        single_rebuild::judge(seen(Some(own), Some((h, rc)), false, NONCE + 1), 1),
        Verdict::Odd(_)
    ));
    let Verdict::Odd(why) = single_rebuild::judge(seen(None, None, false, NONCE + 1), 2) else {
        panic!("nonce занят ни нашей, ни отменой — противоречие");
    };
    assert!(
        why.contains("не наша транзакция") && why.contains("(2)"),
        "{why}"
    );
    assert!(matches!(
        single_rebuild::judge(seen(None, None, true, NONCE + 1), 1),
        Verdict::Wait(_)
    ));
    assert!(matches!(
        single_rebuild::judge(seen(None, None, false, NONCE), 1),
        Verdict::Wait(_)
    ));
}

#[tokio::test]
async fn cancelled_equals_normal_path() {
    let c = ctx("cancelled").await;
    let (hash, _) = ours_rc();
    let (h, rc) = cancel_rc();
    let e = unknown(&c, hash, &[h]);
    let sc = ctx_of(&e);
    let late = single_rebuild::cancelled(&c, &sc, hash, (h, rc.clone()), "очередь кошелька снята")
        .expect_err("не исполнено");
    // Обычный путь: трекер нашёл квитанцию отмены (send/track_util.rs landed) → exec/result.rs → подпись.
    let t = Tracked {
        outcome: TxOutcome::NotIncluded,
        receipt: None,
        cancel: Some((h, rc.clone())),
        unresolved: None,
    };
    let d = txs::Done {
        chain: chain_tx::chain_tx(
            hash,
            NONCE,
            vec!["48club".into()],
            SendMethod::Single,
            &t,
            None,
        ),
        receipt: None,
        cancel: Some((h, rc.clone())),
        tip_bnb: None,
        cancels: vec![h],
    };
    let tr = cycle_steps::trade_request(&req());
    let normal = result::by(
        result::result(&c, tr, d, W1, (B1, B1), (DEC, DEC), &[]),
        BY_CYCLE_EXECUTOR,
    )
    .expect_err("обычный путь — не попала");
    assert_eq!(late.code, ErrorCode::TxNotIncluded);
    assert_eq!(late.code, normal.code);
    let js = |e: &GatewayError| serde_json::to_value(e.tx.as_deref()).unwrap();
    assert_eq!(js(&late), js(&normal), "факт транзакции тот же");
    let tx = late.tx.as_deref().unwrap();
    assert_eq!(tx.cancel_hash, Some(h));
    // Газ отмены — по квитанции: gasUsed × effectiveGasPrice.
    let wei = U256::from(rc.gas_used) * rc.effective_gas_price;
    assert_eq!(
        tx.cancel_gas_bnb,
        Some(Decimal::from_str(&wei.to_string()).unwrap() / Decimal::from(10u64.pow(18)))
    );
    assert!(
        late.message.starts_with(&format!("[{BY_CYCLE_EXECUTOR}]")),
        "{}",
        late.message
    );
    assert!(
        outcome_resolve::definitive(&late),
        "ядро: итог окончательный"
    );
    assert!(
        OpError::from(&late).cancel_confirmed(),
        "продажа — безопасный повтор (229)"
    );
    // Без отмены в факте TxNotIncluded окончательным не считается (ядро спросит ещё).
    let mut bare = late.clone();
    if let Some(t) = bare.tx.as_deref_mut() {
        t.cancel_hash = None;
    }
    assert!(!outcome_resolve::definitive(&bare));
}

/// Живое ядро: виды площадок, обязательные money.* и risk.* (как tests/bsc_dex_cycle_resolve.rs).
async fn core(name: &str) -> Core {
    let d = std::env::temp_dir().join(format!("arb-single-core-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![(VenueId("bsc_dex".into()), VenueKind::Dex)];
    for (n, x) in [
        ("money.min_profit_usd", "0.1"),
        ("money.min_profit_pct", "0.1"),
        ("money.max_per_trade_usd", "50"),
        ("money.max_per_coin_usd", "100"),
        ("money.max_in_transit_usd", "200"),
        ("risk.max_daily_loss_usd", "15"),
        ("risk.min_bnb_balance", "0.01"),
    ] {
        settings
            .set(n, Scope::Global, Some(json!(x)), "тест")
            .await
            .unwrap();
    }
    core
}

#[tokio::test]
async fn core_closes_cycle_with_cancel_gas() {
    let c = ctx("core").await;
    let core = core("cancel").await;
    let (hash, _) = ours_rc();
    let (h, rc) = cancel_rc();
    let e = unknown(&c, hash, &[h]);
    let sc = ctx_of(&e);
    let found = single_rebuild::cancelled(&c, &sc, hash, (h, rc), "очередь кошелька снята")
        .expect_err("не исполнено");
    let gas = found.tx.as_deref().unwrap().cancel_gas_bnb.unwrap();
    assert!(gas > Decimal::ZERO);
    let rq = req();
    let d = cycle_plan::direction(&rq.cycle);
    let bnb = rates().bnb_usdt;
    let mut deal = ledger::open(&core, &d, json!({}), Decimal::ZERO, None);
    cycle_deal::settle_unknown(&core, &mut deal, bnb, &e, Some(Err(found))).await;
    assert_eq!(deal.step, Step::Closed, "{:?}", deal.timeline);
    assert_eq!(deal.spent, gas * bnb, "газ отмены — в затраты");
    assert_eq!(
        (deal.invested, deal.proceeds),
        (Decimal::ZERO, Decimal::ZERO)
    );
}
