//! Файл: tests/bsc_dex_bundle_resolve.rs
//! Что проверяет: решение 233 — исход бандла свопов, без сети, на НАСТОЯЩИХ квитанциях BSC
//! (tests/data/receipts_2026-10-25/responses.json, eth_getBlockReceipts блока 129702236: шесть транзакций подряд
//! (индексы 2–7) с одного адреса 0x7068… — по порядку nonce, в одном блоке, как бандл; токен 0x394c… ушёл с адреса,
//! LP-токен 0x253a… пришёл на него). Кошелёк и стороны — взгляд теста (кошелёк = отправитель, продажа 0x394c… за
//! 0x253a…), decimals 18/18 — параметр теста: сверяется равенство двух путей.
//! 1) итог не установлен (Pending) → OutcomeUnknown (не TxNotIncluded) с «чем найти»: хеши всех транзакций, первый
//!    nonce, maxBlockNumber, кошелёк, токены; ядро не повторяет (OpError: не retry_safe, не cancel_confirmed);
//! 2) «в блоке», а квитанций меньше, чем транзакций — тоже OutcomeUnknown; штатное истечение — по-прежнему TxNotIncluded;
//! 3) позднее выяснение «в блоке» (exec/bundle_rebuild.rs) = обычный путь бандла (bundle_result + подпись);
//! 4) вердикты judge: противоречия (часть квитанций, разные блоки, разные статусы) — человек; ждём (срок не прошёл,
//!    узел знает транзакцию в блоке); «не исполнено» — после maxBlockNumber + send.replaced_confirm_blocks или nonce
//!    занят не нами; итог «не исполнено» — NotFilled без газа; откат всего бандла — SwapReverted, как обычный путь.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{ctx, exec, send}, arb::core::op_error, arb::gateway::errors, настоящая
//! SQLite во временной папке (Ctx: тексты ошибок с площадкой).

use std::str::FromStr;

use alloy_primitives::{Address, B256, U64, address};
use rust_decimal::Decimal;
use serde_json::Value;
use tokio::sync::broadcast;

use arb::core::op_error::OpError;
use arb::core::types::{Asset, Side, TradeRequest, TradeResult, TradeStatus, TxOutcome, VenueId};
use arb::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::bundle_rebuild::{self, Seen, Verdict};
use arb::venues::bsc_dex::exec::bundle_ref::BundleRef;
use arb::venues::bsc_dex::exec::bundle_send::Sent;
use arb::venues::bsc_dex::exec::swap_ctx::{self, SwapCtx};
use arb::venues::bsc_dex::exec::{bundle_trade, result};
use arb::venues::bsc_dex::send::receipt::Receipt;
use arb::venues::bsc_dex::send::track_util::Tracked;

const RECEIPTS: &str = "tests/data/receipts_2026-10-25/responses.json";
/// Отправитель шести транзакций подряд в блоке 129702236 — «кошелёк» теста.
const WALLET: Address = address!("0x7068593fe9bc3adbc71f966f360e54bb3fbb36ef");
/// Токен, ушедший с кошелька (tx индекса 3).
const COIN: Address = address!("0x394cf57908a95a84060cd55291ef70d80ac14444");
/// LP-токен, пришедший на кошелёк (tx индекса 5) — «USDT» теста.
const OUT: Address = address!("0x253a3bf69d952fab65bb4bee959c7d7a4f5a177e");
const DECS: (u8, u8) = (18, 18);
const FIRST_NONCE: u64 = 100;
const MAX_BLOCK: u64 = 129_702_238;

/// Квитанции из записи: results[i] — eth_getBlockReceipts, индексы транзакций. Паника — файла нет.
fn receipts(res: usize, idx: std::ops::Range<usize>) -> Vec<(B256, Receipt)> {
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(RECEIPTS).unwrap()).unwrap();
    idx.map(|i| {
        let r = rec["results"][res]["response"]["result"][i].clone();
        let h = B256::from_str(r["transactionHash"].as_str().unwrap()).unwrap();
        (h, serde_json::from_value(r).unwrap())
    })
    .collect()
}

