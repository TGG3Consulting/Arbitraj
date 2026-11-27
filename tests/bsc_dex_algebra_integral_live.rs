//! Файл: tests/bsc_dex_algebra_integral_live.rs
//! Что проверяет: решение 188 — Algebra Integral (THENA V3) на РЕАЛЬНЫХ ответах узла BSC, записанных на одном блоке
//! 132036268 (tests/data/algebra_integral_2026-11-06/probe.json): два пула thena-v3 (BTCB/WBNB, SINGULARRY/WBNB).
//! 1. ABI: globalState (6 слов: price, tick, lastFee, pluginConfig, communityFee, unlocked) и ticks (6 слов: liquidityTotal,
//!    liquidityDelta, prevTick, nextTick, …) разбираются по integral-core 1.2.x; список тиков идёт от MIN_TICK до MAX_TICK.
//! 2. Окно карты (load_algebra_integral::window) накрывает текущий тик.
//! 3. Наша математика (math/swap_algebra.rs) на этой карте и комиссии fee() пула даёт тот же выход, что виртуальный
//!    квотер (настоящий swap пула с плагином, решение 186) — на 1, 100 и 10000 долларов, в обе стороны.
//!
//! 4. Решение 205: глубина в полосе (quote/depth_band_algebra.rs) на тех же пулах — обе стороны не нулевые, и обмен
//!    найденного входа без предела цены (без комиссии) даёт тот же выход (полоса — честный предел обмена).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/algebra_integral_common (запись, разбор, состояние пула), arb::venues::bsc_dex::math::swap_algebra.

mod algebra_integral_common;
mod native_common;

use alloy_primitives::U256;

use arb::venues::bsc_dex::math::swap_algebra::{self, AlgebraState, NextTick};

use algebra_integral_common::{pool_state, probe, word};

