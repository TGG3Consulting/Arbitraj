//! Файл: tests/bsc_dex_executor_v2_review.rs
//! Что делает: ревью 272 (Б7) — исполнитель v2 в боте без сети и без моков логики:
//! - факт executeNative (executor/fact.rs native_result) по ЗАПИСИ исполнения настоящего байткода v2 на EVM ethereumjs
//!   (tests/data/executor_v2_evm_2026-11-14, contracts/evmtest): статус, средняя цена, частичное исполнение (остаток
//!   вернулся), подкинутый BNB не меняет «продано»; транзакция в блоке без события NativeSettled — OutcomeUnknown;
//! - позднее выяснение (Б4): признак executeNative в контексте (SwapCtx.executor_native) — без события OutcomeUnknown,
//!   а не «0 монет по пути ERC-20»;
//! - продажа BNB: value = вход, разрешений ERC-20 исполнителю нет (executor::approvals без узла);
//! - газ обёртки (quote/native_gas.rs) при маршруте через исполнитель — одна обёртка (наибольший замер долей), без
//!   исполнителя — по роутерам бандла;
//! - миграция: база со старой таблицей dex_wallets (без executor_version / executor_wrapped) открывается, столбцы
//!   добавляются, кошелёк читается «не сверен»;
//! - Б1: запись версии исполнителя (как её делает onchain.rs::ensure → remember) не поднимает сигнал «кошельки
//!   изменились» (подписки кошельков не перезапускаются); смена адреса вручную — поднимает, как раньше.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec, quote, send}, arb::settings::accounts, arb::storage, tests/route_common.

mod route_common;

use alloy_primitives::{Address, B256, U256};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{
    Asset, ChainTx, SendMethod, Side, TokenTaxes, TradeRequest, TradeStatus, TxOutcome, VenueId,
};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::executor::{self, call, fact};
use arb::venues::bsc_dex::exec::swap_ctx::SwapCtx;
use arb::venues::bsc_dex::exec::{chain_tx, fill, swap_rebuild, txs};
use arb::venues::bsc_dex::quote::native_gas;
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{Quoting, graph_route};
use arb::venues::bsc_dex::send::track::Receipt;
use arb::venues::bsc_dex::send::track_util::Tracked;

use route_common::{CAKE, USDT};

const DECS: (u8, u8) = (18, 18);

/// Запись EVM (contracts/evmtest/run.mjs → native_settled.json).
fn rec() -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/executor_v2_evm_2026-11-14/native_settled.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}
fn a(v: &Value) -> Address {
    v.as_str().unwrap().parse().unwrap()
}
fn u(v: &Value) -> U256 {
    v.as_str().unwrap().parse().unwrap()
}
fn dec(x: U256) -> Decimal {
    arb::venues::bsc_dex::num::to_decimal(x, 18).unwrap()
}

/// Квитанция из записи (поля блока, которых у EVM без блока нет, — null, как у eth_getTransactionReceipt Log).
/// keep_settled = false — без события NativeSettled исполнителя (транзакция в блоке без события).
fn receipt(case: &Value, keep_settled: bool) -> Receipt {
    let topic = format!(
        "{:#x}",
        <executor::abi::ArbExecutor::NativeSettled as alloy::sol_types::SolEvent>::SIGNATURE_HASH
    );
    let logs: Vec<Value> = case["logs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|l| keep_settled || l["topics"][0].as_str() != Some(topic.as_str()))
        .map(|l| {
            json!({
                "address": l["address"], "topics": l["topics"], "data": l["data"], "blockHash": null,
                "blockNumber": null, "transactionHash": null, "transactionIndex": null, "logIndex": null,
                "removed": false
            })
        })
        .collect();
    serde_json::from_value(json!({
        "status": "0x1", "blockNumber": "0x1", "gasUsed": "0x5208", "effectiveGasPrice": "0x3b9aca00", "logs": logs
    }))
    .unwrap()
}

fn req(side: Side, qty: Decimal, wbnb: Address) -> TradeRequest {
    TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            chain: None,
            ticker: "BNB".into(),
            contract: Some(wbnb),
        },
        side,
        qty,
        worst_price: Decimal::ONE,
        deal_id: "20261114120000000BNB".into(),
        route: None,
        exit_position: false,
        remeasure: None,
    }
}

