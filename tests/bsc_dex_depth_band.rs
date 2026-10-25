//! Файл: tests/bsc_dex_depth_band.rs
//! Что делает: решение 73 — глубина пула по направлениям в полосе цены на живой записи 2026-10-25
//! (tests/data/depth_2026-10-25/responses.json, рекордер tests/recorders/record_depth.js; у каждого пула свой блок):
//! - пулы USDT монет ZKP, OPN, IOST (Infinity CL и Uniswap V4, в том числе tickSpacing 1): глубина покупки и продажи
//!   в полосе считается, полоса прочитана целиком, шире полоса — не меньше глубина; если внутри полосы нет
//!   установленных тиков — глубина совпадает с формулой одного диапазона (Δx = L·(√P − √Pₑ)/(√P·√Pₑ),
//!   Δy = L·(√Pₑ − √P), вход — с комиссией);
//! - AAVE Uniswap V3 0x8e3ecc0b…: продажа монеты по всей прочитанной карте (±8 слов) отдаёт 94,3% USDT пула
//!   (balanceOf USDT пула на том же блоке — 1915,19; живой замер 25.10 днём — 2044,75): не больше баланса; остальное —
//!   за прочитанными словами и невыплаченные комиссии позиций.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common (окружение на настоящей SQLite), arb::venues::bsc_dex::{quote::{depth_band,
//! pool_quote}, pools::{abi_v4, abi_v2v3, ticks, families}}.

mod native_common;

use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, U256};
use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{PoolRef, PoolState};
use arb::venues::bsc_dex::pools::abi_v2v3::{ITickLens, IUniswapV3Pool, IV3Pool};
use arb::venues::bsc_dex::pools::abi_v4::{IInfinityCl, IStateView};
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::depth_band::band_units;
use arb::venues::bsc_dex::quote::pool_quote;

const USDT: &str = "0x55d398326f99059ff775485246999027b3197955";

/// Записанные ответы: имя вызова → байты результата.
struct Rec {
    got: BTreeMap<String, Vec<u8>>,
    pools: Vec<Value>,
}

fn rec() -> Rec {
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/depth_2026-10-25/responses.json").unwrap(),
    )
    .unwrap();
    let mut got = BTreeMap::new();
    for r in v["results"].as_array().unwrap() {
        if let Some(s) = r["response"]["result"].as_str() {
            got.insert(
                r["name"].as_str().unwrap().to_string(),
                alloy_primitives::hex::decode(s).unwrap(),
            );
        }
    }
    Rec {
        got,
        pools: v["pools"].as_array().unwrap().clone(),
    }
}

/// Слово ABI `i` (32 байта) как адрес. Ошибок нет (запись — по форме).
fn addr_word(b: &[u8], i: usize) -> Address {
    Address::from_slice(&b[i * 32 + 12..i * 32 + 32])
}