#[test]
fn integral_executor_quotes_and_sender_specific_fee() {
    use alloy::rpc::types::Log;
    use arb::venues::bsc_dex::pools::events::{self, Applied};
    use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::str::FromStr;

    let number = |v: &Value| U256::from_str(v.as_str().unwrap()).unwrap();
    let sequential: Value = serde_json::from_str(include_str!(
        "data/first_run_dex_2026-11-26/hydrex_sequential_base.json"
    ))
    .unwrap();
    for (kind, fixture, count) in [
        (
            "algebra_integral_hydrex",
            include_str!("data/first_run_dex_2026-11-26/hydrex_base.json"),
            6,
        ),
        (
            "algebra_integral_quickswap",
            include_str!("data/first_run_dex_2026-11-26/quickswap_base.json"),
            2,
        ),
    ] {
        let data: Value = serde_json::from_str(fixture).unwrap();
        let markets = data["markets"].as_array().unwrap();
        let mut special = 0;
        assert_eq!(markets.len(), 3);
        for m in markets {
            if kind == "algebra_integral_quickswap" {
                assert_ne!(
                    data["active_plugin_factory"],
                    data["addresses"]["PluginFactory"]
                );
                assert_ne!(m["plugin"], m["previous_plugin"]);
                let profiles: Value =
                    serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
                let settings = &profiles
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|row| row["id"] == "quickswap-v4-base")
                    .unwrap()["settings"];
                assert_eq!(
                    settings["dex.plugin_runtime_hashes"],
                    serde_json::json!([data["proxy_template_hash"]])
                );
                let evidence = data["sources"]
                    .as_object()
                    .unwrap()
                    .values()
                    .find_map(|proof| proof["immutable_offset"].as_u64())
                    .unwrap();
                assert_eq!(settings["dex.plugin_pool_offset"].as_u64(), Some(evidence));
            }
            let mut ticks = BTreeMap::new();
            for (index, row) in m["ticks"].as_object().unwrap() {
                let gross = row["gross"].as_str().unwrap().parse::<u128>().unwrap();
                if gross > 0 {
                    ticks.insert(
                        index.parse().unwrap(),
                        TickData {
                            gross,
                            net: row["net"].as_str().unwrap().parse().unwrap(),
                        },
                    );
                }
            }
            let map = TickMap {
                words: (
                    m["lower_word"].as_i64().unwrap() as i16,
                    m["upper_word"].as_i64().unwrap() as i16,
                ),
                spacing: 1,
                ticks: ticks.into(),
            };
            let state = AlgebraState {
                sqrt_price: number(&m["price"]),
                tick: m["tick"].as_i64().unwrap() as i32,
                liquidity: m["liquidity"].as_str().unwrap().parse().unwrap(),
                fee: m["current_fee"].as_u64().unwrap() as u32,
                ticks: &map,
                next: NextTick::Linked,
            };
            let cases = m["cases"].as_array().unwrap();
            if m["pool"] == sequential["pool"] {
                use arb::core::types::PoolState;
                use arb::venues::bsc_dex::quote::pool_after;
                let mut entry = native_common::entry("algebra_integral_hydrex", Some(state.fee));
                entry.pool.state = PoolState::Algebra {
                    sqrt_price_x96: state.sqrt_price,
                    tick: state.tick,
                    liquidity: state.liquidity,
                    fee: state.fee,
                    fee_otz: None,
                };
                entry.ticks = Some(map.clone());
                assert!(!pool_after::appliable(&entry));
                let pairs = sequential["cases"].as_array().unwrap();
                assert_eq!(pairs.len(), 4);
                for c in pairs {
                    let logs = c["events"].as_array().unwrap();
                    let out = |after: &Value| {
                        let next = AlgebraState {
                            sqrt_price: number(&after["price"]),
                            tick: after["tick"].as_i64().unwrap() as i32,
                            liquidity: after["liquidity"].as_str().unwrap().parse().unwrap(),
                            fee: state.fee,
                            ticks: &map,
                            next: NextTick::Linked,
                        };
                        let r = swap_algebra::swap(
                            &next,
                            c["zero_for_one"].as_bool().unwrap(),
                            true,
                            number(&c["dx"]),
                        )
                        .unwrap();
                        assert!(r.complete);
                        r.amount_out
                    };
                    assert_eq!(logs[0]["sender"], sequential["executor"]);
                    assert_ne!(logs[1]["sender"], sequential["executor"]);
                    assert_eq!(logs[2]["sender"], sequential["executor"]);
                    let second = number(&c["output"][1]);
                    assert_ne!(out(&logs[0]), second);
                    assert_eq!(out(&logs[1]), second);
                    assert_eq!(c["results"]["above"]["status"], "0x0");
                    assert_eq!(
                        number(&c["results"]["exact"]["returnData"]),
                        number(&c["output"][0]) + second
                    );
                }
            }
            assert_eq!(cases.len(), 4);
            for case in cases {
                let amount = number(&case["dx"]);
                let quote =
                    swap_algebra::swap(&state, case["source"] == m["tokens"][0], true, amount)
                        .unwrap();
                assert!(quote.complete);
                assert_eq!(quote.amount_in, amount);
                assert_eq!(quote.amount_out, number(&case["at_block"][0]));
                let result = &case["results"];
                let out = number(&case["out"]);
                let returned = |name: &str| number(&result[name]["returnData"]);
                assert_eq!(returned("exact"), out);
                assert_eq!(returned("out_after") - returned("out_before"), out);
                assert_eq!(returned("in_before") - returned("in_after"), amount);
                assert_eq!(result["above"]["status"], "0x0");
                if case["event_fees"].as_array().unwrap().len() > 1 {
                    let discounted = case["event_fees"].as_array().unwrap().last().unwrap();
                    assert_eq!(discounted[0], 1);
                    assert_ne!(case["next_fee"], discounted[0]);
                    let mut entry = native_common::entry(kind, None);
                    let log = result["exact"]["logs"].as_array().unwrap().iter().rev()
                    .find(|l| l["address"] == m["pool"] && l["topics"][0] ==
                          format!("{:#x}", alloy_primitives::keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24,uint24,uint24)")))
                    .unwrap();
                    let log: Log = serde_json::from_value(log.clone()).unwrap();
                    assert!(matches!(
                        events::apply(&mut entry, &log),
                        Applied::Refresh(_)
                    ));
                    assert!(!entry.valid);
                    special += 1;
                }
            }
        }
        assert_eq!(special, count);
    }
}

