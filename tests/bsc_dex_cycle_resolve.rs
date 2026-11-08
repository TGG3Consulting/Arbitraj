//! Файл: tests/bsc_dex_cycle_resolve.rs
//! Что проверяет: решение 233 (дополнение, п. 4) — позднее выяснение исхода сделки цикла DEX ↔ DEX, на НАСТОЯЩИХ
//! квитанциях BSC и живом ядре (настоящая SQLite во временной папке, моков нет):
//! 1) одиночная транзакция цикла через исполнитель: «исход неизвестен» (Pending) несёт «чем найти» цикла (хеш, nonce,
//!    кошелёк, токены «базовая → базовая», decimals, чаевые, подпись, курсы сделки); итог по квитанции
//!    (exec/swap_rebuild.rs → exec/cycle_ref.rs finish) РАВЕН обычному пути цикла (exec/cycle_exec.rs: result::result →
//!    подпись → cycle_steps::finish). Квитанция — tests/data/receipts_2026-10-25/responses_3.json, блок 129703969,
//!    tx 0xff0fbd19… (индекс 54): отправитель 0x4bf2… отдал и получил один токен 0xe277… — взгляд теста: кошелёк цикла
//!    и его базовая монета;
//! 2) бандл цикла: «чем найти» с хешами всех транзакций, первым nonce, maxBlockNumber; «в блоке» (bundle_rebuild judge →
//!    rebuild → cycle_ref finish) РАВЕН обычному пути (exec/cycle_bundle_trade.rs: bundle_result → подпись → finish);
//!    «окончательно не исполнено» (срок бандла прошёл с запасом / nonce занят не нами) — итог с нулями, NotFilled,
//!    транзакции NotIncluded (core/cycle_outcome.rs not_executed). Квитанции — responses.json, блок 129702239, индексы
//!    5 и 6 (две транзакции подряд отправителя 0x0fe0…: USDT пришёл и ушёл) — взгляд теста: бандл цикла по USDT;
//! 3) нет курсов цикла в «чем найти» — NotApplicable (человек);
//! 4) живое ядро (core/cycle_deal.rs settle_unknown): «в блоке» — сделка Closed с учётом по факту, как исполненный цикл;
//!    «не исполнено» — Closed, вложено / выручка / затраты 0; не выяснено — Halted (ручная проверка), как раньше.
//!
//! Курсы, decimals и nonce — параметры теста (сверяется равенство двух путей и разбор ядра).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::exec::{bundle_rebuild, bundle_ref, bundle_send, bundle_trade, chain_tx,
//! cycle_ref, cycle_steps, cycle_trade, result, swap_ctx, swap_rebuild}, arb::core::{cycle_deal, cycle_outcome, ledger},
//! настоящая SQLite.

use std::str::FromStr;

use alloy_primitives::{Address, B256, address};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::Core;
use arb::core::deal::Step;
use arb::core::deps::Deps;
use arb::core::types::{
    CycleTradeRequest, CycleTradeResult, DexCycle, SendMethod, TxOutcome, VenueId,
};
use arb::core::{cycle_deal, cycle_outcome, cycle_plan, ledger};
use arb::gateway::contract::VenueKind;
use arb::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::bundle_rebuild::{self, Seen, Verdict};
use arb::venues::bsc_dex::exec::bundle_ref::BundleRef;
use arb::venues::bsc_dex::exec::bundle_send::Sent;
use arb::venues::bsc_dex::exec::cycle_trade::{BY_CYCLE_BUNDLE, BY_CYCLE_EXECUTOR};
use arb::venues::bsc_dex::exec::swap_ctx::{self, SwapCtx};
use arb::venues::bsc_dex::exec::{
    bundle_trade, chain_tx, cycle_ref, cycle_steps, result, swap_rebuild, txs,
};
use arb::venues::bsc_dex::quote::cycle_cost::Rates;
use arb::venues::bsc_dex::send::receipt::Receipt;
use arb::venues::bsc_dex::send::track_util::Tracked;

const R1: &str = "tests/data/receipts_2026-10-25/responses.json";
const R3: &str = "tests/data/receipts_2026-10-25/responses_3.json";
/// Одиночная: отправитель tx 0xff0fbd19… и его токен.
const W1: Address = address!("0x4bf2763aea6a607a3b77b19f3df069e9486d2db0");
const B1: Address = address!("0xe277b2762a4355bdb04da7b98833c8aa7dff8888");
/// Бандл: отправитель двух транзакций подряд блока 129702239 и USDT BSC.
const W2: Address = address!("0x0fe062123a013c0dc79e4ec0e5bdf332201d7b7e");
const USDT: Address = address!("0x55d398326f99059ff775485246999027b3197955");
const DEC: u8 = 18;
const NONCE: u64 = 77;
const MAX_BLOCK: u64 = 129_702_240;

