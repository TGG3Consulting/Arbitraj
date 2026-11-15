//! Файл: tests/mid_dex_common/mod.rs
//! Что делает: окружение теста tests/bsc_dex_mid_dex.rs (решение 276) — настоящая SQLite во временной папке с
//! настройками котировки, пара V2 с записанными резервами, проверка монеты без налогов, допуск пулов. Тот же порядок,
//! что в tests/bsc_dex_via.rs; адреса строки DEX (роутер, фабрика, Universal Router) — условные: сеть в тесте не нужна.
//! Кто вызывает: tests/bsc_dex_mid_dex.rs, tests/bsc_dex_mid_dex_pick.rs, tests/bsc_dex_mid_dex_read.rs (каждому нужна
//! своя часть — отсюда allow(dead_code)); чтение записи — rec.rs.
//! От чего зависит: arb::venues::bsc_dex::{ctx, parts, pools, check}, arb::settings, arb::storage.
#![allow(dead_code)]

pub mod rec;

use alloy_primitives::{Address, U256};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

use arb::core::types::{
    Asset, CheckState, CoinCheck, Pool, PoolRef, PoolState, TokenTaxes, VenueId,
};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::check::state::PoolTaxes;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::registry::Entry;

/// Окружение. Принимает: имя папки, с настройками котировки и строкой DEX (false — пустое хранилище, кроме
/// venue.quote_asset: для mids::read без роутеров, т.е. без обращений к узлу).
pub async fn ctx(name: &str, quoting: bool) -> Ctx {
    let d = std::env::temp_dir().join(format!("arb-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let c = Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        account: None,
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    let v = Scope::Venue("bsc_dex".into());
    let dx = Scope::Dex("pancakeswap_v2".into());
    let a = json!("0x4000000000000000000000000000000000000004");
    let mut list = vec![("venue.quote_asset", v.clone(), json!("USDT"))];
    if quoting {
        // Значения теста, не предложение (как tests/bsc_dex_via.rs).
        list.extend([
            ("dex.min_pool_liquidity_usd", v.clone(), json!(1)),
            ("dex.depth_band_pct", v.clone(), json!(10)),
            ("dex.depth_hysteresis_pct", v.clone(), json!(0)),
            ("exec.dex_slippage_pct", v, json!(1)),
            ("tax.max_buy_pct", Scope::Global, json!("5")),
            ("tax.max_sell_pct", Scope::Global, json!("5")),
            ("dex.kind", dx.clone(), json!("pancakeswap_v2")),
            ("dex.pool_enabled", dx.clone(), json!(true)),
            // Комиссия пар fstswap 0,3 % — факт (tests/bsc_dex_fstswap_fee.rs, запись fee_check.json).
            ("dex.v2_fee_pct", dx.clone(), json!("0.3")),
            ("dex.universal_router", dx.clone(), a.clone()),
            (
                "dex.universal_router_version",
                dx.clone(),
                json!("pancakeswap_infinity"),
            ),
            ("dex.permit2", dx.clone(), a.clone()),
            ("dex.factory", dx.clone(), a.clone()),
            ("dex.router", dx.clone(), a),
            ("dex.v2_fee_source", dx, json!("fixed")),
        ]);
    }
    for (name, scope, val) in list {
        c.settings
            .set(name, scope, Some(val), "тест")
            .await
            .unwrap();
    }
    c
}

/// Пара V2 (token0 < token1 по адресу) с резервами записи. Принимает: адрес пары, монету и встречный токен записи
/// реестра, показное имя, резервы монеты и встречного токена.
pub fn pair(
    addr: Address,
    coin: Address,
    quote: Address,
    ticker: &str,
    r_coin: u128,
    r_quote: u128,
) -> Entry {
    let f = families::make("pancakeswap_v2", "pancakeswap_v2").unwrap();
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
        asleep: false,
        gen: 0,
    }
}

/// Проверка монеты без налогов.
pub fn clean(ticker: &str, a: Address) -> CoinCheck {
    CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            ticker: ticker.into(),
            bsc_contract: Some(a),
        },
        taxes: CheckState::Known(TokenTaxes {
            buy_pct: Decimal::ZERO,
            sell_pct: Decimal::ZERO,
            transfer_pct: Decimal::ZERO,
        }),
        source: "тест".into(),
        checked_at: Some(Utc::now()),
        wait: None,
    }
}

/// Монеты подписки (USDT и монета ETH — их знает биржа; промежуточной в подписке нет) и допуск пулов: пулы в
/// реестре, газ пробы, сверка с квотером и налоги пула (нулевые) — как в tests/bsc_dex_via.rs.
pub fn prepare(p: &Parts, pools: &[&Entry], usdt: Address, coin: Address) {
    p.tokens.set(&[
        Asset {
            ticker: "USDT".into(),
            bsc_contract: Some(usdt),
        },
        Asset {
            ticker: "ETH".into(),
            bsc_contract: Some(coin),
        },
    ]);
    for e in pools {
        p.registry.put((*e).clone());
        p.checks.update_pool(&e.pool.reference, |f| {
            f.gas_buy = Some(100_000);
            f.gas_sell = Some(110_000);
            f.quoter.buy = Some(Ok(()));
            f.quoter.sell = Some(Ok(()));
            f.taxes = Some(PoolTaxes {
                buy_pct: Decimal::ZERO,
                sell_pct: Some(Decimal::ZERO),
                transfer_pct: Decimal::ZERO,
            });
        });
    }
}