/// Бандл теста: шесть транзакций кошелька (индексы 2–7 блока 129702236).
fn bundle() -> Vec<(B256, Receipt)> {
    receipts(2, 2..8)
}

/// Окружение площадки (тексты ошибок). Настоящая SQLite во временной папке; сеть не нужна.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-bundle-resolve-{}-{tag}", std::process::id()));
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

/// Продажа 1e9 монет (столько ушло с кошелька в записи).
fn req() -> TradeRequest {
    TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "COIN".into(),
            bsc_contract: Some(COIN),
        },
        side: Side::Sell,
        qty: Decimal::new(1_000_000_000, 0),
        worst_price: Decimal::new(1, 6),
        deal_id: "20261108120000000COIN".into(),
        route: None,
        exit_position: true,
        remeasure: None,
    }
}

/// Итог отправки, как его отдаёт exec/bundle_send.rs: хеши, каналы, итог трекера, квитанции.
fn sent(b: &[(B256, Receipt)], t: Tracked, rs: Vec<Receipt>) -> Sent {
    Sent {
        hashes: b.iter().map(|x| x.0).collect(),
        channels: vec!["48club".into()],
        tracked: t,
        receipts: rs,
        max_block: MAX_BLOCK,
    }
}

/// Обычный путь бандла: bundle_result + подпись исполнителя (exec/trade.rs).
fn normal(c: &Ctx, s: &Sent) -> GatewayResult<TradeResult> {
    let out =
        bundle_trade::bundle_result(c, req(), s, WALLET, (COIN, OUT), DECS, (FIRST_NONCE, &[]));
    result::by(out, result::BY_BUNDLE)
}

/// Ошибка «исход неизвестен», как её отдаёт путь (exec/bundle_trade.rs trade → exec/trade.rs): Pending → bundle_result
/// → контекст с BundleRef → подпись; затем JSON (так ядро хранит и передаёт ошибку).
fn failed(c: &Ctx, b: &[(B256, Receipt)]) -> GatewayError {
    let why = Some("итог бандла не установлен за send.queue_max_wait_s: узел молчал".to_string());
    let s = sent(b, Tracked::bare(TxOutcome::Pending, why), Vec::new());
    let mut sc = SwapCtx::swap(WALLET, (COIN, OUT), DECS, &[], None, None);
    sc.bundle = Some(BundleRef::of(&s, FIRST_NONCE));
    let e = normal(c, &s).map_err(|e| sc.attach(e)).unwrap_err();
    let e = swap_ctx::who(e, result::BY_BUNDLE);
    serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap()
}

/// Что видит узел: квитанции, голова, nonce.
fn seen(rs: Vec<Option<Receipt>>, head: u64, latest: u64, deep: u64) -> Seen {
    Seen {
        receipts: rs,
        in_block: false,
        nonce_latest: latest,
        nonce_deep: deep,
        head,
        confirm: 3,
    }
}

fn bref(b: &[(B256, Receipt)]) -> BundleRef {
    BundleRef {
        hashes: b.iter().map(|x| x.0).collect(),
        first_nonce: FIRST_NONCE,
        max_block: MAX_BLOCK,
    }
}

#[tokio::test]
async fn pending_is_outcome_unknown_with_bundle_ref() {
    let c = ctx("pending").await;
    let b = bundle();
    let e = failed(&c, &b);
    assert_eq!(e.code, ErrorCode::OutcomeUnknown);
    assert!(
        e.message
            .starts_with("[свой сборщик: бандл] Исход бандла неизвестен")
    );
    let tx = e.tx.as_deref().unwrap();
    assert_eq!(tx.outcome, TxOutcome::Pending);
    let p = e.pending.as_deref().unwrap();
    assert_eq!(p.venue_ref.as_deref(), Some(b[5].0.to_string().as_str()));
    let sc = SwapCtx::of(p).unwrap();
    assert!(sc.complete());
    let r = sc.bundle.unwrap();
    assert_eq!(r, bref(&b));
    assert_eq!(r.last_nonce(), FIRST_NONCE + 5);
    assert_eq!(r.last_hash(), b[5].0);
    // Ядро: исход неизвестен — не повторяет и не считает отмену подтверждённой.
    let op = OpError::from(&e);
    assert!(op.outcome_unknown() && !op.retry_safe() && !op.cancel_confirmed());
}

