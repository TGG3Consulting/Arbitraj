//! Файл: tests/core_sell_turn_t200.rs
//! Что делает: решение 415 (t200) — очередь продажи счёта (core/sell_turn.rs, core/sell_turn_fresh.rs) на настоящем
//! ядре и SQLite во временной папке (площадок в реестре нет — как tests/core_coin_reentry_t179.rs, tests/core_sell_unsent.rs).
//! Живой случай 19.11.2026 (данные бота — хронологии сделок): кошелёк DEX w1, монета LQR, BingX → bsc_dex, одна группа
//! докупки. A 20261119085840767LQR — зачислено 7316.64153, продано по 0.1183486421559381922735799249; B
//! 20261119085843064LQR — зачислено 5371.95816, котировка 0.1184877284330296105213894667 (пул ДО продажи A), в пуле
//! после A — 0.1174479740659173351024386981 («хуже допустимой» → продажа не ушла), затем «Своих монет на счёте не видно:
//! … на DEX (кошелёк w1) сейчас 5372 LQR, но по учёту 7317 LQR … принадлежат другим сделкам: сделка A».
//! Проверяется:
//! 1) lqr_b_quotes_after_a_sale: B ждёт хода, пока A котирует и продаёт; после итога A — котировка B только по пулу после
//!    A (котировка по блоку до её блока отвергается, ждём тика пула): средняя B — 0.11744797…, не 0.11848772…;
//!    минимальная цена B от неё ниже цены пула после A — продажа проходит (по прежней котировке минимальная цена выше —
//!    отказ, как вживую); учёт A в памяти уже 0 — «своих монет не видно» нет (сверка 5372 оставляет B её 5371.95816);
//!    учёт обеих после продаж — 0.
//! 2) other_keys_run_in_parallel: другие монеты и другие площадки ход не ждут; та же монета той же площадки — по
//!    одной; след продажи — после исполнения и (ревью t200, С-2) после исхода «неизвестен» (конец — сейчас, блок
//!    неизвестен, пулы из запроса); маршрут следа известен, только если exec.via_kyber не «да» (С-3).
//! 3) continuation_does_not_hold_turn: путь sell::sell_or_stuck без котировки (площадки нет) → режим застрявшей монеты
//!    «Ожидание» (минуты): ход свободен всё время ожидания, котировка-решение на тике берёт ход и сразу снимает.
//!
//! Настоящий путь продажи двух сделок (sell.rs, стенд XT) — tests/core_sell_turn_live_t200.rs.
//! Блоки и адрес пула в записях случая нет — это метки теста (номер 100 / 101, адрес-метка), как в t179 (route(PAIR,
//! 100)); exec.dex_slippage_pct = 0,5 — по числам случая (0.117895 / 0.11848772843 ≈ 0,995), feed.stale_after_ms —
//! параметр теста.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (sell_turn, sell_turn_fresh, credit_lock, remain_others, remain_fact, worst, sell,
//! ledger), arb::settings, arb::storage, arb::notify; tests/t198_live (big).

mod t198_live;

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use alloy_primitives::{Address, B256};
use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deal_group::DealGroup;
use arb::core::deps::Deps;
use arb::core::op_error::OpError;
use arb::core::remain_others::{Own, on_account, own_qty};
use arb::core::sell_turn::{self, Last};
use arb::core::sell_turn_fresh::{self, Quoted, Use};
use arb::core::types::{
    Asset, ChainTx, DexAccountId, DexRoute, Direction, PoolKind, PoolRef, Quote, RouteHop,
    RouteLeg, RouteMode, SendMethod, Side, TradeRequest, TradeResult, TradeStatus, TxOutcome,
    VenueId,
};
use arb::core::{credit_lock, ledger, worst};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// A: зачислено и продано (хронология A).
const A_QTY: &str = "7316.64153";
/// A: средняя цена продажи (хронология A).
const A_SOLD: &str = "0.1183486421559381922735799249";
/// B: зачислено (хронология B).
const B_QTY: &str = "5371.95816";
/// B: котировка продажи, взятая по пулу до продажи A (хронология B).
const B_PRE: &str = "0.1184877284330296105213894667";
/// B: цена пула для её количества после продажи A (ошибка «Цена в пуле сейчас … хуже допустимой»).
const B_POST: &str = "0.1174479740659173351024386981";
/// Факт кошелька w1 после продажи A (текст алерта: «сейчас 5372 LQR»).
const W1_AFTER_A: &str = "5372";
/// Блок продажи A — метка теста (в записях случая блока нет).
const A_BLOCK: u64 = 100;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn dex() -> VenueId {
    VenueId("bsc_dex".into())
}

