//! Файл: tests/bsc_dex_prefetch_325_review.rs
//! Что делает: ревью решения 325 (задача t152) на РЕАЛЬНЫХ ответах узла BSC — запись
//! tests/data/algebra_integral_2026-11-06/probe.json (блок 132036268; пулы Algebra Integral thena-v3 — pluginConfig 193,
//! флаг DYNAMIC_FEE есть — и dexswap-bsc — pluginConfig 0; data и raw каждого вызова записаны узлом):
//! 1. Байты вызовов плана Algebra Integral (prefetch_integral first: globalState, liquidity, nextTickGlobal,
//!    prevTickGlobal; ticks() тиков обхода) — ровно записанные данные запросов бота; fee() в плане — только при
//!    DYNAMIC_FEE по записанному globalState (Н5).
//! 2. Слияние частичных попаданий книги (node/multicall.rs merge): ответы книги и узла встают по порядку вызовов,
//!    откат узла — на своём месте, ответов узла меньше — ошибка у недостающего.
//! 3. Книга есть только внутри scope (prefetch::active, Н1: без книги aggregate идёт к узлу сразу).
//! 4. Отбор вызовов обращения (prefetch_ask pick, Н5): частей не больше нужных вызовов при любом размере части.
//! 5. KyberSwap Elastic: план без ticks() (их код чтения берёт одним view_many), memo у Elastic и Integral не берётся.
//! 6. «Не задано = как раньше»: dex.event_refresh_batch не задана — prefetch_read::size = None, настройки в ячейке
//!    памяти (event_flags.rs) — «нет»; смена настройки в хранилище подхватывается на лету.
//! 7. «Только комиссия» (integral_fee fits / set_fee): только у достоверного пула (Н2), только с состоянием Algebra
//!    (Н3), не в очереди повтора; set_fee не у Algebra — запись не меняется и событие не отмечается учтённым.
//!
//! Чего здесь нет (нужна запись aggregate3 с узла, docs/TASKS.md решение 325): равенство ответа внутри aggregate3 и
//! отдельного eth_call по байтам у каждого вида; настоящие ответы tickTable / tickTreeSecondLayer.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/algebra_integral_common (запись), tests/native_common (entry), arb::settings (настоящая
//! SQLite во временной папке), arb::venues::bsc_dex::{node::{multicall, prefetch}, pools::{prefetch_*, integral_fee,
//! event_flags}}.
//! Настройки теста: dex.event_refresh_fee_only, dex.event_refresh_batch (область «DEX») — по случаю.

mod algebra_integral_common;
mod native_common;

use std::time::Duration;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, Bytes};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{PoolRef, PoolState};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::node::multicall::merge;
use arb::venues::bsc_dex::node::prefetch::{self, Book, Call};
use arb::venues::bsc_dex::pools::abi_algebra_integral::IAlgebraIntegralPool as P;
use arb::venues::bsc_dex::pools::abi_elastic::IElasticPool;
use arb::venues::bsc_dex::pools::load_cl::i24;
use arb::venues::bsc_dex::pools::prefetch_ask::{Ask, pick};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::{
    event_flags, integral_fee, prefetch_integral, prefetch_plan, prefetch_read,
};

use algebra_integral_common::{pool_state, probe};

/// Окружение на настоящей SQLite без настроек. Ошибок нет (тест падает).
async fn ctx(tag: &str) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-prefetch-325-{tag}-{}", std::process::id()));
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

/// Записать (Some) или снять (None) настройку в области «DEX». Ошибок нет (тест падает).
async fn set_dex(c: &Ctx, name: &str, id: &str, v: Option<Value>) {
    c.settings
        .set(name, Scope::Dex(id.into()), v, "тест")
        .await
        .unwrap();
}