/// Пул CL (Infinity CL или Uniswap V4) из записи как запись реестра бота. None — встречный токен не USDT.
fn cl_entry(r: &Rec, p: &Value) -> Option<Entry> {
    let tag = p["tag"].as_str().unwrap();
    let infinity = p["kind"] == "infinity_cl";
    let key = if infinity {
        &r.got[&format!("{tag} CLPoolManager poolIdToPoolKey")]
    } else {
        &r.got[&format!("{tag} PositionManager poolKeys")]
    };
    let (c0, c1) = (addr_word(key, 0), addr_word(key, 1));
    let usdt = Address::from_str(USDT).unwrap();
    let coin = if c0 == usdt {
        c1
    } else if c1 == usdt {
        c0
    } else {
        return None;
    };
    let id = B256::from_str(p["id"].as_str().unwrap()).unwrap();
    let spacing = p["spacing"].as_i64().unwrap() as i32;
    let (sqrt, tick, protocol_fee, lp_fee, liquidity) = if infinity {
        let s = IInfinityCl::getSlot0Call::abi_decode_returns(
            &r.got[&format!("{tag} CLPoolManager getSlot0")],
        )
        .unwrap();
        let l = IInfinityCl::getLiquidityCall::abi_decode_returns(
            &r.got[&format!("{tag} CLPoolManager getLiquidity")],
        )
        .unwrap();
        (s.sqrtPriceX96, s.tick, s.protocolFee, s.lpFee, l)
    } else {
        let s = IStateView::getSlot0Call::abi_decode_returns(
            &r.got[&format!("{tag} StateView getSlot0")],
        )
        .unwrap();
        let l = IStateView::getLiquidityCall::abi_decode_returns(
            &r.got[&format!("{tag} StateView getLiquidity")],
        )
        .unwrap();
        (s.sqrtPriceX96, s.tick, s.protocolFee, s.lpFee, l)
    };
    let words = p["words"].as_array().unwrap();
    let words = (
        words[0].as_i64().unwrap() as i16,
        words[1].as_i64().unwrap() as i16,
    );
    let mut ticks = BTreeMap::new();
    let prefix = if infinity {
        format!("{tag} CLPoolManager getPoolTickInfo ")
    } else {
        format!("{tag} StateView getTickLiquidity ")
    };
    for (name, out) in r.got.range(prefix.clone()..) {
        let Some(t) = name.strip_prefix(&prefix) else {
            break;
        };
        let t: i32 = t.parse().unwrap();
        let (gross, net) = if infinity {
            let i = IInfinityCl::getPoolTickInfoCall::abi_decode_returns(out).unwrap();
            (i.liquidityGross, i.liquidityNet)
        } else {
            let i = IStateView::getTickLiquidityCall::abi_decode_returns(out).unwrap();
            (i.liquidityGross, i.liquidityNet)
        };
        ticks.insert(t, TickData { gross, net });
    }
    let kind = if infinity {
        "pancakeswap_infinity_cl"
    } else {
        "uniswap_v4"
    };
    let mut e = native_common::entry(kind, None);
    e.pool.reference = PoolRef::Singleton {
        manager: Address::ZERO,
        id,
    };
    e.pool.token0 = c0;
    e.pool.token1 = c1;
    e.pool.tick_spacing = Some(spacing);
    e.pool.state = PoolState::Cl {
        sqrt_price_x96: sqrt.to::<U256>(),
        tick: tick.as_i32(),
        liquidity,
        protocol_fee: protocol_fee.to(),
        lp_fee: lp_fee.to(),
    };
    e.ticker = p["coin"].as_str().unwrap().into();
    e.coin = coin;
    e.usdt = usdt;
    e.ticks = Some(TickMap {
        spacing,
        ticks,
        words,
    });
    Some(e)
}

/// Единицы USDT (18 знаков в BSC) → Decimal.
fn usd(u: U256) -> Decimal {
    Decimal::from_str(&u.to_string()).unwrap() / Decimal::from(10u64.pow(18))
}

fn f(u: U256) -> f64 {
    u.to_string().parse().unwrap()
}