/// Монета теста (свой тикер в каждом тесте: замки очереди — общие на процесс).
fn coin(t: &str) -> Asset {
    Asset {
        chain: None,
        ticker: t.into(),
        contract: None,
    }
}

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-t200-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе во временной папке; bingx — биржа, bsc_dex — DEX. Возвращает ядро, настройки, журнал.
async fn start(name: &str) -> (Core, Store, Journal) {
    let d = dir(name);
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
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.clone(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![
        (VenueId("bingx".into()), VenueKind::Cex),
        (dex(), VenueKind::Dex),
    ];
    (core, settings, journal)
}

/// Настройка площадки bsc_dex.
async fn set_dex(s: &Store, name: &str, v: Value) {
    s.set(name, Scope::Venue("bsc_dex".into()), Some(v), "тест")
        .await
        .unwrap();
}

/// Настройка всей системы.
async fn set(s: &Store, name: &str, v: Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

/// Сделка BingX → bsc_dex с монетами на кошельке w1 (продажа), группа докупки «A».
fn deal(id: &str, a: &Asset, qty: &str) -> Deal {
    let dx = Direction {
        asset: a.clone(),
        buy_on: VenueId("bingx".into()),
        sell_on: dex(),
    };
    let mut x = Deal::new(id.into(), dx, json!({}), d("1"));
    x.group = Some(Box::new(DealGroup::new("20261119085840767LQR")));
    x.at = dex();
    x.qty = d(qty);
    x.step = Step::Selling;
    x.dex_account = Some(DexAccountId("w1".into()));
    x
}

/// Пул маршрута — метка теста (адреса пула LQR в записях случая нет).
fn pool() -> PoolRef {
    PoolRef::Contract(Address::repeat_byte(0x4c))
}

/// Прямой маршрут через пул по состоянию блока.
fn route(block: u64) -> DexRoute {
    DexRoute {
        mode: RouteMode::Direct,
        legs: vec![RouteLeg {
            share: Decimal::ONE,
            hops: vec![RouteHop {
                family: "pancakeswap_v2".into(),
                kind: PoolKind::V2,
                pool: pool(),
            }],
            expected_out: Decimal::ZERO,
        }],
        block,
        gas_units: 0,
        executable: true,
        exec_note: None,
    }
}

/// Котировка продажи DEX на количество по средней, маршрут по блоку.
fn quote(a: &Asset, qty: Decimal, avg: Decimal, block: u64) -> Quote {
    Quote {
        venue: dex(),
        asset: a.clone(),
        side: Side::Sell,
        qty,
        fillable_qty: qty,
        avg_price: avg,
        costs: vec![],
        route: Some(route(block)),
        signal_route: None,
        pick: None,
        at: Utc::now(),
    }
}

/// Исполненная продажа сделки на bsc_dex через пул в блоке.
fn sold(id: &str, a: &Asset, qty: Decimal, avg: Decimal, block: u64) -> TradeResult {
    TradeResult {
        request: TradeRequest {
            venue: dex(),
            asset: a.clone(),
            side: Side::Sell,
            qty,
            worst_price: avg,
            deal_id: id.into(),
            route: Some(route(block - 1)),
            exit_position: true,
            remeasure: None,
        },
        status: TradeStatus::Filled,
        filled_qty: qty,
        avg_price: avg,
        fees: vec![],
        venue_ref: id.into(),
        chain: Some(ChainTx {
            hash: B256::ZERO,
            nonce: 0,
            channels: vec![],
            method: SendMethod::Single,
            outcome: TxOutcome::Included,
            block: Some(block),
            gas_used: None,
            gas_bnb: None,
            tip_bnb: None,
            cancel_hash: None,
            cancel_gas_bnb: None,
            approve_gas_bnb: None,
            tip_hash: None,
            tip_gas_bnb: None,
        }),
        finished_at: Utc::now(),
        executor: None,
    }
}

/// Количество сделки в учёте памяти ядра.
fn qty_of(core: &Core, id: &str) -> Decimal {
    core.inner.deals.lock().unwrap().get(id).unwrap().qty
}

#[tokio::test]
async fn lqr_b_quotes_after_a_sale() {
    let (core, s, j) = start("lqr").await;
    set_dex(&s, "feed.stale_after_ms", json!(10_000)).await;
    set_dex(&s, "exec.dex_slippage_pct", json!("0.5")).await;
    let lqr = coin("LQR");
    let a = deal("20261119085840767LQR", &lqr, A_QTY);
    let b = deal("20261119085843064LQR", &lqr, B_QTY);
    ledger::save(&core, &a).await;
    ledger::save(&core, &b).await;
    // Состояние пулов у адаптера (блок, по которому он котирует): до продажи A.
    let pool_block = Arc::new(AtomicU64::new(A_BLOCK - 1));
    // A: ход очереди — её котировка и ордер.
    let ta = sell_turn::take(&dex(), &lqr).await;
    let got = Arc::new(AtomicBool::new(false));
    let (c, bb, pb, g, l) = (
        core.clone(),
        b.clone(),
        pool_block.clone(),
        got.clone(),
        lqr.clone(),
    );
    let h = tokio::spawn(async move {
        let mut t = sell_turn::take(&dex(), &l).await;
        g.store(true, Ordering::SeqCst);
        let qty = bb.qty;
        let (asset, blocks) = (l.clone(), pb);
        let f = move || {
            let blk = blocks.load(Ordering::SeqCst);
            // Котировка адаптера по своему состоянию: до блока A — пул до её продажи, с блока A — после.
            let avg = if blk >= A_BLOCK { d(B_POST) } else { d(B_PRE) };
            let r: Quoted = Ok((qty * avg, quote(&asset, qty, avg, blk), None));
            async move { r }
        };
        let (_, q, _) = sell_turn_fresh::quote_with(&c, &bb, &mut t, Use::Sale, f)
            .await
            .unwrap();
        let worst = worst::worst_price(&c, &dex(), &l, Side::Sell, qty, q.avg_price).unwrap();
        // Проверка адаптера перед отправкой (exec/order.rs check_price): цена пула сейчас (после A) не хуже допустимой.
        let now = d(B_POST);
        let (id, l2) = (bb.id.clone(), l.clone());
        let order = async move {
            if now < worst {
                Err(OpError::from(format!(
                    "Цена в пуле сейчас {now} хуже допустимой {worst}"
                )))
            } else {
                Ok(sold(&id, &l2, qty, now, A_BLOCK + 1))
            }
        };
        let res = credit_lock::selling(&c, &bb, order).await;
        t.done(&c, &bb, q.route.as_ref(), &res);
        (q, worst, res)
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !got.load(Ordering::SeqCst),
        "B не котирует, пока A котирует и продаёт"
    );
    // Было (живой случай): учёт A ещё не обновлён — для B на кошельке «всё чужое».
    let o = on_account(&core, &b, &dex());
    assert_eq!(o.qty, d(A_QTY));
    assert_eq!(own_qty(b.qty, d(W1_AFTER_A), &o), Own::Human);
    // Продажа A: учёт в памяти — до снятия замков (credit_lock::selling), затем итог — ход снят.
    let ra = credit_lock::selling(&core, &a, async {
        Ok(sold(&a.id, &lqr, d(A_QTY), d(A_SOLD), A_BLOCK))
    })
    .await;
    let a_done = ra.as_ref().unwrap().finished_at;
    assert_eq!(
        qty_of(&core, &a.id),
        Decimal::ZERO,
        "учёт A в памяти обновлён"
    );
    ta.done(&core, &a, Some(&route(A_BLOCK - 1)), &ra);
    // Квитанция A есть, а событие пула ещё не пришло: B котирует по блоку до A — отвергается, ждёт тика.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(got.load(Ordering::SeqCst));
    assert!(!h.is_finished(), "котировка по пулу до A не берётся");
    // Сверка B сейчас (кошелёк 5372 после A): A своё продала — чужих монет нет, у B её 5371.95816, алерта нет.
    let o = on_account(&core, &b, &dex());
    assert_eq!(o.qty, Decimal::ZERO);
    let mut check = b.clone();
    assert!(arb::core::remain_fact::apply_fact(&core, &mut check, d(W1_AFTER_A)).await);
    assert_eq!(check.qty, d(B_QTY));
    // Событие пула блока A пришло к адаптеру — тик монеты.
    pool_block.store(A_BLOCK, Ordering::SeqCst);
    let _ = core.inner.coin_ticks.send(lqr.clone());
    let (q, worst, res) = tokio::time::timeout(Duration::from_secs(5), h)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(q.avg_price, d(B_POST), "котировка B — по пулу после A");
    assert!(q.at > a_done, "котировка B позже продажи A");
    assert_eq!(worst, d(B_POST) * (Decimal::ONE - d("0.005")));
    assert!(res.is_ok(), "продажа B проходит: {res:?}");
    // По прежней котировке (до A) минимальная цена выше цены пула после A — отказ, как вживую.
    let old = worst::worst_price(&core, &dex(), &lqr, Side::Sell, b.qty, d(B_PRE)).unwrap();
    assert!(d(B_POST) < old);
    assert_eq!(qty_of(&core, &a.id), Decimal::ZERO);
    assert_eq!(qty_of(&core, &b.id), Decimal::ZERO);
    let rows = j.list(500, None).await.unwrap();
    assert!(
        !rows
            .iter()
            .any(|e| e.message.contains("Своих монет на счёте не видно")),
        "ложного алерта нет"
    );
    assert!(
        !rows.iter().any(|e| e.kind == "core.sell_turn"),
        "срок не выходил"
    );
}

/// Сделка из учёта памяти ядра (с количеством до продажи).
fn b_of(core: &Core, id: &str) -> Deal {
    core.inner.deals.lock().unwrap().get(id).cloned().unwrap()
}

#[tokio::test]
async fn other_keys_run_in_parallel() {
    let (core, s, _j) = start("par").await;
    let (x, y) = (coin("T200PX"), coin("T200PY"));
    let bingx = VenueId("bingx".into());
    let dx = deal("T200PX1", &x, "10");
    let t = sell_turn::take(&dex(), &x).await;
    assert!(sell_turn::try_take(&dex(), &y).is_some(), "другая монета");
    assert!(sell_turn::try_take(&bingx, &x).is_some(), "другая площадка");
    assert!(
        sell_turn::try_take(&dex(), &x).is_none(),
        "та же монета той же площадки — по одной"
    );
    // Другая монета продаёт, пока ход X держится.
    let other = tokio::time::timeout(Duration::from_secs(1), sell_turn::take(&dex(), &y)).await;
    assert!(other.is_ok(), "ход другой монеты не ждёт");
    drop(other);
    // Неисполненный ордер следа не оставляет; исполненный — оставляет до проверки котировкой.
    let none = OpError::from("Продажа не исполнилась".to_string());
    t.done(&core, &dx, Some(&route(6)), &Err(none));
    let t = sell_turn::take(&dex(), &x).await;
    assert!(t.last().is_none());
    let r = sold("X1", &x, d("10"), d("2"), 7);
    t.done(&core, &dx, r.request.route.as_ref(), &Ok(r.clone()));
    let mut t = sell_turn::take(&dex(), &x).await;
    // exec.via_kyber не задана — исполнял свой сборщик: маршрут запроса — фактический (ревью t200, С-3).
    assert_eq!(t.last(), Some(&Last::of(&r, true)));
    assert_eq!(t.last().unwrap().pools, vec![pool()]);
    assert_eq!(t.last().unwrap().block, Some(7));
    t.seen();
    // Ревью t200 (С-2): исход «неизвестен» — след: конец сейчас, блок неизвестен, пулы из запроса.
    let unknown = OpError {
        code: Some(arb::gateway::errors::ErrorCode::OutcomeUnknown),
        message: "исход неизвестен".into(),
        sent: true,
        cancelled: false,
        price_moved: false,
    };
    let before = Utc::now();
    t.done(&core, &dx, Some(&route(8)), &Err(unknown));
    let t = sell_turn::take(&dex(), &x).await;
    let l = t.last().cloned().unwrap();
    assert_eq!(
        (l.block, l.pools, l.deal.as_str()),
        (None, vec![pool()], "T200PX1")
    );
    assert!(l.at >= before);
    assert!(sell_turn::sold_after(
        &dex(),
        &x,
        before - chrono::Duration::milliseconds(1)
    ));
    // exec.via_kyber = да — маршрут обмена строит KyberSwap: маршрут следа «неизвестен».
    drop(t);
    set_dex(&s, "exec.via_kyber", json!(true)).await;
    assert!(!sell_turn::route_known(&core, &dex()));
}

#[test]
fn continuation_does_not_hold_turn() {
    t198_live::big(async {
        let (core, s, _j) = start("cont").await;
        set(&s, "stuck.mode", json!("wait")).await;
        set(&s, "stuck.wait_ms", json!(600_000)).await;
        let c = coin("T200S");
        let mut x = deal("T200S1", &c, "10");
        x.group = None;
        ledger::save(&core, &x).await;
        let k = core.clone();
        // Продажа без котировки (площадки нет): ход снят сразу, дальше — «Ожидание» (минуты).
        let h = tokio::spawn(async move { arb::core::sell::sell_or_stuck(&k, x).await });
        let mut stuck = false;
        for _ in 0..200 {
            if b_of(&core, "T200S1").step == Step::Stuck {
                stuck = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(stuck, "сделка в режиме застрявшей монеты");
        assert!(!h.is_finished(), "ожидание идёт");
        assert!(
            sell_turn::try_take(&dex(), &c).is_some(),
            "режим застрявшей монеты хода не держит"
        );
        // Тик монеты: котировка-решение берёт ход и сразу снимает (котировки нет — ждём дальше).
        let _ = core.inner.coin_ticks.send(c.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!h.is_finished());
        assert!(
            sell_turn::try_take(&dex(), &c).is_some(),
            "после котировки-решения ход свободен"
        );
        h.abort();
    });
}