fn done(r: Receipt) -> txs::Done {
    let t = Tracked {
        outcome: TxOutcome::Included,
        receipt: Some(r.clone()),
        cancel: None,
        unresolved: None,
    };
    txs::Done {
        chain: chain_tx::chain_tx(B256::ZERO, 7, vec![], SendMethod::Single, &t, None),
        receipt: Some(r),
        cancel: None,
        tip_bnb: None,
        cancels: Vec::new(),
    }
}

/// Итог executeNative по случаю записи. Принимает: окружение, случай, сторону, количество запроса.
fn native(
    ctx: &Ctx,
    c: &Value,
    side: Side,
    qty: Decimal,
    keep: bool,
) -> arb::gateway::errors::GatewayResult<arb::core::types::TradeResult> {
    let (w, us) = (a(&c["wbnb"]), a(&c["usdt"]));
    let tokens = if side == Side::Sell { (w, us) } else { (us, w) };
    let ends = (a(&c["owner"]), a(&c["executor"]));
    fact::result(
        ctx,
        req(side, qty, w),
        done(receipt(c, keep)),
        ends,
        tokens,
        DECS,
        true,
    )
}

#[tokio::test]
async fn native_result_status_price_partial_unknown() {
    let p = route_common::parts().await;
    let r = rec();
    let cs = &r["cases"];
    // Продажа 1 BNB целиком.
    let s = &cs["sell"];
    let t = native(&p.ctx, s, Side::Sell, Decimal::ONE, true).unwrap();
    assert_eq!(
        (t.status, t.filled_qty),
        (TradeStatus::Filled, Decimal::ONE)
    );
    assert_eq!(t.avg_price, dec(u(&s["usdt_got"])));
    // Продажа доли 5000 bps: остаток 0,5 вернулся — продано 0,5, частично, цена = USDT / 0,5.
    let l = &cs["sell_left"];
    let got = fill::moved(&receipt(l, true), a(&l["usdt"]), None, Some(a(&l["owner"])));
    let t = native(&p.ctx, l, Side::Sell, Decimal::ONE, true).unwrap();
    let half = Decimal::new(5, 1);
    assert_eq!(
        (t.status, t.filled_qty),
        (TradeStatus::PartiallyFilled, half)
    );
    assert_eq!(t.avg_price, dec(got) / half);
    // Подкинутые 3 BNB (ревью А3): событие — только свои операции, продано по-прежнему 0,5.
    let f = &cs["sell_foreign_bnb"];
    let t = native(&p.ctx, f, Side::Sell, Decimal::ONE, true).unwrap();
    assert_eq!(
        (t.status, t.filled_qty),
        (TradeStatus::PartiallyFilled, half)
    );
    // Покупка: куплено = nativeOut, цена = потрачено USDT / куплено.
    let b = &cs["buy"];
    let bnb = dec(u(&b["bnb_got"]));
    let t = native(&p.ctx, b, Side::Buy, bnb, true).unwrap();
    assert_eq!((t.status, t.filled_qty), (TradeStatus::Filled, bnb));
    assert_eq!(t.avg_price, dec(u(&b["usdt_spent"])) / bnb);
    // В блоке, но без NativeSettled — исход неизвестен (с фактом транзакции), не тихий 0.
    let e = native(&p.ctx, s, Side::Sell, Decimal::ONE, false).expect_err("без события");
    assert_eq!(e.code, ErrorCode::OutcomeUnknown);
    assert!(
        e.tx.is_some() && e.message.contains("NativeSettled"),
        "{}",
        e.message
    );
}

