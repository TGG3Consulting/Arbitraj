//! Файл: tests/bsc_dex_exit_quote.rs
//! Что делает: решение 177 на цифрах живого случая 05.11.2026 (сделка 20261105131254836GAIB, mexc → bsc_dex, кошелёк w2,
//! 2434.66 GAIB, пул pancakeswap-infinity-clmm): «перед сделкой расчёт 42559024903490846447 против квотера
//! 42489232805231828113: расхождение 0.1642583% > 0.1% (dex.quoter_max_diff_pct)»; продажа прошла по 0.01745 после того,
//! как допуск подняли до 0.25.
//! Проверяется без узла (сверка после ответа квотера — чистые функции): (1) текст отказа сверки — тот же, что в живом
//! случае; (2) покупка (и продажа без признака выхода) с этим расхождением — отказ QuoteMismatch, как раньше; (3) продажа выхода — не отказ: ожидаемый выход пути = выход квотера, минимум «получить
//! не меньше» — от квотера с exec.dex_slippage_pct; (4) выход по квотеру хуже худшей цены — отказ, как раньше.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec, quote, ctx, pools}, arb::settings (настоящая SQLite во временной папке).

use std::path::PathBuf;
use std::str::FromStr;

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
use arb::venues::bsc_dex::exec::{exit_quote, precheck, trade};
use arb::venues::bsc_dex::math::swap_cl::Swapped;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::path::{self, Path};
use arb::venues::bsc_dex::quote::quoter::{self, Judged};
use arb::venues::bsc_dex::quote::select::Usable;

const COIN: Address = address!("0x1000000000000000000000000000000000000001");
const USDT: Address = address!("0x2000000000000000000000000000000000000002");
/// Живой случай: локальный выход (за вычетом крючка) и выход квотера, USDT в минимальных единицах (18 знаков).
const LOCAL: &str = "42559024903490846447";
const REMOTE: &str = "42489232805231828113";

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn u(s: &str) -> U256 {
    U256::from_str(s).unwrap()
}