/// Квитанции записи: results[res] — eth_getBlockReceipts, индексы транзакций. Паника — файла нет.
fn receipts(file: &str, res: usize, idx: &[usize]) -> Vec<(B256, Receipt)> {
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
    idx.iter()
        .map(|&i| {
            let r = rec["results"][res]["response"]["result"][i].clone();
            let h = B256::from_str(r["transactionHash"].as_str().unwrap()).unwrap();
            (h, serde_json::from_value(r).unwrap())
        })
        .collect()
}

/// Курсы сделки (параметр теста).
fn rates() -> Rates {
    Rates {
        base_usdt: Decimal::new(25, 2),
        bnb_usdt: Decimal::new(612, 0),
        bnb_in_base: Decimal::new(2448, 0),
    }
}

/// Запрос цикла ядра (hops сделке не нужны: своп «базовая → базовая» — cycle_steps::trade_request).
fn req(base: Address, ticker: &str, input: i64) -> CycleTradeRequest {
    let cycle = DexCycle {
        venue: VenueId("bsc_dex".into()),
        base,
        base_ticker: ticker.into(),
        input: Decimal::from(input),
        output: Decimal::from(input),
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
        deal_id: format!("20261108120000000{ticker}"),
        cycle,
        input: Decimal::from(input),
        min_out: Decimal::from(input + 1),
    }
}

/// Окружение площадки (тексты ошибок). Настоящая SQLite во временной папке; сеть не нужна.
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-cycle-resolve-{}-{tag}", std::process::id()));
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

/// Без отметки времени — сравнение двух путей.
fn plain(r: &CycleTradeResult) -> Value {
    let mut v = serde_json::to_value(r).unwrap();
    v["trade"]["finished_at"] = Value::Null;
    v
}

/// Ошибка как её отдаёт путь, затем JSON (так ядро хранит и передаёт ошибку). Ошибок нет.
fn through_json(e: GatewayError) -> GatewayError {
    serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap()
}

/// Одиночная транзакция цикла через исполнитель: (обычный путь, ошибка «исход неизвестен» как её отдаёт путь).
fn single(
    c: &Ctx,
) -> (
    GatewayResult<CycleTradeResult>,
    GatewayError,
    (B256, Receipt),
) {
    let (hash, rc) = receipts(R3, 2, &[54]).remove(0);
    let tip = Some(Decimal::new(1, 4));
    let rq = req(B1, "TKN", 18);
    let tr = || cycle_steps::trade_request(&rq);
    let ch = vec!["48club".to_string()];
    // Обычный путь: exec/txs.rs Done → exec/cycle_exec.rs result::result → exec/cycle_trade.rs подпись → finish.
    let landed = Tracked {
        outcome: TxOutcome::Included,
        receipt: Some(rc.clone()),
        cancel: None,
        unresolved: None,
    };
    let d = txs::Done {
        chain: chain_tx::chain_tx(hash, NONCE, ch.clone(), SendMethod::Single, &landed, tip),
        receipt: Some(rc.clone()),
        cancel: None,
        tip_bnb: tip,
        cancels: Vec::new(),
    };
    let t = result::result(c, tr(), d, W1, (B1, B1), (DEC, DEC), &[]);
    let normal = result::by(t, BY_CYCLE_EXECUTOR).map(|t| cycle_steps::finish(t, &rates()));
    // Путь при Pending: exec/txs.rs seed → exec/cycle_exec.rs cycle_ref::ctx attach → exec/cycle_trade.rs who.
    let pend = Tracked::bare(TxOutcome::Pending, Some("узел молчал".into()));
    let chain = chain_tx::chain_tx(hash, NONCE, ch, SendMethod::Single, &pend, tip);
    let e = c
        .err(ErrorCode::OutcomeUnknown, "исход неизвестен")
        .with_tx(chain.clone())
        .pending(swap_ctx::seed(&chain, tip));
    let e = cycle_ref::ctx(W1, B1, DEC, &rates()).attach(e);
    let e = through_json(swap_ctx::who(e, BY_CYCLE_EXECUTOR));
    (normal, e, (hash, rc))
}