/// Факт транзакции на момент «исход неизвестен» (как exec/txs.rs).
fn unknown_chain() -> ChainTx {
    ChainTx {
        hash: B256::ZERO,
        nonce: 7,
        channels: vec![],
        method: SendMethod::Single,
        outcome: TxOutcome::Pending,
        block: None,
        gas_used: None,
        gas_bnb: None,
        tip_bnb: None,
        cancel_hash: None,
        cancel_gas_bnb: None,
        approve_gas_bnb: None,
        tip_hash: None,
        tip_gas_bnb: None,
    }
}

#[tokio::test]
async fn late_resolve_uses_native_flag_from_ctx() {
    let p = route_common::parts().await;
    let r = rec();
    let s = &r["cases"]["sell"];
    let (w, us, owner, x) = (
        a(&s["wbnb"]),
        a(&s["usdt"]),
        a(&s["owner"]),
        a(&s["executor"]),
    );
    let mut sc = SwapCtx::swap(owner, (w, us), DECS, &[], Some(x), None);
    sc.chain = Some(unknown_chain());
    sc.executor_native = true;
    let rq = || req(Side::Sell, Decimal::ONE, w);
    let e = swap_rebuild::rebuild(&p.ctx, rq(), &sc, B256::ZERO, receipt(s, false))
        .expect_err("признак, без события");
    assert_eq!(e.code, ErrorCode::OutcomeUnknown);
    let t = swap_rebuild::rebuild(&p.ctx, rq(), &sc, B256::ZERO, receipt(s, true)).unwrap();
    assert_eq!(t.filled_qty, Decimal::ONE);
    // Без признака (контекст до ревью) и без события — путь ERC-20 (WBNB с кошелька не уходил): 0 монет. Это и было
    // «0 монет по пути ERC-20», которое признак закрывает.
    sc.executor_native = false;
    let old = swap_rebuild::rebuild(&p.ctx, rq(), &sc, B256::ZERO, receipt(s, false)).unwrap();
    assert_eq!(old.filled_qty, Decimal::ZERO);
}

#[tokio::test]
async fn sell_native_value_without_approve() {
    let p = route_common::parts().await;
    let v = Scope::Venue("bsc_dex".into());
    p.ctx
        .settings
        .set("dex.deadline_s", v, Some(json!(60)), "тест")
        .await
        .unwrap();
    let q = p.quoting();
    let amt = U256::from(10u64).pow(U256::from(18u8));
    let step = arb::venues::bsc_dex::exec::chain::Step {
        entry: q
            .reg
            .get(&arb::core::types::PoolRef::Contract(
                route_common::WBNB_USDT_V2,
            ))
            .unwrap(),
        order: arb::venues::bsc_dex::exec::plan::Order {
            token_in: route_common::WBNB,
            token_out: USDT,
            amount_in: amt,
            min_out: U256::from(5u8),
        },
        from_wallet: true,
        to_wallet: true,
    };
    let x = Address::repeat_byte(0xee);
    let c = call::build(&q.ctx, &[vec![step]], x, 1_000)
        .unwrap()
        .with_native(&q.ctx, route_common::WBNB, true)
        .unwrap();
    assert_eq!(c.value(), amt);
    // Продажа BNB платит value — разрешений нет (и узел не спрашивается: у теста узла нет).
    let need = executor::approvals(&q.ctx, &c, Address::repeat_byte(0xaa), x)
        .await
        .unwrap();
    assert!(need.is_empty());
}

/// Графовая котировка 50k CAKE (мульти-роутерный маршрут записи) — как tests/bsc_dex_bundle_tip_plan.rs.
async fn graph_quote(q: &Quoting) -> arb::core::types::Quote {
    let t = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    let via = Via {
        q,
        coin: CAKE,
        usdt: USDT,
        side: Side::Buy,
        taxes: &t,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &q.mids,
        gas: "BNB",
    };
    let cake = Asset {
        chain: None,
        ticker: "CAKE".into(),
        contract: Some(CAKE),
    };
    graph_route::quote(
        &via,
        &names,
        &cake,
        Decimal::from(50_000u32),
        U256::from(1_000_000_000u64),
    )
    .await
    .expect("котировка графа")
}