/// Окружение адаптера на настоящей SQLite: допуск сверки 0.1% (как в живом случае), допуск минимума 0.5%.
async fn ctx(name: &str) -> Ctx {
    let dir: PathBuf = std::env::temp_dir().join(format!("arb-exit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let c = Ctx {
        raw: RawLog::start(dir.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(dir.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    for (n, v) in [
        ("dex.quoter_max_diff_pct", json!("0.1")),
        ("exec.dex_slippage_pct", json!("0.5")),
    ] {
        c.settings
            .set(n, Scope::Venue("bsc_dex".into()), Some(v), "тест")
            .await
            .unwrap();
    }
    c
}

/// Пул Infinity CL «GAIB — USDT» (состояние для этих проверок не читается: сверка уже дала цифры).
fn entry() -> Entry {
    let family = families::make("pancakeswap-infinity-clmm", "pancakeswap_infinity_cl").unwrap();
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
            state: PoolState::Cl {
                sqrt_price_x96: U256::ZERO,
                tick: 0,
                liquidity: 0,
                protocol_fee: 0,
                lp_fee: 0,
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "GAIB".into(),
        coin: COIN,
        usdt: USDT,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

/// Запрос: сторона, худшая цена, признак выхода. 2434.66 GAIB — как в сделке.
fn req(side: Side, worst: &str, exit: bool) -> TradeRequest {
    TradeRequest {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: "GAIB".into(),
            bsc_contract: Some(COIN),
        },
        side,
        qty: d("2434.66"),
        worst_price: d(worst),
        deal_id: "20261105131254836GAIB".into(),
        route: None,
        exit_position: exit,
    }
}

/// Живой путь продажи по локальному расчёту: 2434.66 GAIB → 42.559024903490846447 USDT.
fn sell_path(c: &Ctx) -> Path {
    let usable = Usable {
        gas: 0,
        hook_pct: Decimal::ZERO,
        tax_pct: Decimal::ZERO,
    };
    let swapped = Swapped {
        amount_in: u("2434660000000000000000"),
        amount_out: u(LOCAL),
        complete: true,
    };
    let hop = path::hop(c, entry(), usable, swapped, (COIN, USDT), (18, 18)).unwrap();
    Path {
        coins: d("2434.66"),
        usdt: hop.pool_out,
        hops: vec![hop],
    }
}

#[tokio::test]
async fn live_mismatch_text_and_buy_still_refused() {
    let c = ctx("buy").await;
    let j = quoter::judge(&c, u(LOCAL), u(REMOTE), Decimal::ZERO).unwrap();
    let Judged::Diff { pct, .. } = &j else {
        panic!("ждали расхождение: {j:?}");
    };
    assert_eq!(pct.round_dp(7), d("0.1642583"));
    // Тот же текст, что в живом случае и что давал within до решения 177.
    let why = quoter::within(&c, u(LOCAL), u(REMOTE), Decimal::ZERO)
        .unwrap()
        .unwrap_err();
    assert!(why.starts_with(
        "расчёт 42559024903490846447 против квотера 42489232805231828113: расхождение 0.1642583"
    ));
    assert!(why.ends_with("% > 0.1% (dex.quoter_max_diff_pct)"));
    assert_eq!(j.reason().as_deref(), Some(why.as_str()));
    // Покупка — признак выхода не действует (адаптер его не исполняет), отказ QuoteMismatch, как раньше.
    for r in [
        req(Side::Buy, "0.02", true),
        req(Side::Buy, "0.02", false),
        req(Side::Sell, "0.017", false),
    ] {
        let exit = exit_quote::exit_of(&r);
        assert!(!exit);
        let e = precheck::verdict(&c, &entry(), j.clone(), exit).unwrap_err();
        assert_eq!(e.code, ErrorCode::QuoteMismatch);
        assert_eq!(
            e.message,
            format!("pancakeswap-infinity-clmm GAIB: перед сделкой {why}")
        );
    }
}

#[tokio::test]
async fn exit_sells_by_quoter_with_min_from_quoter() {
    let c = ctx("exit").await;
    let j = quoter::judge(&c, u(LOCAL), u(REMOTE), Decimal::ZERO).unwrap();
    // Худшая цена 0.0170: по квотеру (≈0.017452) продать можно.
    let r = req(Side::Sell, "0.0170", true);
    assert!(exit_quote::exit_of(&r));
    let fix = precheck::verdict(&c, &entry(), j, true)
        .unwrap()
        .expect("продажа выхода — не отказ, а поправка по квотеру");
    assert_eq!(
        fix.text(),
        "pancakeswap-infinity-clmm GAIB: расчёт 42559024903490846447 против квотера 42489232805231828113 (0.1643%) — продажа по квотеру"
    );
    let mut p = sell_path(&c);
    let ratio = exit_quote::rescale(&c, &mut p, Side::Sell, &[(0, fix)]).unwrap();
    assert!(ratio < Decimal::ONE);
    // Ожидаемый выход пути = выход квотера (до 1e-18 USDT — округление Decimal).
    let quoter_usdt = d("42.489232805231828113");
    assert!((p.usdt - quoter_usdt).abs() < d("0.000000000000000002"));
    assert_eq!(p.hops[0].pool_out, p.usdt);
    // Худшая цена — по квотеру; минимум — от квотера с exec.dex_slippage_pct 0.5%.
    trade::check_price(&c, &r, p.coins, p.usdt).unwrap();
    let o = trade::order(&c, &r, &p.hops[0].entry, r.qty, (p.coins, p.usdt), (18, 18)).unwrap();
    let min = arb::venues::bsc_dex::num::to_units(p.usdt * d("0.995"), 18).unwrap();
    assert_eq!(o.min_out, min);
    // Квотер даёт ~42.4892: минимум ≈42.2768 — сделка исполнима; от локального расчёта был бы 42.3462.
    assert!(o.min_out < u(REMOTE));
    let local_min =
        arb::venues::bsc_dex::num::to_units(d("42.559024903490846447") * d("0.995"), 18);
    assert!(o.min_out < local_min.unwrap());
    // Свежие минимумы графа — тем же множителем.
    assert_eq!(
        exit_quote::scale_mins(&[d("42.346229")], ratio),
        vec![d("42.346229") * ratio]
    );
}

#[tokio::test]
async fn exit_worse_than_worst_price_is_refused() {
    let c = ctx("worst").await;
    let j = quoter::judge(&c, u(LOCAL), u(REMOTE), Decimal::ZERO).unwrap();
    // Худшая цена 0.01746: локально (≈0.017481) проходит, по квотеру (≈0.017452) — нет.
    let r = req(Side::Sell, "0.01746", true);
    let local = sell_path(&c);
    trade::check_price(&c, &r, local.coins, local.usdt).unwrap();
    let fix = precheck::verdict(&c, &entry(), j, true).unwrap().unwrap();
    let mut p = sell_path(&c);
    exit_quote::rescale(&c, &mut p, Side::Sell, &[(0, fix)]).unwrap();
    let e = trade::check_price(&c, &r, p.coins, p.usdt).unwrap_err();
    assert_eq!(e.code, ErrorCode::QuoteMismatch);
    assert!(e.message.contains("хуже допустимой 0.01746"));
}