/// Бандл цикла: (транзакции, обычный путь, ошибка «исход неизвестен» как её отдаёт путь).
fn bundle(
    c: &Ctx,
) -> (
    Vec<(B256, Receipt)>,
    GatewayResult<CycleTradeResult>,
    GatewayError,
) {
    let b = receipts(R1, 8, &[5, 6]);
    let rq = req(USDT, "USDT", 1);
    let tr = || cycle_steps::trade_request(&rq);
    let sent = |t: Tracked, rs: Vec<Receipt>| Sent {
        hashes: b.iter().map(|x| x.0).collect(),
        channels: vec!["48club".into()],
        tracked: t,
        receipts: rs,
        max_block: MAX_BLOCK,
    };
    let first = b[0].1.clone();
    let landed = Tracked {
        outcome: TxOutcome::Included,
        receipt: Some(first),
        cancel: None,
        unresolved: None,
    };
    let s = sent(landed, b.iter().map(|x| x.1.clone()).collect());
    let w = (W2, (USDT, USDT), (DEC, DEC), (NONCE, &[][..]));
    let t = bundle_trade::bundle_result(c, tr(), &s, w.0, w.1, w.2, w.3);
    let normal = result::by(t, BY_CYCLE_BUNDLE).map(|t| cycle_steps::finish(t, &rates()));
    let why = Some("итог бандла не установлен за send.queue_max_wait_s: узел молчал".into());
    let s = sent(Tracked::bare(TxOutcome::Pending, why), Vec::new());
    let mut sc = cycle_ref::ctx(W2, USDT, DEC, &rates());
    sc.bundle = Some(BundleRef::of(&s, NONCE));
    let e = bundle_trade::bundle_result(c, tr(), &s, w.0, w.1, w.2, w.3)
        .map_err(|e| sc.attach(e))
        .expect_err("исход неизвестен");
    let e = through_json(swap_ctx::who(e, BY_CYCLE_BUNDLE));
    (b, normal, e)
}

/// Контекст разбора из «чем найти» ошибки (как его читает resolve.rs / resolve_cycle.rs).
fn ctx_of(e: &GatewayError) -> SwapCtx {
    SwapCtx::of(e.pending.as_deref().expect("«чем найти»")).expect("контекст разбора")
}

#[tokio::test]
async fn single_cycle_late_equals_normal() {
    let c = ctx("single").await;
    let (normal, e, (hash, rc)) = single(&c);
    assert_eq!(e.code, ErrorCode::OutcomeUnknown);
    let p = e.pending.as_deref().unwrap();
    assert_eq!(
        p.venue_ref.as_deref(),
        Some(hash.to_string().as_str()),
        "ключ — хеш"
    );
    assert_eq!(p.nonce, Some(NONCE));
    let sc = ctx_of(&e);
    assert!(sc.complete(), "контекст свопа полный");
    assert_eq!((sc.wallet, sc.tokens), (Some(W1), Some((B1, B1))));
    assert_eq!(sc.cycle, Some(cycle_ref::CycleRef::of(&rates())));
    assert_eq!(sc.who.as_deref(), Some(BY_CYCLE_EXECUTOR));
    assert_eq!(sc.tip_bnb, Some(Decimal::new(1, 4)), "чаевые сохранены");
    // resolve_cycle.rs: resolve::trade → swap_rebuild::rebuild → cycle_ref::finish.
    let tr = cycle_steps::trade_request(&req(B1, "TKN", 18));
    let t = swap_rebuild::rebuild(&c, tr, &sc, hash, rc).expect("итог свопа");
    let late = cycle_ref::finish(&c, t, Some(&sc)).expect("итог цикла");
    let normal = normal.expect("обычный путь исполнен");
    assert_eq!(
        plain(&late),
        plain(&normal),
        "позднее выяснение = обычный путь"
    );
    assert!(
        late.output > late.input,
        "по записи пришло больше, чем ушло"
    );
    assert!(!cycle_outcome::not_executed(&late));
}

#[tokio::test]
async fn bundle_cycle_landed_and_not_executed() {
    let c = ctx("bundle").await;
    let (b, normal, e) = bundle(&c);
    assert_eq!(e.code, ErrorCode::OutcomeUnknown, "{}", e.message);
    let sc = ctx_of(&e);
    let br = sc.bundle.clone().expect("BundleRef");
    assert_eq!(
        br.hashes,
        b.iter().map(|x| x.0).collect::<Vec<_>>(),
        "хеши всех транзакций"
    );
    assert_eq!((br.first_nonce, br.max_block), (NONCE, MAX_BLOCK));
    assert!(sc.cycle.is_some() && sc.who.as_deref() == Some(BY_CYCLE_BUNDLE));
    let tr = || cycle_steps::trade_request(&req(USDT, "USDT", 1));
    let block = b[0].1.block_number.to::<u64>();
    // «В блоке»: обе квитанции, один блок, один статус.
    let seen = Seen {
        receipts: b.iter().map(|x| Some(x.1.clone())).collect(),
        in_block: false,
        nonce_latest: NONCE + 2,
        nonce_deep: NONCE + 2,
        head: block + 3,
        confirm: 1,
    };
    let Verdict::Landed(rs) = bundle_rebuild::judge(&br, &seen) else {
        panic!("ожидалось «в блоке»");
    };
    let t = bundle_rebuild::rebuild(&c, tr(), &sc, rs).expect("итог бандла");
    let late = cycle_ref::finish(&c, t, Some(&sc)).expect("итог цикла");
    assert_eq!(
        plain(&late),
        plain(&normal.expect("обычный путь")),
        "позднее = обычный путь"
    );
    // «Окончательно не исполнено»: квитанций нет, срок бандла прошёл с запасом.
    let gone = Seen {
        receipts: vec![None, None],
        in_block: false,
        nonce_latest: NONCE,
        nonce_deep: NONCE,
        head: MAX_BLOCK + 2,
        confirm: 1,
    };
    let Verdict::NotExecuted(why) = bundle_rebuild::judge(&br, &gone) else {
        panic!("ожидалось «не исполнено»");
    };
    let t = bundle_rebuild::not_executed(&c, tr(), &sc, &why).expect("итог «не исполнено»");
    let r = cycle_ref::finish(&c, t, Some(&sc)).expect("итог цикла");
    assert!(cycle_outcome::not_executed(&r), "ядро видит «не исполнено»");
    assert_eq!((r.input, r.output), (Decimal::ZERO, Decimal::ZERO));
    assert!(r.trade.fees.is_empty(), "газа нет");
    let by = r.trade.executor.clone().unwrap_or_default();
    assert!(
        by.starts_with(BY_CYCLE_BUNDLE) && by.contains("бандл не исполнен"),
        "{by}"
    );
}