#[tokio::test]
async fn native_gas_one_wrap_via_executor() {
    let p = route_common::parts().await;
    route_common::channel(&p, "club48", "bundle").await;
    let v = Scope::Venue("bsc_dex".into());
    let set = |n: &'static str, val: Value| {
        let (s, v) = (p.ctx.settings.clone(), v.clone());
        async move { s.set(n, v, Some(val), "тест").await.unwrap() }
    };
    set("dex.bundle_max_routers", json!(5)).await;
    let x = "0x9ef558a5d437718dde44e00990e0efb9976cf794";
    route_common::executor(&p, x).await;
    let q = p.quoting();
    let quote = graph_quote(&q).await;
    // Логика группировки газа обёртки проверяется на записанном мульти-роутерном маршруте: монета маршрута (CAKE)
    // объявлена «нативной» настройками (dex.native_ticker / dex.native_wrapped) ПОСЛЕ котировки — маршрут тот же.
    set("dex.native_ticker", json!("CAKE")).await;
    set("dex.native_wrapped", json!(CAKE.to_string())).await;
    // У каждого DEX записи свой Universal Router (route_common даёт всем один адрес) — бандл группирует обёртку по ним.
    for (fam, ur) in [
        ("pancakeswap_v3", 0x91u8),
        ("uniswap_v3", 0x92),
        ("pancakeswap_v2", 0x93),
    ] {
        let addr = format!("{:#x}", Address::repeat_byte(ur));
        p.ctx
            .settings
            .set(
                "dex.universal_router",
                Scope::Dex(fam.into()),
                Some(json!(addr)),
                "тест",
            )
            .await
            .unwrap();
    }
    let route = quote.route.clone().unwrap();
    let fams: std::collections::HashSet<String> = route
        .legs
        .iter()
        .map(|l| l.hops.last().unwrap().family.clone())
        .collect();
    assert!(
        fams.len() >= 2,
        "доли маршрута кончаются пулами разных DEX: {fams:?}"
    );
    let mut max = 0u64;
    for (i, leg) in route.legs.iter().enumerate() {
        let g = 10_000 + 1_000 * i as u64;
        max = max.max(g);
        q.checks.update_pool(&leg.hops.last().unwrap().pool, |f| {
            f.wrap_gas = Some(Ok((g, g)))
        });
    }
    let gas = U256::from(1_000_000_000u64);
    // Исполнитель не сверен — маршрут бандлом: газ по роутерам (у каждого Universal Router своя обёртка).
    let (_, bundle) = native_gas::cost(&q, &quote, gas).unwrap().unwrap();
    // Исполнитель v2, обёртка = dex.native_wrapped — одна транзакция executeNative: одна обёртка, наибольший замер.
    let a = p.ctx.settings.accounts();
    let w = a.wallets().into_iter().find(|w| w.name == "тест").unwrap();
    a.set_executor_version(
        &w.id,
        x,
        Some("v2".into()),
        Some(format!("{CAKE:#x}")),
        "тест",
    )
    .await
    .unwrap();
    assert!(executor::native_capable(&q.ctx));
    let (_, one) = native_gas::cost(&q, &quote, gas).unwrap().unwrap();
    assert_eq!(one, max, "через исполнитель — одна обёртка");
    assert!(bundle > one, "бандлом — по роутерам: {bundle} > {one}");
    // Обёртка в исполнителе не та (ревью А1) — нативный BNB через исполнитель не идёт, снова по роутерам.
    a.set_executor_version(
        &w.id,
        x,
        Some("v2".into()),
        Some(format!("{USDT:#x}")),
        "тест",
    )
    .await
    .unwrap();
    assert!(executor::native_block(&q.ctx).is_some_and(|w| w.contains("dex.native_wrapped")));
    assert_eq!(
        native_gas::cost(&q, &quote, gas).unwrap().unwrap().1,
        bundle
    );
}