#[tokio::test]
async fn partial_receipts_unknown_expired_is_not_included() {
    let c = ctx("partial").await;
    let b = bundle();
    let first = b[0].1.clone();
    let t = Tracked {
        outcome: TxOutcome::Included,
        receipt: Some(first.clone()),
        cancel: None,
        unresolved: None,
    };
    let rs: Vec<Receipt> = b[..4].iter().map(|x| x.1.clone()).collect();
    let e = normal(&c, &sent(&b, t, rs)).unwrap_err();
    assert_eq!(e.code, ErrorCode::OutcomeUnknown);
    assert!(e.message.contains("квитанции 4 из 6"));
    // Штатное истечение (исход установлен, nonce свободны) — как раньше «не попала в блок».
    let e = normal(
        &c,
        &sent(&b, Tracked::bare(TxOutcome::NotIncluded, None), vec![]),
    )
    .unwrap_err();
    assert_eq!(e.code, ErrorCode::TxNotIncluded);
}

#[tokio::test]
async fn landed_rebuild_equals_normal_path() {
    let c = ctx("landed").await;
    let b = bundle();
    let all: Vec<Receipt> = b.iter().map(|x| x.1.clone()).collect();
    let t = Tracked {
        outcome: TxOutcome::Included,
        receipt: Some(all[0].clone()),
        cancel: None,
        unresolved: None,
    };
    let want = normal(&c, &sent(&b, t, all.clone())).unwrap();
    let sc = SwapCtx::of(failed(&c, &b).pending.as_deref().unwrap()).unwrap();
    let v = bundle_rebuild::judge(
        &bref(&b),
        &seen(all.iter().cloned().map(Some).collect(), 0, 0, 0),
    );
    let Verdict::Landed(rs) = v else {
        panic!("ждали «в блоке»: {v:?}")
    };
    let got = bundle_rebuild::rebuild(&c, req(), &sc, rs).unwrap();
    assert_eq!(got.status, TradeStatus::Filled);
    assert_eq!(got.filled_qty, Decimal::new(1_000_000_000, 0));
    assert_eq!(got.filled_qty, want.filled_qty);
    assert_eq!(got.avg_price, want.avg_price);
    assert_eq!(got.fees, want.fees);
    assert_eq!(got.executor, want.executor);
    assert_eq!(got.executor.as_deref(), Some(result::BY_BUNDLE));
    let (g, w) = (got.chain.unwrap(), want.chain.unwrap());
    assert_eq!(
        serde_json::to_value(&g).unwrap(),
        serde_json::to_value(&w).unwrap()
    );
    assert_eq!(
        (g.outcome, g.block, g.nonce),
        (TxOutcome::Included, Some(129702236), FIRST_NONCE)
    );
}

#[test]
fn odd_receipts_go_to_human() {
    let b = bundle();
    let r = bref(&b);
    let mut part: Vec<Option<Receipt>> = b.iter().map(|x| Some(x.1.clone())).collect();
    part[3] = None;
    assert!(
        matches!(bundle_rebuild::judge(&r, &seen(part, 0, 0, 0)), Verdict::Odd(w) if w.contains("5 из 6"))
    );
    // Последняя транзакция — из следующего блока (129702237) записи.
    let mut blocks: Vec<Option<Receipt>> = b.iter().map(|x| Some(x.1.clone())).collect();
    blocks[5] = Some(receipts(4, 0..1).remove(0).1);
    assert!(
        matches!(bundle_rebuild::judge(&r, &seen(blocks, 0, 0, 0)), Verdict::Odd(w) if w.contains("разных блоках"))
    );
    let mut mixed: Vec<Option<Receipt>> = b.iter().map(|x| Some(x.1.clone())).collect();
    if let Some(x) = mixed[2].as_mut() {
        x.status = U64::from(0u8);
    }
    assert!(
        matches!(bundle_rebuild::judge(&r, &seen(mixed, 0, 0, 0)), Verdict::Odd(w) if w.contains("откатилась"))
    );
}

