//! Файл: tests/bsc_dex_multi_pair_routes.rs
//! Что делает: решение 295, тест (в) — один многомонетный пул не идёт дважды в одном маршруте и не делится между долями
//! (по адресу пула, `PoolRef::site`): три пары Ellipsis 3pool (BUSD–USDT, USDC–USDT, BUSD–USDC) как записи
//! `PoolRef::Multi` с состоянием из ЖИВОЙ записи (tests/data/ellipsis_2026-11-15, разбор load_curve::state_with) →
//! граф (pools/graph.rs) → пути route_paths::enumerate и route_best::enumerate (USDT → BUSD через USDC: только прямой),
//! циклы route_paths::enumerate_cycles (по одному пулу — ни одного), graph_seq::shared / unsupported (доли на разных
//! парах одного пула — общий пул, вид без расчёта состояния после обмена — отказ), Via::best (обе ноги через USDC —
//! один пул — не путь).
//! Кто вызывает: cargo test --test bsc_dex_multi_pair_routes.
//! От чего зависит: tests/multi_common, arb::venues::bsc_dex::{pools, quote, parts}.

mod multi_common;

use std::collections::HashSet;
use std::str::FromStr;

use alloy_primitives::Address;
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, CheckState, CoinCheck, Pool, PoolRef, Side, TokenTaxes, VenueId};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::state::PoolTaxes;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::mids::Mid;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::{families, graph, load_curve};
use arb::venues::bsc_dex::quote::route_calc::Leg;
use arb::venues::bsc_dex::quote::select::Usable;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{graph_seq, route_best, route_paths};

use multi_common::{bad, coin, ctx, loader_answers, rec, set_dex};

const DIR: &str = "ellipsis_2026-11-15";

/// Запись реестра пары `coin`–`usdt` 3pool (ссылка Multi, состояние пары из записи). Ошибок нет (паника теста).
fn entry(pool: Address, raw: &load_curve::Raw, coin: Address, usdt: Address) -> Entry {
    let f = families::make("ellipsis", "curve_stableswap").unwrap();
    let (t0, t1) = if coin < usdt {
        (coin, usdt)
    } else {
        (usdt, coin)
    };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Multi {
                contract: pool,
                token0: t0,
                token1: t1,
            },
            token0: t0,
            token1: t1,
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: load_curve::state_with(raw, (t0, t1), &bad).unwrap(),
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin,
        usdt,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
        state_gen: 0,
    }
}