/// Хранилище на базе в папке.
async fn store(d: &std::path::Path) -> Store {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    Store::load(db.clone(), changes, Journal::new(db, jtx))
        .await
        .unwrap()
}

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("arb-exv2-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[tokio::test]
async fn old_db_gets_executor_columns() {
    let d = dir("migrate");
    {
        // Таблица dex_wallets до решения 272 (storage/db.rs SCHEMA тех коммитов) и кошелёк в ней.
        let c = rusqlite::Connection::open(d.join("t.sqlite")).unwrap();
        c.execute_batch(
            "CREATE TABLE dex_wallets (id TEXT PRIMARY KEY, name TEXT NOT NULL, secret TEXT NOT NULL, executor TEXT, \
             queue_key TEXT NOT NULL, created_at TEXT NOT NULL);
             INSERT INTO dex_wallets VALUES ('1', 'BingX', 'bsc_dex.wallet_key', \
             '0x9ef558a5d437718dde44e00990e0efb9976cf794', 'bsc_dex', '2026-11-03T00:00:00Z');",
        )
        .unwrap();
    }
    let s = store(&d).await;
    let w = s.accounts().wallet("1").expect("старый кошелёк читается");
    assert_eq!(
        w.executor.as_deref(),
        Some("0x9ef558a5d437718dde44e00990e0efb9976cf794")
    );
    assert_eq!(
        (w.executor_version, w.executor_wrapped),
        (None, None),
        "не сверен"
    );
    let c = rusqlite::Connection::open(d.join("t.sqlite")).unwrap();
    let cols: Vec<String> = c
        .prepare("SELECT name FROM pragma_table_info('dex_wallets')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        cols.iter().any(|c| c == "executor_version")
            && cols.iter().any(|c| c == "executor_wrapped")
    );
}

#[tokio::test]
async fn version_write_does_not_restart_wallet_tasks() {
    let d = dir("b1");
    let s = store(&d).await;
    let a = s.accounts();
    let id = a.reserve_id().await.unwrap();
    a.add(&id, "BingX", "wallet.k", "bsc_dex", "тест")
        .await
        .unwrap();
    let x = "0x9ef558a5d437718dde44e00990e0efb9976cf794";
    let mut rx = a.subscribe();
    a.set_executor(&id, Some(x.into()), "тест").await.unwrap();
    assert!(
        rx.has_changed().unwrap(),
        "смена адреса вручную — сигнал, как раньше"
    );
    let _ = rx.borrow_and_update();
    // Как onchain.rs::ensure → remember: сверка нашла v1 — запись без сигнала (подписки кошельков не трогаются).
    a.set_executor_version(&id, x, Some("v1".into()), None, "сверка кода исполнителя")
        .await
        .unwrap();
    assert!(
        !rx.has_changed().unwrap(),
        "запись версии не перезапускает подписки"
    );
    assert_eq!(
        a.wallet(&id).unwrap().executor_version.as_deref(),
        Some("v1")
    );
    let wb = "0xbb4cdb9cbd36b01bd1cbaebf2de08d9173bc095c";
    a.set_executor_version(&id, x, Some("v2".into()), Some(wb.into()), "сверка")
        .await
        .unwrap();
    assert!(!rx.has_changed().unwrap());
    assert_eq!(a.wallet(&id).unwrap().executor_wrapped.as_deref(), Some(wb));
    // Версия переживает перезапуск; смена адреса сбрасывает версию и обёртку.
    let w = store(&d).await.accounts().wallet(&id).unwrap();
    assert_eq!(
        (w.executor_version.as_deref(), w.executor_wrapped.as_deref()),
        (Some("v2"), Some(wb))
    );
    a.set_executor(
        &id,
        Some("0x00000000000000000000000000000000000000ef".into()),
        "тест",
    )
    .await
    .unwrap();
    let w = a.wallet(&id).unwrap();
    assert!(
        rx.has_changed().unwrap() && w.executor_version.is_none() && w.executor_wrapped.is_none()
    );
}