#[tokio::test]
async fn not_landed_wait_or_final() {
    let c = ctx("final").await;
    let b = bundle();
    let r = bref(&b);
    let none = || vec![None; 6];
    // Срок не прошёл (голова ≤ maxBlockNumber + 3), nonce свободен — ждём.
    assert!(matches!(
        bundle_rebuild::judge(&r, &seen(none(), MAX_BLOCK + 3, FIRST_NONCE, FIRST_NONCE)),
        Verdict::Wait(_)
    ));
    // Узел знает транзакцию в блоке без квитанции — ждём, даже после срока.
    let mut s = seen(none(), MAX_BLOCK + 10, FIRST_NONCE, FIRST_NONCE);
    s.in_block = true;
    assert!(matches!(bundle_rebuild::judge(&r, &s), Verdict::Wait(_)));
    // Nonce занят не нами только на «latest» (недавно) — ещё ждём; занят не позже «голова − 3» — окончательно.
    let recent = seen(none(), MAX_BLOCK, FIRST_NONCE + 1, FIRST_NONCE);
    assert!(matches!(
        bundle_rebuild::judge(&r, &recent),
        Verdict::Wait(_)
    ));
    let deep = seen(none(), MAX_BLOCK, FIRST_NONCE + 1, FIRST_NONCE + 1);
    assert!(
        matches!(bundle_rebuild::judge(&r, &deep), Verdict::NotExecuted(w) if w.contains("занят не транзакцией"))
    );
    // Срок прошёл с запасом, квитанций нет — окончательно «не исполнено».
    let v = bundle_rebuild::judge(&r, &seen(none(), MAX_BLOCK + 4, FIRST_NONCE, FIRST_NONCE));
    let Verdict::NotExecuted(why) = v else {
        panic!("ждали «не исполнено»: {v:?}")
    };
    assert!(why.contains("бандл истёк") && why.contains("свободен"));
    let sc = SwapCtx::of(failed(&c, &b).pending.as_deref().unwrap()).unwrap();
    let t = bundle_rebuild::not_executed(&c, req(), &sc, &why).unwrap();
    assert_eq!(t.status, TradeStatus::NotFilled);
    assert!(t.filled_qty.is_zero() && t.fees.is_empty());
    let tx = t.chain.unwrap();
    assert_eq!(
        (tx.outcome, tx.block, tx.gas_bnb, tx.hash),
        (TxOutcome::NotIncluded, None, None, b[5].0)
    );
    assert!(
        t.executor
            .unwrap()
            .starts_with("свой сборщик: бандл; бандл не исполнен (решение 233)")
    );
}

#[tokio::test]
async fn whole_bundle_reverted_is_swap_reverted() {
    let c = ctx("revert").await;
    let b = bundle();
    let all: Vec<Receipt> = b
        .iter()
        .map(|x| {
            let mut r = x.1.clone();
            r.status = U64::from(0u8);
            r
        })
        .collect();
    let t = Tracked {
        outcome: TxOutcome::Reverted,
        receipt: Some(all[0].clone()),
        cancel: None,
        unresolved: None,
    };
    let want = normal(&c, &sent(&b, t, all.clone())).unwrap_err();
    let sc = SwapCtx::of(failed(&c, &b).pending.as_deref().unwrap()).unwrap();
    let Verdict::Landed(rs) = bundle_rebuild::judge(
        &bref(&b),
        &seen(all.into_iter().map(Some).collect(), 0, 0, 0),
    ) else {
        panic!("ждали «в блоке»")
    };
    let got = bundle_rebuild::rebuild(&c, req(), &sc, rs).unwrap_err();
    assert_eq!(got.code, ErrorCode::SwapReverted);
    assert_eq!((got.code, &got.message), (want.code, &want.message));
}
