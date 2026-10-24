//! Файл: tests/bsc_dex_via.rs
//! Что делает: проверяет путь через промежуточную монету (решение 52) без сети: покупка «USDT → WBNB → монета» и
//! продажа «монета → WBNB → USDT» по двум парам V2 (официальная формула getAmountOut/getAmountIn — проверена в
//! tests/bsc_dex_math.rs на значениях v2-periphery), затраты по шагам (шаг 1 «USDT → BNB», шаг 2 «BNB → X», валюты —
//! USDT, BNB, тикер монеты), шаги исполнения: первый платит кошелёк и отдаёт роутеру, второй — весь баланс роутера.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{quote, exec, pools, check}, arb::settings (настоящая SQLite во временной
//! папке). Резервы пар заданы числами — записанных пулов BSC нет (из песочницы узлы BSC недоступны); проверяется
//! композиция шагов, а не рынок.

use std::str::FromStr;

use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::types::{
    Asset, CheckState, CoinCheck, Pool, PoolRef, PoolState, Side, TokenTaxes, TradeRequest, VenueId,
};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::exec::chain;
use arb::venues::bsc_dex::exec::ur::CONTRACT_BALANCE;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::families::FAMILIES;
use arb::venues::bsc_dex::pools::mids::Mid;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::quote::steps::{self, Names};
use arb::venues::bsc_dex::quote::via::Via;

const COIN: Address = address!("0x1000000000000000000000000000000000000001");
const WBNB: Address = address!("0x2000000000000000000000000000000000000002");
const USDT: Address = address!("0x3000000000000000000000000000000000000003");

/// Окружение на настоящей SQLite с настройками, без которых путь не считается.
async fn ctx() -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-dex-via-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let c = Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    let v = Scope::Venue("bsc_dex".into());
    for (name, scope, val) in [
        ("venue.quote_asset", v.clone(), json!("USDT")),
        ("dex.min_pool_liquidity_usd", v.clone(), json!(1)),
        ("exec.dex_slippage_pct", v.clone(), json!(1)),
        ("dex.enabled", Scope::Dex("pancakeswap".into()), json!(true)),
        (
            "dex.pool_enabled",
            Scope::Dex("pancakeswap_v2".into()),
            json!(true),
        ),
        (
            "dex.v2_fee_pct",
            Scope::Dex("pancakeswap_v2".into()),
            json!("0.25"),
        ),
    ] {
        c.settings
            .set(name, scope, Some(val), "тест")
            .await
            .unwrap();
    }
    c
}

/// Пара V2 с резервами (token0 < token1 по адресу).
fn pair(
    addr: Address,
    coin: Address,
    quote: Address,
    ticker: &str,
    r_coin: u128,
    r_quote: u128,
) -> Entry {
    let f = FAMILIES.iter().find(|f| f.id == "pancakeswap_v2").unwrap();
    let (t0, t1, r0, r1) = if coin < quote {
        (coin, quote, r_coin, r_quote)
    } else {
        (quote, coin, r_quote, r_coin)
    };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(addr),
            token0: t0,
            token1: t1,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::from(r0),
                reserve1: U256::from(r1),
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: ticker.into(),
        coin,
        usdt: quote,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
    }
}

/// Проверка монеты без налогов.
fn clean(ticker: &str, a: Address) -> CoinCheck {
    let zero = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: Some(a),
        },
        taxes: CheckState::Known(zero),
        source: "тест".into(),
        checked_at: Some(Utc::now()),
    }
}