/// Глубина одного диапазона (внутри полосы нет установленных тиков): (покупка — USDT в пул, продажа — USDT из пула)
/// по формулам Δx, Δy шапки; вход — с комиссией `fee` пипсов. Ошибок нет.
fn closed_form(e: &Entry, pct: f64, fee_buy: u32) -> (f64, f64) {
    let PoolState::Cl {
        sqrt_price_x96,
        liquidity,
        ..
    } = e.pool.state
    else {
        unreachable!()
    };
    let q = 2f64.powi(96);
    let s = f(sqrt_price_x96) / q;
    let l = liquidity as f64;
    let coin0 = e.coin == e.pool.token0;
    let tick = |up: bool| {
        let b: f64 = pct / 100.0;
        let r = if up { (1.0 + b).ln() } else { -(1.0 - b).ln() };
        (r / 1.0001f64.ln()).floor() as i32
    };
    let PoolState::Cl { tick: t, .. } = e.pool.state else {
        unreachable!()
    };
    let at = |k: i32| 1.0001f64.powf(k as f64 / 2.0);
    let gross = |x: f64| x / (1.0 - fee_buy as f64 / 1e6);
    if coin0 {
        // Монета — токен0: продажа — цена (токен1/токен0) вниз, из пула Δy; покупка — вверх, в пул Δy с комиссией.
        let down = at(t - tick(false));
        let up = at(t + tick(true));
        (gross(l * (up - s)), l * (s - down))
    } else {
        // Монета — токен1: продажа — цена вверх (монеты в пул), из пула Δx USDT; покупка — вниз, в пул Δx.
        let up = at(t + tick(false));
        let down = at(t - tick(true));
        (gross(l * (s - down) / (s * down)), l * (up - s) / (s * up))
    }
}

#[tokio::test]
async fn cl_band_depth_zkp_opn_iost() {
    let ctx = native_common::ctx("depth-band").await;
    let r = rec();
    let (mut seen, mut formula) = (0, 0);
    for p in r.pools.iter().filter(|p| {
        matches!(p["coin"].as_str(), Some("ZKP" | "OPN" | "IOST"))
            && matches!(p["kind"].as_str(), Some("infinity_cl" | "uni_v4"))
    }) {
        let Some(e) = cl_entry(&r, p) else { continue };
        let PoolState::Cl {
            liquidity,
            protocol_fee,
            lp_fee,
            tick,
            ..
        } = e.pool.state
        else {
            unreachable!()
        };
        if liquidity == 0 {
            continue;
        }
        seen += 1;
        let tag = p["tag"].as_str().unwrap();
        let (buy, sell, full) = band_units(&ctx, &e, Decimal::from(2)).unwrap();
        let (buy5, sell5, _) = band_units(&ctx, &e, Decimal::from(5)).unwrap();
        eprintln!(
            "{tag}: tickSpacing {}, ±2%: покупка {} USDT, продажа {} USDT; ±5%: {} / {}",
            p["spacing"],
            usd(buy).round_dp(2),
            usd(sell).round_dp(2),
            usd(buy5).round_dp(2),
            usd(sell5).round_dp(2)
        );
        assert!(full, "{tag}: полоса ±2% внутри прочитанных слов");
        assert!(
            buy5 >= buy && sell5 >= sell,
            "{tag}: шире полоса — не меньше глубина"
        );
        // Внутри ±2% нет установленных тиков — одна ликвидность, формула сходится.
        let ticks = &e.ticks.as_ref().unwrap().ticks;
        let (lo, hi) = (tick - 210, tick + 210);
        if ticks.range(lo..=hi).next().is_none() {
            let coin0 = e.coin == e.pool.token0;
            let fee = pool_quote::swap_fee(protocol_fee, lp_fee, !coin0);
            let (b, s) = closed_form(&e, 2.0, fee);
            formula += 1;
            let rel = |x: f64, y: f64| ((x - y) / y).abs();
            assert!(
                rel(f(buy), b) < 1e-6,
                "{tag}: покупка {} против формулы {b}",
                f(buy)
            );
            assert!(
                rel(f(sell), s) < 1e-6,
                "{tag}: продажа {} против формулы {s}",
                f(sell)
            );
        }
    }
    assert!(seen >= 5, "пулов USDT с ликвидностью в записи: {seen}");
    assert!(
        formula >= 3,
        "пулов без тиков в полосе (сверка с формулой): {formula}"
    );
}

