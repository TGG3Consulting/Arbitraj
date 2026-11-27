//! Сверка трёх фабрик Aerodrome Base по записанным eth_call на одном каноническом блоке.
//! Проверяет ABI текущих загрузчиков, локальный расчёт обоих направлений и адреса профилей.
use std::collections::BTreeMap;

use alloy::sol_types::SolCall;
use alloy_primitives::{U256, hex};
use arb::venues::bsc_dex::math::swap_cl::{ClState, Version, swap};
use arb::venues::bsc_dex::node::abi_dex::ISlipstreamQuoterV2;
use arb::venues::bsc_dex::pools::abi_slipstream::ICLPool;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::load_cl::set_ticks;
use arb::venues::bsc_dex::pools::per_block;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use serde::Deserialize;

#[derive(Deserialize)]
struct Capture {
    pools: Vec<Pool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pool {
    pool: String,
    factory: String,
    quoter: String,
    spacing: i32,
    slot0: String,
    fee: u32,
    liquidity: String,
    word: i16,
    bitmap: String,
    sampled_ticks: BTreeMap<i32, String>,
    quotes: Vec<Quote>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Quote {
    zero_for_one: bool,
    amount_in: String,
    raw: String,
}

/// Декодирует ответ контракта тем же ABI, которым пользуется котировщик.
fn decode<C: SolCall>(raw: &str) -> C::Return {
    C::abi_decode_returns(&hex::decode(raw).unwrap()).unwrap()
}

#[test]
fn local_output_matches_each_deployed_quoter_in_both_directions() {
    let data: Capture =
        serde_json::from_str(include_str!("data/base_slipstream_2026-11-26/quotes.json")).unwrap();
    let execution: serde_json::Value = serde_json::from_str(include_str!(
        "data/base_slipstream_2026-11-26/executor.json"
    ))
    .unwrap();
    let cases = execution["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 16);
    let profiles: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    assert_eq!(data.pools.len(), 3);
    for p in data.pools {
        let profile = profiles
            .iter()
            .find(|x| {
                x["chain_id"] == 8453
                    && x["settings"]["dex.factory"]
                        .as_str()
                        .is_some_and(|f| f.eq_ignore_ascii_case(&p.factory))
            })
            .unwrap();
        assert!(
            profile["settings"]["dex.quoter"]
                .as_str()
                .unwrap()
                .eq_ignore_ascii_case(&p.quoter)
        );
        let family = families::make(
            profile["id"].as_str().unwrap(),
            profile["settings"]["dex.kind"].as_str().unwrap(),
        )
        .unwrap();
        assert!(per_block::touched(family));
        let slot = decode::<ICLPool::slot0Call>(&p.slot0);
        let word = decode::<ICLPool::tickBitmapCall>(&p.bitmap);
        let initialized = set_ticks(p.word, &[word], p.spacing);
        assert_eq!(
            initialized,
            p.sampled_ticks.keys().copied().collect::<Vec<_>>()
        );
        let ticks = p
            .sampled_ticks
            .iter()
            .map(|(&k, raw)| {
                let t = decode::<ICLPool::ticksCall>(raw);
                assert!(t.initialized);
                (
                    k,
                    TickData {
                        gross: t.liquidityGross,
                        net: t.liquidityNet,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let map = TickMap {
            spacing: p.spacing,
            ticks: ticks.into(),
            words: (p.word, p.word),
        };
        let state = ClState {
            sqrt_price: U256::from(slot.sqrtPriceX96),
            tick: slot.tick.as_i32(),
            liquidity: p.liquidity.parse().unwrap(),
            fee: p.fee,
            ticks: &map,
            version: Version::V3,
        };
        assert_eq!(
            p.quotes.iter().map(|q| q.zero_for_one).collect::<Vec<_>>(),
            [true, false]
        );
        // Реальные ответы последовательной eth_simulateV1: обе стороны, два или три размера.
        // Сопоставляем расчёт с квотером в том же блоке, фактической оплатой входа и полученным выходом.
        let measured: Vec<_> = cases.iter().filter(|c| c["pool"] == p.pool).collect();
        for direction in [false, true] {
            assert!(
                measured
                    .iter()
                    .filter(|c| c["forward"] == direction)
                    .count()
                    >= 2
            );
        }
        for c in measured {
            let results = &c["results"];
            assert!(
                results
                    .as_object()
                    .unwrap()
                    .values()
                    .all(|r| r["status"] == "0x1")
            );
            let amount = U256::from_str_radix(c["amount"].as_str().unwrap(), 10).unwrap();
            let n = |k: &str| {
                U256::from_str_radix(
                    results[k]["returnData"]
                        .as_str()
                        .unwrap()
                        .trim_start_matches("0x"),
                    16,
                )
                .unwrap()
            };
            let observed = decode::<ISlipstreamQuoterV2::quoteExactInputSingleCall>(
                results["quote"]["returnData"].as_str().unwrap(),
            );
            let model = swap(&state, c["forward"].as_bool().unwrap(), true, amount).unwrap();
            assert!(model.complete);
            assert_eq!(model.amount_out, observed.amountOut);
            assert_eq!(n("owner_in_before") - n("owner_in_after"), amount);
            assert_eq!(
                n("owner_out_after") - n("owner_out_before"),
                observed.amountOut
            );
            assert_eq!(n("execute"), observed.amountOut);
            assert_eq!(
                (n("executor_input"), n("executor_output")),
                (U256::ZERO, U256::ZERO)
            );
        }
        for q in p.quotes {
            let input = U256::from_str_radix(&q.amount_in, 10).unwrap();
            let observed = decode::<ISlipstreamQuoterV2::quoteExactInputSingleCall>(&q.raw);
            let model = swap(&state, q.zero_for_one, true, input).unwrap();
            assert!(model.complete);
            assert_eq!(model.amount_in, input);
            assert_eq!(model.amount_out, observed.amountOut, "{}", p.factory);
        }
    }
}

#[test]
fn each_legacy_factory_can_discover_registered_spacing_without_an_indexer() {
    let evidence:serde_json::Value=serde_json::from_str(include_str!("data/first_run_dex_2026-11-26/early_slipstream_base.json")).unwrap();
    let profiles:Vec<serde_json::Value>=serde_json::from_str(include_str!("../config/first_run_dex.json")).unwrap();
    for (id,spacing) in evidence["registry_spacings"].as_object().unwrap(){
        let profile=profiles.iter().find(|p|p["id"]==id.as_str()).unwrap();
        let settings=&profile["settings"];
        let actual:Vec<i64>=settings["dex.cl_tick_spacings"].as_array().unwrap().iter().map(|n|n.as_str().unwrap().parse().unwrap()).collect();
        assert_eq!(serde_json::json!(actual),*spacing);
        let f=families::make(id,settings["dex.kind"].as_str().unwrap()).unwrap();
        assert!(per_block::touched(f));
        if id=="aerodrome-slipstream-early" {
            assert_eq!(settings["dex.universal_router"],evidence["roles"]["router"]["address"]);
            assert_eq!(settings["dex.factory"],evidence["roles"]["factory"]["address"]);
            assert_eq!(settings["dex.quoter"],evidence["roles"]["quoter"]["address"]);
            assert_eq!(settings["dex.permit2"],evidence["permit2"]["address"]);
        }
    }
}