#[tokio::test]
async fn buy_and_sell_through_wbnb() {
    let p = Parts::new(ctx().await);
    let q = p.quoting();
    let e18 = 10u128.pow(18);
    p.tokens.set(&[
        Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(USDT),
        },
        Asset {
            ticker: "X".into(),
            bsc_contract: Some(COIN),
        },
    ]);
    p.mids.set(vec![Mid {
        address: WBNB,
        ticker: "BNB".into(),
    }]);
    let p1 = pair(
        address!("0xa000000000000000000000000000000000000001"),
        WBNB,
        USDT,
        "BNB",
        1000 * e18,
        600_000 * e18,
    );
    let p2 = pair(
        address!("0xa000000000000000000000000000000000000002"),
        COIN,
        WBNB,
        "X",
        100_000 * e18,
        500 * e18,
    );
    for e in [&p1, &p2] {
        p.registry.put(e.clone());
        p.checks.update_pool(&e.pool.reference, |f| {
            f.gas_buy = Some(100_000);
            f.gas_sell = Some(110_000);
            f.quoter = Some(Ok(()));
        });
    }
    p.checks.set_coin(clean("BNB", WBNB));
    p.checks.set_coin(clean("X", COIN));
    let t = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    let mid = Mid {
        address: WBNB,
        ticker: "BNB".into(),
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (COIN, "X"),
        mids: &p.mids,
    };
    let qty = Decimal::from(10u8);
    for side in [Side::Buy, Side::Sell] {
        let v = Via {
            q: &q,
            coin: COIN,
            usdt: USDT,
            side,
            taxes: &t,
            coin_dec: 18,
            usdt_dec: 18,
        };
        let (a, b) = v.pools(&mid, &|_| true).expect("пары допущены");
        let path = v.calc(&mid, 18, &a[0], &b[0], qty).expect("путь считается");
        // Цена монеты ≈ 600 USDT/BNB × 0,005 BNB/X = 3 USDT; с двумя комиссиями 0,25 % и проскальзыванием — рядом.
        let price = path.usdt / path.coins;
        assert!(
            price > Decimal::from_str("2.9").unwrap() && price < Decimal::from_str("3.1").unwrap(),
            "{price}"
        );
        let tokens: Vec<_> = path
            .hops
            .iter()
            .map(|h| (h.token_in, h.token_out))
            .collect();
        match side {
            Side::Buy => {
                assert_eq!(tokens, vec![(USDT, WBNB), (WBNB, COIN)]);
                assert!(path.coins >= qty - Decimal::new(1, 15) && path.coins <= qty);
                assert!(price > Decimal::from(3u8), "покупка дороже середины рынка");
            }
            Side::Sell => {
                assert_eq!(tokens, vec![(COIN, WBNB), (WBNB, USDT)]);
                assert_eq!(path.coins, qty);
                assert!(price < Decimal::from(3u8), "продажа дешевле середины рынка");
            }
        }
        // Затраты по шагам: газ, комиссия пула, крючок — у каждого шага свой RouteStep; валюты пересчитываемы.
        let costs = steps::path_costs(
            &q.ctx,
            &path,
            side,
            &t,
            U256::from(1_000_000_000u64),
            &names,
        )
        .unwrap();
        let idx: Vec<u32> = costs
            .iter()
            .filter_map(|c| c.step.as_ref().map(|s| s.index))
            .collect();
        assert!(idx.contains(&1) && idx.contains(&2));
        assert!(
            costs
                .iter()
                .all(|c| ["USDT", "BNB", "X"].contains(&c.asset.as_str()))
        );
        let s1 = costs[0].step.clone().unwrap();
        let want = if side == Side::Buy {
            ("USDT", "BNB")
        } else {
            ("X", "BNB")
        };
        assert_eq!((s1.from.as_str(), s1.to.as_str()), want);
        // Шаги исполнения: первый — из кошелька роутеру с ненулевым минимумом, второй — весь баланс роутера кошельку.
        let worst = if side == Side::Buy {
            Decimal::from(4u8)
        } else {
            Decimal::from(2u8)
        };
        let req = TradeRequest {
            venue: VenueId("bsc_dex".into()),
            asset: Asset {
                ticker: "X".into(),
                bsc_contract: Some(COIN),
            },
            side,
            qty,
            worst_price: worst,
            deal_id: "т".into(),
            route: None,
        };
        let st = chain::steps(&q.ctx, &req, &path, qty, (18, 18)).unwrap();
        assert_eq!(st.len(), 2);
        assert!(st[0].from_wallet && !st[0].to_wallet && !st[0].order.min_out.is_zero());
        assert!(!st[1].from_wallet && st[1].to_wallet && st[1].order.amount_in == CONTRACT_BALANCE);
    }
}