#[tokio::test]
async fn aave_v3_sell_depth_matches_pool_usdt_balance() {
    let ctx = native_common::ctx("depth-aave").await;
    let r = rec();
    let p = r
        .pools
        .iter()
        .find(|p| p["addr"] == "0x8e3ecc0b261f1a4db62321090575eb299844f077")
        .unwrap();
    let tag = p["tag"].as_str().unwrap();
    let s = IUniswapV3Pool::slot0Call::abi_decode_returns(&r.got[&format!("{tag} slot0")]).unwrap();
    let liquidity =
        IV3Pool::liquidityCall::abi_decode_returns(&r.got[&format!("{tag} liquidity")]).unwrap();
    let spacing = p["spacing"].as_i64().unwrap() as i32;
    let words = p["words"].as_array().unwrap();
    let words = (
        words[0].as_i64().unwrap() as i16,
        words[1].as_i64().unwrap() as i16,
    );
    let mut ticks = BTreeMap::new();
    for k in words.0..=words.1 {
        let out = &r.got[&format!("{tag} TickLens word {k}")];
        for t in ITickLens::getPopulatedTicksInWordCall::abi_decode_returns(out).unwrap() {
            ticks.insert(
                t.tick.as_i32(),
                TickData {
                    gross: t.liquidityGross,
                    net: t.liquidityNet,
                },
            );
        }
    }
    // Адрес AAVE — calls.json (coins.AAVE).
    let aave = Address::from_str("0xfb6115445bff7b52feb98650c87f44907e58f802").unwrap();
    let usdt = Address::from_str(USDT).unwrap();
    // Uniswap V3: token0 < token1 по адресу.
    let (t0, t1) = if aave < usdt {
        (aave, usdt)
    } else {
        (usdt, aave)
    };
    // fee() пула рекордер не читал: tickSpacing 60 у Uniswap V3 — уровень 3000 (UniswapV3Factory, конструктор:
    // feeAmountTickSpacing[3000] = 60). Комиссия влияет на вход, а проверка — по выходу.
    assert_eq!(spacing, 60);
    let mut e = native_common::entry("uniswap_v3", Some(3000));
    e.pool.token0 = t0;
    e.pool.token1 = t1;
    e.pool.tick_spacing = Some(spacing);
    e.pool.state = PoolState::V3 {
        sqrt_price_x96: s.sqrtPriceX96.to::<U256>(),
        liquidity,
        tick: s.tick.as_i32(),
    };
    e.ticker = "AAVE".into();
    e.coin = aave;
    e.usdt = usdt;
    e.ticks = Some(TickMap {
        spacing,
        ticks,
        words,
    });
    let bal = U256::from_be_slice(&r.got[&format!("{tag} USDT balanceOf")]);
    let all = pool_quote::swap(&ctx, &e, aave, true, U256::from(u128::MAX)).unwrap();
    let (band_buy, band_sell, _) = band_units(&ctx, &e, Decimal::from(2)).unwrap();
    eprintln!(
        "AAVE 0x8e3ecc0b… блок {}: USDT пула {} — продажа по всей карте {} USDT; ±2%: покупка {} / продажа {}",
        p["block"],
        usd(bal).round_dp(2),
        usd(all.amount_out).round_dp(2),
        usd(band_buy).round_dp(2),
        usd(band_sell).round_dp(2)
    );
    assert!(
        all.amount_out <= bal,
        "из пула не уходит больше, чем в нём USDT"
    );
    // На блоке записи в пуле 1915,19 USDT (25.10 днём было 2044,75), по карте ±8 слов бот отдаёт 1805,81 (94,3%):
    // остальное — в позициях выше прочитанных слов (AAVE — токен1, продажа двигает тик вверх; карта кончилась —
    // complete = false) и в невыплаченных комиссиях позиций, которые лежат на балансе пула, но обменом не отдаются.
    assert!(!all.complete, "карта кончилась раньше суммы");
    let share = f(all.amount_out) / f(bal);
    assert!(
        (0.94..1.0).contains(&share),
        "доля USDT пула в прочитанной карте: {share}"
    );
    assert!(band_sell <= all.amount_out);
}