#[test]
fn abi_and_window_match_live_pools() {
    let d = probe();
    for p in d["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["dex_id"] == "thena-v3")
    {
        let gs = p["calls"]["globalState"]["raw"].as_str().unwrap();
        // 6 слов globalState (integral-core 1.2.x), unlocked = 1.
        assert_eq!(gs.trim_start_matches("0x").len(), 64 * 6, "{}", p["pool"]);
        assert_eq!(word(gs, 5), U256::from(1u8));
        let (_, tick, _, _, m) = pool_state(p);
        let (lo, hi) = (i32::from(m.words.0) * 256, i32::from(m.words.1) * 256 + 255);
        assert!(
            lo <= tick && tick <= hi,
            "{}: тик {tick} вне [{lo}; {hi}]",
            p["pool"]
        );
    }
}

#[test]
fn local_math_equals_virtual_quoter() {
    let d = probe();
    for p in d["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["dex_id"] == "thena-v3")
    {
        let (sqrt, tick, liq, fee, m) = pool_state(p);
        let s = AlgebraState {
            sqrt_price: sqrt,
            tick,
            liquidity: liq,
            fee,
            ticks: &m,
            next: NextTick::Linked,
        };
        for q in p["quoter"].as_array().unwrap() {
            let raw = q["raw"].as_str().unwrap();
            let (used, out) = (word(raw, 0), word(raw, 1));
            let amount = U256::from_str_radix(&q["amountIn"].to_string(), 10).unwrap();
            assert_eq!(used, amount, "{}: квотер взял не весь вход", p["pool"]);
            let zfo = q["zeroForOne"].as_bool().unwrap();
            let r = swap_algebra::swap(&s, zfo, true, amount).expect("расчёт");
            assert!(
                r.complete,
                "{} {zfo} {}: не исполнено целиком",
                p["pool"], q["usd"]
            );
            assert_eq!(
                r.amount_out, out,
                "{} zeroForOne={zfo} ${}: наша математика ≠ виртуальный квотер",
                p["pool"], q["usd"]
            );
        }
    }
}

#[test]
fn band_depth_algebra_on_live_pools() {
    let d = probe();
    for p in d["pools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["dex_id"] == "thena-v3")
    {
        let (sqrt, tick, liq, _, m) = pool_state(p);
        let s = AlgebraState {
            sqrt_price: sqrt,
            tick,
            liquidity: liq,
            fee: 0,
            ticks: &m,
            next: NextTick::Linked,
        };
        for zfo in [true, false] {
            let (a_in, a_out, _) =
                arb::venues::bsc_dex::quote::depth_band_algebra::algebra(&s, zfo, 0.02, !zfo)
                    .expect("глубина Algebra");
            assert!(
                !a_in.is_zero() && !a_out.is_zero(),
                "{} {zfo}: глубина 0",
                p["pool"]
            );
            let r = swap_algebra::swap(&s, zfo, true, a_in).expect("обмен");
            let diff = if r.amount_out > a_out {
                r.amount_out - a_out
            } else {
                a_out - r.amount_out
            };
            assert!(
                diff <= a_out / U256::from(1_000_000u32) + U256::from(1u8),
                "{} {zfo}: обмен {} ≠ глубина {}",
                p["pool"],
                r.amount_out,
                a_out
            );
        }
    }
}