/// Ждёт, пока условие станет истинным (ячейка настройки перечитывается задачей по рассылке изменений).
async fn until(f: impl Fn() -> bool) -> bool {
    for _ in 0..300 {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    f()
}

fn hex(v: &Value) -> Bytes {
    Bytes::from(
        alloy_primitives::hex::decode(v.as_str().unwrap().trim_start_matches("0x")).unwrap(),
    )
}

fn pool_of(p: &Value) -> Address {
    p["pool"].as_str().unwrap().parse().unwrap()
}

/// Записанный вызов пула: (контракт, данные запроса). Ошибок нет (тест падает).
fn call(p: &Value, name: &str) -> Call {
    (pool_of(p), hex(&p["calls"][name]["data"]))
}

/// Записанный ответ вызова. Ошибок нет (тест падает).
fn raw(p: &Value, name: &str) -> Bytes {
    hex(&p["calls"][name]["raw"])
}

/// Пул записи записью реестра (вид algebra_integral): состояние и карта — как собирает бот (algebra_integral_common).
fn integral(p: &Value) -> Entry {
    let (sqrt_price_x96, tick, liquidity, fee, map) = pool_state(p);
    let mut e = native_common::entry("algebra_integral", Some(fee));
    e.pool.reference = PoolRef::Contract(pool_of(p));
    e.pool.state = PoolState::Algebra {
        sqrt_price_x96,
        tick,
        liquidity,
        fee,
        fee_otz: None,
    };
    e.ticks = Some(map);
    e
}

/// Флаг DYNAMIC_FEE в записанном globalState (поле pluginConfig — 4-е слово ответа).
fn dynamic(p: &Value) -> bool {
    let gs = P::globalStateCall::abi_decode_returns(&raw(p, "globalState")).unwrap();
    integral_fee::dynamic(gs.pluginConfig)
}

#[tokio::test]
async fn integral_plan_bytes_equal_recorded_requests() {
    let c = ctx("bytes").await;
    let d = probe();
    for p in d["pools"].as_array().unwrap() {
        let e = integral(p);
        let ask = prefetch_integral::first(&e, pool_of(p));
        // Нужное — ровно поодиночные вызовы головы кода чтения (байты записанных запросов бота).
        let head = [
            "globalState",
            "liquidity",
            "nextTickGlobal",
            "prevTickGlobal",
        ];
        let want: Vec<Call> = head.iter().map(|n| call(p, n)).collect();
        assert_eq!(ask.useful, want, "{}", p["pool"]);
        // Вспомогательное: второй слой (28 узлов) и листы записи.
        assert_eq!(ask.extra.len(), 2);
        assert_eq!(ask.extra[0].len(), prefetch_integral::nodes().count());
        // ticks() не читаются заранее в first (ревью 325, Н5).
        let sel = P::ticksCall::SELECTOR;
        assert!(ask.extra.iter().flatten().all(|(_, x)| x[..4] != sel));
        // ticks() тиков обхода — те же байты, что записанные запросы обхода списка.
        for side in ["ticks_up", "ticks_down"] {
            for t in p[side].as_array().unwrap() {
                let tick = i24(&c, t["tick"].as_i64().unwrap() as i32).unwrap();
                let data = Bytes::from(P::ticksCall { tick }.abi_encode());
                assert_eq!(data, hex(&t["data"]), "{} {side}", p["pool"]);
            }
        }
    }
}

#[tokio::test]
async fn integral_more_reads_fee_only_with_dynamic_fee() {
    // dex.integral_tick_steps не задана — цепочки нет, остаётся только правило комиссии.
    let c = ctx("fee-rule").await;
    let d = probe();
    let mut seen = (false, false);
    for p in d["pools"].as_array().unwrap() {
        let pool = pool_of(p);
        let book = Book::new(d["block_hex"].as_str().unwrap());
        let names = ["globalState", "nextTickGlobal", "prevTickGlobal"];
        let calls: Vec<Call> = names.iter().map(|n| call(p, n)).collect();
        let answers: Vec<Result<Bytes, String>> = names.iter().map(|n| Ok(raw(p, n))).collect();
        book.fill(&calls, &answers);
        let ask = prefetch_integral::more(&c, pool, &book);
        let fee = (pool, Bytes::from(P::feeCall {}.abi_encode()));
        if dynamic(p) {
            assert_eq!(ask.useful, vec![fee.clone()], "{}", p["pool"]);
            seen.0 = true;
        } else {
            assert!(ask.useful.is_empty(), "{}", p["pool"]);
            seen.1 = true;
        }
        // Записанный запрос fee() бота — те же байты.
        assert_eq!(fee, call(p, "fee"));
    }
    assert_eq!(seen, (true, true), "в записи есть пулы с DYNAMIC_FEE и без");
}

#[test]
fn merge_puts_book_and_node_answers_in_call_order() {
    let d = probe();
    let p = &d["pools"][0];
    let (gs, liq, fee, next) = (
        raw(p, "globalState"),
        raw(p, "liquidity"),
        raw(p, "fee"),
        raw(p, "nextTickGlobal"),
    );
    let known = vec![Some(gs.clone()), None, Some(fee.clone()), None];
    let got = vec![Ok(liq.clone()), Ok(next.clone())];
    assert_eq!(
        merge(known.clone(), got),
        vec![
            Ok(gs.clone()),
            Ok(liq.clone()),
            Ok(fee.clone()),
            Ok(next.clone())
        ]
    );
    // Откат у узла — на своём месте.
    let got = vec![Err("откат".to_string()), Ok(next.clone())];
    assert_eq!(
        merge(known.clone(), got),
        vec![
            Ok(gs.clone()),
            Err("откат".into()),
            Ok(fee.clone()),
            Ok(next)
        ]
    );
    // Ответов узла меньше — ошибка у недостающего, порядок прочих прежний.
    let short = merge(known, vec![Ok(liq.clone())]);
    assert_eq!(short[..3], [Ok(gs), Ok(liq.clone()), Ok(fee)]);
    assert!(short[3].is_err());
    // В книге ничего — ответы узла как есть.
    assert_eq!(merge(vec![None], vec![Ok(liq.clone())]), vec![Ok(liq)]);
}

#[tokio::test]
async fn book_is_active_only_inside_scope() {
    assert!(!prefetch::active());
    let inside = prefetch::scope(Book::new("0x1"), async { prefetch::active() }).await;
    assert!(inside);
    assert!(!prefetch::active());
}

#[test]
fn pick_keeps_parts_not_above_useful_calls() {
    let d = probe();
    let p = &d["pools"][0];
    let e = integral(p);
    let pool = pool_of(p);
    let tag = d["block_hex"].as_str().unwrap();
    let ask = || prefetch_integral::first(&e, pool);
    let (useful, layer, leaves) = {
        let a = ask();
        (a.useful.len(), a.extra[0].len(), a.extra[1].len())
    };
    assert!(leaves > 0, "у записи есть листы тиков");
    // Часть по 1 вызову: вспомогательное не помещается — только голова (4 части = 4 снятых обращения).
    assert_eq!(pick(&Book::new(tag), ask(), 1).len(), useful);
    // Часть вмещает голову и второй слой в `useful` частей, но не листы.
    let part = (useful + layer).div_ceil(useful);
    if (useful + layer + leaves).div_ceil(part) > useful {
        assert_eq!(pick(&Book::new(tag), ask(), part).len(), useful + layer);
    }
    // Часть на всё — всё одним обращением.
    let all = useful + layer + leaves;
    assert_eq!(pick(&Book::new(tag), ask(), all).len(), all);
    // Нужных нет (уже спрошены) — обращения нет, даже если есть вспомогательное.
    let book = Book::new(tag);
    let a = ask();
    let oks: Vec<Result<Bytes, String>> = a.useful.iter().map(|_| Err("откат".into())).collect();
    book.fill(&a.useful, &oks);
    assert!(pick(&book, ask(), all).is_empty());
    assert!(pick(&Book::new(tag), Ask::default(), all).is_empty());
}

#[tokio::test]
async fn elastic_plan_leaves_ticks_to_one_view_many() {
    let c = ctx("elastic").await;
    let mut e = native_common::entry("kyber_elastic", None);
    // Ссылка записи — нулевой адрес (native_common::entry): проверяются только вызовы плана.
    let pool = Address::ZERO;
    e.pool.reference = PoolRef::Contract(pool);
    let ask = prefetch_plan::first(&c, &e);
    // dex.tick_reader не задана — только голова (getTicksInRange без читателя не строится).
    let head = vec![
        (
            pool,
            Bytes::from(IElasticPool::getPoolStateCall {}.abi_encode()),
        ),
        (
            pool,
            Bytes::from(IElasticPool::getLiquidityStateCall {}.abi_encode()),
        ),
        (
            pool,
            Bytes::from(IElasticPool::swapFeeUnitsCall {}.abi_encode()),
        ),
    ];
    assert_eq!(ask.useful, head);
    assert!(ask.extra.is_empty());
    let more = prefetch_plan::more(&c, &e, &Book::new("0x1"));
    assert!(more.useful.is_empty() && more.extra.is_empty());
    assert!(!prefetch_plan::memo(&e));
    assert!(!prefetch_plan::memo(&native_common::entry(
        "algebra_integral",
        None
    )));
    assert!(prefetch_plan::memo(&native_common::entry(
        "pancakeswap_v2",
        None
    )));
}

#[tokio::test]
async fn batch_not_set_is_as_before() {
    let c = ctx("batch").await;
    let e = native_common::entry("thena_fusion", None);
    assert!(!event_flags::batch(&c, e.family));
    assert_eq!(prefetch_read::size(&c, &e), None);
    set_dex(&c, event_flags::BATCH, "thena_fusion", Some(json!(true))).await;
    assert!(
        until(|| event_flags::batch(&c, e.family)).await,
        "смена на лету"
    );
    // dex.multicall3 / dex.load_batch не заданы — путь не применяется.
    assert_eq!(prefetch_read::size(&c, &e), None);
    set_dex(&c, event_flags::BATCH, "thena_fusion", None).await;
    assert!(
        until(|| !event_flags::batch(&c, e.family)).await,
        "снятие на лету"
    );
}

#[tokio::test]
async fn fee_only_needs_valid_algebra_pool() {
    let c = ctx("fee-only").await;
    let d = probe();
    let e = integral(&d["pools"][0]);
    let id = e.family.id;
    // Не задана — прежний Refresh.
    assert!(!integral_fee::fits(&c, &e, false, false));
    set_dex(&c, event_flags::FEE_ONLY, id, Some(json!(true))).await;
    assert!(
        until(|| integral_fee::fits(&c, &e, false, false)).await,
        "смена на лету"
    );
    // Пул ждёт повтора (решение 298) — прежний путь.
    assert!(!integral_fee::fits(&c, &e, true, false));
    // Н2: недостоверный пул — полная перечитка (она вернёт достоверность).
    let mut bad = e.clone();
    bad.valid = false;
    assert!(!integral_fee::fits(&c, &bad, false, false));
    // Н3: состояние не Algebra — комиссию некуда записать, полная перечитка.
    let mut other = e.clone();
    other.pool.state = native_common::entry("algebra_integral", None).pool.state;
    assert!(!integral_fee::fits(&c, &other, false, false));
    set_dex(&c, event_flags::FEE_ONLY, id, Some(json!(false))).await;
    assert!(
        until(|| !integral_fee::fits(&c, &e, false, false)).await,
        "«нет» на лету"
    );
}

#[test]
fn set_fee_writes_only_algebra_state() {
    let d = probe();
    let p = &d["pools"][0];
    let fee = u32::from(P::feeCall::abi_decode_returns(&raw(p, "fee")).unwrap());
    let block = d["block"].as_u64().unwrap();
    let mut e = integral(p);
    e.synced_block = block - 1;
    assert!(integral_fee::set_fee(&mut e, fee, block, Some(7)));
    assert!(matches!(e.pool.state, PoolState::Algebra { fee: f, .. } if f == fee));
    assert_eq!(
        (e.synced_block, e.synced_log, e.pool.block),
        (block, Some(7), block)
    );
    // Н3: не Algebra — запись не меняется, событие не отмечается учтённым.
    let mut v2 = native_common::entry("algebra_integral", None);
    let before = (v2.synced_block, v2.synced_log, v2.pool.block);
    assert!(!integral_fee::set_fee(&mut v2, fee, block, Some(7)));
    assert_eq!((v2.synced_block, v2.synced_log, v2.pool.block), before);
}