#[tokio::test]
async fn cycle_without_rates_goes_to_human() {
    let c = ctx("norates").await;
    let (_, e, (hash, rc)) = single(&c);
    let mut sc = ctx_of(&e);
    sc.cycle = None;
    let tr = cycle_steps::trade_request(&req(B1, "TKN", 18));
    let t = swap_rebuild::rebuild(&c, tr, &sc, hash, rc).unwrap();
    let r = cycle_ref::finish(&c, t, Some(&sc));
    assert_eq!(r.unwrap_err().code, ErrorCode::NotApplicable);
}

/// Живое ядро (как tests/core_daily_venue.rs): виды площадок, обязательные money.* и risk.*.
async fn core(name: &str) -> Core {
    let d = std::env::temp_dir().join(format!("arb-cycle-core-{name}-{}", std::process::id()));
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
async fn core_closes_cycle_by_found_outcome() {
    let c = ctx("core-adapter").await;
    let core = core("found").await;
    let (_, normal, e) = bundle(&c);
    let landed = normal.expect("обычный путь");
    let rq = req(USDT, "USDT", 1);
    let d = cycle_plan::direction(&rq.cycle);
    let bnb = rates().bnb_usdt;

    // «В блоке» — как исполненный цикл: учёт по факту, Closed.
    let mut deal = ledger::open(&core, &d, json!({}), Decimal::ZERO, None);
    cycle_deal::settle_unknown(&core, &mut deal, bnb, &e, Some(Ok(landed.clone()))).await;
    assert_eq!(deal.step, Step::Closed);
    assert_eq!(deal.invested, landed.input * landed.base_usdt);
    assert_eq!(deal.proceeds, landed.output * landed.base_usdt);
    let gas = landed.trade.fee_in(arb::core::convert::GAS_TICKER);
    assert!(gas > Decimal::ZERO, "газ обеих транзакций");
    assert_eq!(
        deal.spent,
        gas * landed.bnb_usdt,
        "газ — один раз (не из исходной ошибки)"
    );

    // «Окончательно не исполнено» — Closed, денег не стоил.
    let sc = ctx_of(&e);
    let br = sc.bundle.clone().unwrap();
    let gone = Seen {
        receipts: vec![None, None],
        in_block: false,
        nonce_latest: NONCE + 1,
        nonce_deep: NONCE + 1,
        head: MAX_BLOCK,
        confirm: 1,
    };
    let Verdict::NotExecuted(why) = bundle_rebuild::judge(&br, &gone) else {
        panic!("nonce занят не нами — «не исполнено»");
    };
    let tr = cycle_steps::trade_request(&rq);
    let t = bundle_rebuild::not_executed(&c, tr, &sc, &why).unwrap();
    let r = cycle_ref::finish(&c, t, Some(&sc)).unwrap();
    let mut deal = ledger::open(&core, &d, json!({}), Decimal::ZERO, None);
    cycle_deal::settle_unknown(&core, &mut deal, bnb, &e, Some(Ok(r))).await;
    assert_eq!(deal.step, Step::Closed);
    assert_eq!(
        (deal.invested, deal.proceeds, deal.spent),
        (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO)
    );

    // Не выяснено — как раньше: ручная проверка.
    let mut deal = ledger::open(&core, &d, json!({}), Decimal::ZERO, None);
    cycle_deal::settle_unknown(&core, &mut deal, bnb, &e, None).await;
    assert_eq!(deal.step, Step::Halted);
}