/// Окружение: настройки графа и DEX, три пары 3pool в реестре, проверки пар пройдены (газ и квотер — как у
/// tests/route_common), USDC — промежуточная без налогов. Возвращает: окружение, пул, BUSD, USDC, USDT.
async fn scene(tag: &str) -> (Parts, Address, Address, Address, Address) {
    let probe = rec(DIR, "probe.json");
    let pool = Address::from_str(probe["pool"].as_str().unwrap()).unwrap();
    let coins = rec(DIR, "coins.json");
    let (busd, usdc, usdt) = (
        coin(&coins, "BUSD"),
        coin(&coins, "USDC"),
        coin(&coins, "USDT"),
    );
    let (calls, tail) = load_curve::calls(pool);
    let (head, t) = loader_answers(
        &probe,
        &calls,
        tail,
        ["ellipsis coins", "ellipsis balances"],
        &["ellipsis A()", "ellipsis fee()"],
    );
    let decimals = multi_common::decimals(&probe, "ellipsis", "coins", 3);
    let n = head.len();
    let raw = load_curve::Raw {
        pool,
        got: head.into_iter().chain(t).collect(),
        tail: n,
        decimals,
    };
    let c = ctx(tag).await;
    let v = Scope::Venue("bsc_dex".into());
    for (k, val) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.min_pool_liquidity_usd", json!(1)),
        ("dex.depth_band_pct", json!(5)),
        ("dex.depth_hysteresis_pct", json!(0)),
        ("dex.route_max_pools_per_edge", json!(5)),
        ("dex.route_max_hops", json!(3)),
        ("dex.route_max_paths", json!(50)),
    ] {
        c.settings
            .set(k, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    // Пороги налога пула в котировке (решение 146) — как tests/route_common tax_limits; налоги пар записи нулевые.
    for k in ["tax.max_buy_pct", "tax.max_sell_pct"] {
        c.settings
            .set(k, Scope::Global, Some(json!("0")), "тест")
            .await
            .unwrap();
    }
    for (k, val) in [
        ("dex.kind", json!("curve_stableswap")),
        ("dex.pool_enabled", json!(true)),
        ("dex.stable_pools", json!([format!("{pool:#x}")])),
        ("dex.multi_pair", json!(true)),
    ] {
        set_dex(&c, "ellipsis", k, val).await;
    }
    let p = Parts::new(c);
    p.tokens.set(&[
        Asset {
            chain: None,
            ticker: "USDT".into(),
            contract: Some(usdt),
        },
        Asset {
            chain: None,
            ticker: "BUSD".into(),
            contract: Some(busd),
        },
    ]);
    for (a, d) in [busd, usdc, usdt]
        .into_iter()
        .zip(multi_common::decimals(&probe, "ellipsis", "coins", 3))
    {
        p.tokens.remember_decimals(a, d);
    }
    let zero = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    p.checks.set_coin(CoinCheck {
        venue: VenueId("bsc_dex".into()),
        asset: Asset {
            chain: None,
            ticker: "USDC".into(),
            contract: Some(usdc),
        },
        taxes: CheckState::Known(zero),
        source: "тест".into(),
        checked_at: Some(Utc::now()),
        wait: None,
    });
    p.mids.set(vec![Mid {
        address: usdc,
        ticker: "USDC".into(),
    }]);
    for e in [
        entry(pool, &raw, busd, usdt),
        entry(pool, &raw, usdc, usdt),
        entry(pool, &raw, busd, usdc),
    ] {
        let r = e.pool.reference.clone();
        p.registry.put(e);
        p.checks.update_pool(&r, |f| {
            f.gas_buy = Some(120_000);
            f.gas_sell = Some(120_000);
            f.quoter.buy = Some(Ok(()));
            f.quoter.sell = Some(Ok(()));
            f.taxes = Some(PoolTaxes {
                one_share_only:false,
                buy_pct: Decimal::ZERO,
                sell_pct: Some(Decimal::ZERO),
                transfer_pct: Decimal::ZERO,
            });
        });
    }
    (p, pool, busd, usdc, usdt)
}

/// Площадки шагов пути. Ошибок нет.
fn sites(p: &route_paths::RoutePath) -> Vec<PoolRef> {
    p.steps
        .iter()
        .map(|s| s.edge.entry.pool.reference.site())
        .collect()
}

#[tokio::test]
async fn one_pool_one_step_in_paths_and_cycles() {
    let (p, pool, busd, usdc, usdt) = scene("routes-paths").await;
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18).expect("граф");
    assert_eq!(g.len(), 3, "три пары пула — три ребра");
    let allowed = HashSet::from([usdc]);
    for paths in [
        route_paths::enumerate(&g, usdt, busd, &allowed, 3, 50),
        route_best::enumerate(&g, usdt, busd, &allowed, 3, 50),
    ] {
        // USDT → USDC → BUSD шёл бы двумя парами одного пула — запрещён; остаётся прямая пара.
        assert_eq!(paths.len(), 1, "только прямой путь");
        assert_eq!(sites(&paths[0]), vec![PoolRef::Contract(pool)]);
    }
    for base in [usdt, busd, usdc] {
        assert!(
            route_paths::enumerate_cycles(&g, base, 4, 50).is_empty(),
            "цикл по одному пулу — нет"
        );
    }
}

#[tokio::test]
async fn shared_pool_across_shares_and_via() {
    let (p, pool, busd, usdc, usdt) = scene("routes-shared").await;
    let reg = |a: Address, b: Address| p.registry.by_pair(a, b).remove(0);
    let usable = Usable {
        one_share_only:false,
        gas: 120_000,
        hook_pct: Decimal::ZERO,
        tax_pct: Decimal::ZERO,
        hook: Default::default(),
    };
    let leg = |e: Entry, i: Address, o: Address| Leg {
        entry: e,
        usable: usable.clone(),
        token_in: i,
        token_out: o,
        dec_in: 18,
        dec_out: 18,
    };
    let a = vec![leg(reg(busd, usdt), usdt, busd)];
    let b = vec![
        leg(reg(usdc, usdt), usdt, usdc),
        leg(reg(busd, usdc), usdc, busd),
    ];
    let common = graph_seq::shared(&[&a, &b]);
    assert_eq!(
        common,
        HashSet::from([PoolRef::Contract(pool)]),
        "разные пары одного пула — общий пул"
    );
    assert!(
        graph_seq::unsupported(&[&a, &b], &common).is_some(),
        "StableSwap: состояние после обмена не считается"
    );
    let q = p.quoting();
    let zero = TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    };
    let v = Via {
        q: &q,
        coin: busd,
        usdt,
        side: Side::Buy,
        taxes: &zero,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let mid = Mid {
        address: usdc,
        ticker: "USDC".into(),
    };
    let Err(why) = v.best(&[(mid, 18)], Decimal::from(10), &|_| true, &|_, _| Ok(())) else {
        panic!("через промежуточную пути нет");
    };
    assert!(why.iter().any(|w| w.contains("один пул")), "{why:?}");
}
